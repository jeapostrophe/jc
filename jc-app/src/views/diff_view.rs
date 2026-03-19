use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::{Path, PathBuf};

use crate::language::Language;
use crate::views::comment_panel::CommentContext;
use git2::DiffFormat;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::{Input, InputState};
use std::ops::Range;

actions!(diff_view, [MarkReviewed]);

pub fn init(cx: &mut App) {
  cx.bind_keys([KeyBinding::new("cmd-r", MarkReviewed, Some("DiffView"))]);
}

pub enum DiffViewEvent {
  Reviewed,
}

impl EventEmitter<DiffViewEvent> for DiffView {}

#[derive(Clone)]
pub enum DiffSource {
  WorkingTree,
  Commit { oid: git2::Oid, summary: String },
}

impl DiffSource {
  pub fn label(&self) -> &str {
    match self {
      Self::WorkingTree => "Working tree",
      Self::Commit { summary, .. } => summary,
    }
  }
}

pub struct GitLogEntry {
  pub oid: git2::Oid,
  pub short_hash: String,
  pub summary: String,
}

pub struct FileDiff {
  pub name: String,
  pub content: String,
  pub checksum: u64,
}

pub struct DiffView {
  editor: Entity<InputState>,
  project_path: PathBuf,
  source: DiffSource,
  file_diffs: Vec<FileDiff>,
  current_file_index: usize,
  reviewed: HashMap<String, u64>,
  /// Mtime of `.git/index` at last refresh, used for staleness detection.
  git_index_mtime: Option<std::time::SystemTime>,
}

fn git_index_mtime(project_path: &Path) -> Option<std::time::SystemTime> {
  std::fs::metadata(project_path.join(".git/index")).ok().and_then(|m| m.modified().ok())
}

impl DiffView {
  pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let editor = cx
      .new(|cx| InputState::new(window, cx).code_editor("diff").soft_wrap(true).line_number(false));
    let mut view = Self {
      editor,
      project_path,
      source: DiffSource::WorkingTree,
      file_diffs: Vec::new(),
      current_file_index: 0,
      reviewed: HashMap::default(),
      git_index_mtime: None,
    };
    view.refresh(window, cx);
    view
  }

  pub fn set_source(&mut self, source: DiffSource, window: &mut Window, cx: &mut Context<Self>) {
    self.source = source;
    self.reviewed.clear();
    self.refresh(window, cx);
  }

  pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let diff_text = match &self.source {
      DiffSource::WorkingTree => generate_diff(&self.project_path),
      DiffSource::Commit { oid, .. } => generate_commit_diff(&self.project_path, *oid),
    };
    self.file_diffs = parse_file_diffs(&diff_text);
    self.git_index_mtime = git_index_mtime(&self.project_path);

    // Prune reviewed entries: remove if file is gone or checksum changed.
    self.reviewed.retain(|name, checksum| {
      self.file_diffs.iter().any(|fd| fd.name == *name && fd.checksum == *checksum)
    });

    // Reset to first unreviewed file, or 0 if all reviewed.
    self.current_file_index =
      self.file_diffs.iter().position(|fd| !self.is_reviewed(&fd.name)).unwrap_or(0);

    self.show_current_file(window, cx);
  }

  /// Returns true if the diff data may have changed since the last refresh.
  /// For WorkingTree diffs this always returns true because `.git/index`
  /// mtime only tracks staged changes, not working-tree edits.
  pub fn is_stale(&self) -> bool {
    match self.source {
      DiffSource::WorkingTree => true,
      DiffSource::Commit { .. } => git_index_mtime(&self.project_path) != self.git_index_mtime,
    }
  }

  /// Refresh diff data without updating the editor display.
  /// Used by the background problem poll to keep file lists current.
  /// Returns true if the diff content actually changed.
  pub fn refresh_data(&mut self) -> bool {
    let diff_text = match &self.source {
      DiffSource::WorkingTree => generate_diff(&self.project_path),
      DiffSource::Commit { oid, .. } => generate_commit_diff(&self.project_path, *oid),
    };
    let new_diffs = parse_file_diffs(&diff_text);
    self.git_index_mtime = git_index_mtime(&self.project_path);

    let changed = new_diffs.len() != self.file_diffs.len()
      || new_diffs
        .iter()
        .zip(self.file_diffs.iter())
        .any(|(a, b)| a.name != b.name || a.checksum != b.checksum);
    self.file_diffs = new_diffs;

    self.reviewed.retain(|name, checksum| {
      self.file_diffs.iter().any(|fd| fd.name == *name && fd.checksum == *checksum)
    });

    self.current_file_index =
      self.file_diffs.iter().position(|fd| !self.is_reviewed(&fd.name)).unwrap_or(0);

    changed
  }

  fn show_current_file(&self, window: &mut Window, cx: &mut Context<Self>) {
    let (content, language) = if self.file_diffs.is_empty() {
      (String::default(), Language::default())
    } else {
      let fd = &self.file_diffs[self.current_file_index];
      (fd.content.clone(), Language::from_path(Path::new(&fd.name)))
    };
    let is_dark = cx.theme().is_dark();
    let backgrounds = diff_line_backgrounds(&content, is_dark);
    self.editor.update(cx, |state, cx| {
      state.set_highlighter(language.name(), cx);
      state.set_value(content, window, cx);
      state.set_line_backgrounds(backgrounds, cx);
    });
  }

  pub fn set_file_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
    if index < self.file_diffs.len() {
      self.current_file_index = index;
      self.show_current_file(window, cx);
    }
  }

  fn mark_reviewed(&mut self, _: &MarkReviewed, _window: &mut Window, cx: &mut Context<Self>) {
    if let Some(fd) = self.file_diffs.get(self.current_file_index) {
      let name = fd.name.clone();
      let checksum = fd.checksum;
      match self.reviewed.entry(name) {
        Entry::Occupied(e) => {
          e.remove();
        }
        Entry::Vacant(e) => {
          e.insert(checksum);
        }
      }
      cx.emit(DiffViewEvent::Reviewed);
      cx.notify();
    }
  }
}

