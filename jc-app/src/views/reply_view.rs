use crate::file_watcher::watch_dir;
use crate::views::comment_panel::CommentContext;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::{Input, InputState};
use jc_core::session::{
  Turn, discover_latest_session_group, discover_session_group, parse_session_group, session_dir,
};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

pub fn init(_cx: &mut App) {}

pub fn gc_stale_replies(project_path: &Path) {
  let replies_dir = project_path.join(".jc/replies");
  let Ok(entries) = std::fs::read_dir(&replies_dir) else { return };
  let cutoff = std::time::SystemTime::now()
    .checked_sub(std::time::Duration::from_secs(7 * 24 * 3600))
    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
  for entry in entries.flatten() {
    if let Ok(meta) = entry.metadata()
      && let Ok(mtime) = meta.modified()
      && mtime < cutoff
    {
      let _ = std::fs::remove_file(entry.path());
    }
  }
}

pub struct ReplyView {
  editor: Entity<InputState>,
  project_path: PathBuf,
  /// The slug identifying the current session group (stable across forks).
  session_slug: Option<String>,
  turns: Vec<Turn>,
  current_turn_index: usize,
  /// Hash of the last content written to the turn file on disk.
  last_written_hash: u64,
  /// Hash of the last content shown in the editor (to skip no-op updates).
  last_shown_hash: u64,
  _watcher: Option<notify::RecommendedWatcher>,
}

impl ReplyView {
  pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let editor = cx.new(|cx| {
      InputState::new(window, cx).code_editor("markdown").soft_wrap(true).line_number(false)
    });

    let replies_dir = project_path.join(".jc/replies");
    let _ = std::fs::create_dir_all(&replies_dir);

    let mut view = Self {
      editor,
      project_path,
      session_slug: None,
      turns: Vec::new(),
      current_turn_index: 0,
      last_written_hash: 0,
      last_shown_hash: 0,
      _watcher: None,
    };
    view.discover_and_parse();
    if !view.turns.is_empty() {
      view.current_turn_index = view.turns.len() - 1;
    }
    view.show_current_turn(window, cx);
    view.setup_watcher(window, cx);
    view
  }

  /// Re-parse session files. Does NOT touch current_turn_index.
  fn discover_and_parse(&mut self) {
    let group = if let Some(slug) = &self.session_slug {
      discover_session_group(&self.project_path, slug)
    } else {
      discover_latest_session_group(&self.project_path)
    };

    if let Some(group) = group {
      self.session_slug = Some(group.slug.clone());
      self.turns = parse_session_group(&group);
    } else {
      self.turns.clear();
      self.session_slug = None;
    }
  }

  pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let prev_turn_count = self.turns.len();
    let was_at_latest = self.current_turn_index + 1 >= prev_turn_count || prev_turn_count == 0;
    let prev_index = self.current_turn_index;

    self.discover_and_parse();

    // If the user was viewing the latest turn, follow new turns.
    // Otherwise preserve their position (clamped to valid range).
    if was_at_latest && !self.turns.is_empty() {
      self.current_turn_index = self.turns.len() - 1;
    } else if self.turns.is_empty() {
      self.current_turn_index = 0;
    } else {
      self.current_turn_index = prev_index.min(self.turns.len() - 1);
    }

    self.show_current_turn(window, cx);
  }

  fn show_current_turn(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let content = if self.turns.is_empty() {
      String::default()
    } else {
      let turn = &self.turns[self.current_turn_index];
      let md = turn.render_markdown();
      self.write_turn_file(&md);
      md
    };

    // Skip editor update if content hasn't changed to preserve scroll/cursor.
    let mut hasher = DefaultHasher::default();
    content.hash(&mut hasher);
    let hash = hasher.finish();
    if hash == self.last_shown_hash {
      return;
    }
    self.last_shown_hash = hash;

    self.editor.update(cx, |state, cx| {
      state.set_value(content, window, cx);
    });
  }

  pub fn set_turn_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
    if index < self.turns.len() {
      self.current_turn_index = index;
      self.show_current_turn(window, cx);
    }
  }

  fn setup_watcher(&mut self, window: &Window, cx: &mut Context<Self>) {
    // Watch the entire session directory so we catch modifications to any file
    // in the slug group AND new fork files being created.
    let dir = session_dir(&self.project_path);
    if !dir.is_dir() {
      return;
    }

    self._watcher = watch_dir(
      &dir,
      |p| p.extension().is_some_and(|ext| ext == "jsonl"),
      None,
      |view, window, cx| view.refresh(window, cx),
      window,
      cx,
    );
  }

  fn write_turn_file(&mut self, content: &str) {
    let mut hasher = DefaultHasher::default();
    content.hash(&mut hasher);
    let hash = hasher.finish();
    if hash == self.last_written_hash {
      return;
    }
    self.last_written_hash = hash;
    let replies_dir = self.project_path.join(".jc/replies");
    let filename = format!("turn_{:04}.md", self.current_turn_index);
    let _ = std::fs::write(replies_dir.join(filename), content);
  }
}

impl ReplyView {
  pub fn turns(&self) -> &[Turn] {
    &self.turns
  }

  pub fn current_turn_index(&self) -> usize {
    self.current_turn_index
  }

  pub fn current_turn_label(&self) -> String {
    if self.turns.is_empty() {
      return "No session".to_string();
    }
    self.turns[self.current_turn_index].label()
  }

  pub fn editor(&self) -> &Entity<InputState> {
    &self.editor
  }

  pub fn editor_text(&self, cx: &App) -> String {
    super::editor_text(&self.editor, cx)
  }

  pub fn comment_context(&self, cx: &App) -> Option<CommentContext> {
    if self.turns.is_empty() {
      return None;
    }
    let filename = format!(".jc/replies/turn_{:04}.md", self.current_turn_index);
    let (start, end) = super::selection_line_range(&self.editor, cx);
    let prefilled = format!("* {filename}:{} \u{2014} ", super::format_line_range(start, end));
    Some(CommentContext { prefilled })
  }

  pub fn scroll_to_line(&self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
    super::scroll_editor_to_line(&self.editor, line, window, cx);
  }
}

impl super::LineSearchable for ReplyView {
  fn editor_text(&self, cx: &App) -> String {
    self.editor_text(cx)
  }
  fn language_name(&self) -> crate::language::Language {
    crate::language::Language::Markdown
  }
  fn scroll_to_line(&self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
    self.scroll_to_line(line, window, cx)
  }
}

impl ReplyView {
  pub fn project_path(&self) -> &Path {
    &self.project_path
  }

  pub fn set_session_slug(
    &mut self,
    slug: Option<String>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.session_slug != slug {
      self.session_slug = slug;
      self.refresh(window, cx);
    }
  }
}

impl Focusable for ReplyView {
  fn focus_handle(&self, cx: &App) -> FocusHandle {
    self.editor.read(cx).focus_handle(cx)
  }
}

impl Render for ReplyView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();

    if self.turns.is_empty() {
      return div()
        .size_full()
        .key_context("ReplyView")
        .flex()
        .items_center()
        .justify_center()
        .child(div().text_color(theme.muted_foreground).child("No session found"));
    }

    div()
      .size_full()
      .key_context("ReplyView")
      .font_family("Lilex")
      .child(Input::new(&self.editor).h_full().appearance(false).bordered(false).disabled(true))
  }
}
