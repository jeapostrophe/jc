use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use git2::DiffFormat;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::{Input, InputState};

actions!(diff_view, [MarkReviewed]);

pub fn init(cx: &mut App) {
  cx.bind_keys([KeyBinding::new("cmd-r", MarkReviewed, Some("DiffView"))]);
}

pub enum DiffViewEvent {
  Reviewed,
}

impl EventEmitter<DiffViewEvent> for DiffView {}

pub struct FileDiff {
  pub name: String,
  pub content: String,
  pub checksum: u64,
}

pub struct DiffView {
  editor: Entity<InputState>,
  project_path: PathBuf,
  file_diffs: Vec<FileDiff>,
  current_file_index: usize,
  reviewed: HashMap<String, u64>,
}

impl DiffView {
  pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let editor = cx
      .new(|cx| InputState::new(window, cx).code_editor("diff").soft_wrap(true).line_number(false));
    let mut view = Self {
      editor,
      project_path,
      file_diffs: Vec::new(),
      current_file_index: 0,
      reviewed: HashMap::default(),
    };
    view.refresh(window, cx);
    view
  }

  pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let diff_text = generate_diff(&self.project_path);
    self.file_diffs = parse_file_diffs(&diff_text);

    // Prune reviewed entries: remove if file is gone or checksum changed.
    self.reviewed.retain(|name, checksum| {
      self.file_diffs.iter().any(|fd| fd.name == *name && fd.checksum == *checksum)
    });

    // Reset to first unreviewed file, or 0 if all reviewed.
    self.current_file_index =
      self.file_diffs.iter().position(|fd| !self.is_reviewed(&fd.name)).unwrap_or(0);

    self.show_current_file(window, cx);
  }

  fn show_current_file(&self, window: &mut Window, cx: &mut Context<Self>) {
    let content = if self.file_diffs.is_empty() {
      String::default()
    } else {
      self.file_diffs[self.current_file_index].content.clone()
    };
    self.editor.update(cx, |state, cx| {
      state.set_value(content, window, cx);
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
      use std::collections::hash_map::Entry;
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
  pub fn file_diffs(&self) -> &[FileDiff] {
    &self.file_diffs
  }

  pub fn is_reviewed(&self, name: &str) -> bool {
    if let Some(&stored_checksum) = self.reviewed.get(name) {
      self.file_diffs.iter().any(|fd| fd.name == name && fd.checksum == stored_checksum)
    } else {
      false
    }
  }

  pub fn current_file_name(&self) -> Option<&str> {
    self.file_diffs.get(self.current_file_index).map(|fd| fd.name.as_str())
  }

  pub fn reviewed_count(&self) -> usize {
    self.file_diffs.iter().filter(|fd| self.is_reviewed(&fd.name)).count()
  }

  pub fn file_count(&self) -> usize {
    self.file_diffs.len()
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

fn parse_file_diffs(diff_text: &str) -> Vec<FileDiff> {
  let mut diffs = Vec::new();
  let mut current_name: Option<String> = None;
  let mut current_content = String::default();

  for line in diff_text.lines() {
    if let Some(rest) = line.strip_prefix("diff --git a/") {
      // Flush previous file diff.
      if let Some(name) = current_name.take() {
        let checksum = compute_checksum(&current_content);
        diffs.push(FileDiff { name, content: current_content.clone(), checksum });
      }
      let name = rest.split(" b/").next().unwrap_or(rest).to_string();
      current_name = Some(name);
      current_content.clear();
      current_content.push_str(line);
      current_content.push('\n');
    } else {
      current_content.push_str(line);
      current_content.push('\n');
    }
  }

  // Flush last file.
  if let Some(name) = current_name {
    let checksum = compute_checksum(&current_content);
    diffs.push(FileDiff { name, content: current_content, checksum });
  }

  diffs
}

fn compute_checksum(content: &str) -> u64 {
  let mut hasher = DefaultHasher::default();
  content.hash(&mut hasher);
  hasher.finish()
}