impl DiffView {
  pub fn source(&self) -> &DiffSource {
    &self.source
  }

  pub fn file_diffs(&self) -> &[FileDiff] {
    &self.file_diffs
  }

  pub fn is_reviewed(&self, name: &str) -> bool {
    self.reviewed.contains_key(name)
  }

  pub fn current_file_name(&self) -> Option<&str> {
    self.file_diffs.get(self.current_file_index).map(|fd| fd.name.as_str())
  }

  pub fn unreviewed_files(&self) -> Vec<PathBuf> {
    self
      .file_diffs
      .iter()
      .filter(|fd| !self.is_reviewed(&fd.name))
      .map(|fd| PathBuf::from(&fd.name))
      .collect()
  }

  pub fn reviewed_count(&self) -> usize {
    self.file_diffs.iter().filter(|fd| self.is_reviewed(&fd.name)).count()
  }

  pub fn file_count(&self) -> usize {
    self.file_diffs.len()
  }

  pub fn editor(&self) -> &Entity<InputState> {
    &self.editor
  }

  pub fn project_path(&self) -> &Path {
    &self.project_path
  }

  pub fn editor_text(&self, cx: &App) -> String {
    super::editor_text(&self.editor, cx)
  }

  pub fn comment_context(&self, cx: &App) -> Option<CommentContext> {
    let file_name = self.current_file_name()?;
    let (start, end) = super::selection_line_range(&self.editor, cx);
    let lines = super::format_line_range(start, end);
    let source_prefix = match &self.source {
      DiffSource::WorkingTree => String::default(),
      DiffSource::Commit { oid, .. } => format!("{:.7}:", oid),
    };
    let prefilled = format!("* git-diff:{source_prefix}{file_name}:{lines} \u{2014} ");
    Some(CommentContext { prefilled })
  }

  pub fn current_file_language(&self) -> Language {
    self
      .file_diffs
      .get(self.current_file_index)
      .map(|fd| Language::from_path(Path::new(&fd.name)))
      .unwrap_or_default()
  }

  pub fn scroll_to_line(&self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
    super::scroll_editor_to_line(&self.editor, line, window, cx);
  }
}

impl super::LineSearchable for DiffView {
  fn editor_text(&self, cx: &App) -> String {
    self.editor_text(cx)
  }
  fn language_name(&self) -> Language {
    self.current_file_language()
  }
  fn scroll_to_line(&self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
    self.scroll_to_line(line, window, cx)
  }
}

impl Focusable for DiffView {
  fn focus_handle(&self, cx: &App) -> FocusHandle {
    self.editor.read(cx).focus_handle(cx)
  }
}

impl Render for DiffView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();

    if self.file_diffs.is_empty() {
      return div()
        .size_full()
        .key_context("DiffView")
        .flex()
        .items_center()
        .justify_center()
        .child(div().text_color(theme.muted_foreground).child("No changes"));
    }

    div()
      .size_full()
      .key_context("DiffView")
      .on_action(cx.listener(Self::mark_reviewed))
      .font_family("Lilex")
      .child(Input::new(&self.editor).h_full().appearance(false).bordered(false).disabled(true))
  }
}

fn generate_diff(path: &Path) -> String {
  match generate_diff_inner(path) {
    Ok(result) => result,
    Err(e) => format!("Error generating diff: {e}"),
  }
}

fn generate_diff_inner(path: &Path) -> Result<String, git2::Error> {
  let repo = git2::Repository::open(path)?;
  let head = repo.head()?;
  let tree = head.peel_to_tree()?;
  let diff = repo.diff_tree_to_workdir_with_index(Some(&tree), None)?;
  diff_to_string(&diff)
}

fn parse_file_diffs(diff_text: &str) -> Vec<FileDiff> {
  let mut diffs = Vec::new();
  let mut current_name: Option<String> = None;
  let mut current_content = String::default();

  for line in diff_text.lines() {
    if let Some(rest) = line.strip_prefix("diff --git a/") {
      // Flush previous file diff.
      if let Some(name) = current_name.take() {
        let checksum = super::compute_checksum(&current_content);
        diffs.push(FileDiff { name, content: std::mem::take(&mut current_content), checksum });
      }
      let name = rest.split(" b/").next().unwrap_or(rest).to_string();
      if name == "TODO.md" || name == ".claude/settings.local.json" {
        current_name = None;
        current_content.clear();
        continue;
      }
      current_name = Some(name);
      current_content.push_str(line);
      current_content.push('\n');
    } else if current_name.is_some() {
      current_content.push_str(line);
      current_content.push('\n');
    }
  }

  // Flush last file.
  if let Some(name) = current_name {
    let checksum = super::compute_checksum(&current_content);
    diffs.push(FileDiff { name, content: current_content, checksum });
  }

  diffs
}

fn generate_commit_diff(path: &Path, oid: git2::Oid) -> String {
  match generate_commit_diff_inner(path, oid) {
    Ok(result) => result,
    Err(e) => format!("Error generating commit diff: {e}"),
  }
}

fn generate_commit_diff_inner(path: &Path, oid: git2::Oid) -> Result<String, git2::Error> {
  let repo = git2::Repository::open(path)?;
  let commit = repo.find_commit(oid)?;
  let tree = commit.tree()?;

  let parent_tree = if commit.parent_count() > 0 { Some(commit.parent(0)?.tree()?) } else { None };

  let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;
  diff_to_string(&diff)
}

fn diff_to_string(diff: &git2::Diff) -> Result<String, git2::Error> {
  let mut output = String::default();
  diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
    match line.origin() {
      '+' | '-' | ' ' => output.push(line.origin()),
      _ => {}
    }
    let content = std::str::from_utf8(line.content()).unwrap_or("");
    output.push_str(content);
    true
  })?;
  Ok(output)
}

/// Build per-line background segments for diff added/deleted lines.
fn diff_line_backgrounds(content: &str, is_dark: bool) -> Vec<(Range<usize>, Hsla)> {
  let (added_bg, deleted_bg) = if is_dark {
    (hsla(0.33, 0.35, 0.18, 0.6), hsla(0.0, 0.35, 0.18, 0.6))
  } else {
    (hsla(0.33, 0.40, 0.85, 0.4), hsla(0.0, 0.40, 0.85, 0.4))
  };

  let mut segments = Vec::new();
  let mut offset = 0;

  for line in content.split('\n') {
    let line_end = offset + line.len();
    let bg = match line.as_bytes().first() {
      Some(b'+') => Some(added_bg),
      Some(b'-') => Some(deleted_bg),
      _ => None,
    };
    if let Some(color) = bg {
      segments.push((offset..line_end, color));
    }
    offset = line_end + 1;
  }

  segments
}

const MAX_LOG_ENTRIES: usize = 500;

pub fn git_log(path: &Path) -> Vec<GitLogEntry> {
  git_log_inner(path).unwrap_or_default()
}

fn git_log_inner(path: &Path) -> Result<Vec<GitLogEntry>, git2::Error> {
  let repo = git2::Repository::open(path)?;
  let mut revwalk = repo.revwalk()?;
  revwalk.push_head()?;
  revwalk.set_sorting(git2::Sort::TIME)?;

  let mut entries = Vec::new();
  for oid_result in revwalk.take(MAX_LOG_ENTRIES) {
    let oid = oid_result?;
    let commit = repo.find_commit(oid)?;
    let summary = commit.summary().unwrap_or("").to_string();
    let short_hash = format!("{:.7}", oid);
    entries.push(GitLogEntry { oid, short_hash, summary });
  }

  Ok(entries)
}
