use crate::file_watcher::watch_dir;
use crate::language::Language;
use crate::outline::{OutlineItem, breadcrumb_at_byte, compute_outline};
use crate::views::comment_panel::CommentContext;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

actions!(code_view, [ReloadCodeFromDisk]);

pub fn init(cx: &mut App) {
  cx.bind_keys([KeyBinding::new("cmd-r", ReloadCodeFromDisk, Some("CodeView"))]);
}

pub struct CodeView {
  editor: Entity<InputState>,
  current_file: Option<PathBuf>,
  language_override: Option<SharedString>,
  dirty: bool,
  externally_modified: bool,
  saving: Arc<AtomicBool>,
  base_content: String,
  _subscription: Subscription,
  _observer: Subscription,
  _watcher: Option<notify::RecommendedWatcher>,
  /// Cached outline items for the current editor text.
  cached_outline: Vec<OutlineItem>,
  /// Breadcrumb labels for the current cursor position (e.g. ["impl Foo", "fn bar"]).
  breadcrumb: Vec<String>,
  /// Last cursor byte offset used for breadcrumb, to skip redundant lookups.
  last_breadcrumb_offset: usize,
}

impl CodeView {
  pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
    let editor = cx.new(|cx| {
      InputState::new(window, cx)
        .code_editor(Language::default().name())
        .soft_wrap(true)
        .line_number(false)
    });

    let subscription = cx.subscribe(&editor, |this: &mut Self, _, event: &InputEvent, cx| {
      if matches!(event, InputEvent::Change) {
        this.dirty = true;
        this.recompute_outline(cx);
      }
    });

    let observer = cx.observe(&editor, |this: &mut Self, _, cx| {
      this.update_breadcrumb(cx);
    });

    Self {
      editor,
      current_file: None,
      language_override: None,
      dirty: false,
      externally_modified: false,
      saving: Arc::new(AtomicBool::new(false)),
      base_content: String::default(),
      _subscription: subscription,
      _observer: observer,
      _watcher: None,
      cached_outline: Vec::new(),
      breadcrumb: Vec::new(),
      last_breadcrumb_offset: usize::MAX,
    }
  }

  pub fn file_path(&self) -> Option<&Path> {
    self.current_file.as_deref()
  }

  pub fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
    self.setup_watcher(&path, window, cx);
    self.current_file = Some(path);
    self.load_current(window, cx);
  }

  fn setup_watcher(&mut self, path: &Path, window: &Window, cx: &mut Context<Self>) {
    let Some(watched_file) = path.file_name() else { return };
    let watched_file = watched_file.to_os_string();
    let Some(parent) = path.parent() else { return };

    self._watcher = watch_dir(
      parent,
      move |p| p.ends_with(&watched_file),
      Some(self.saving.clone()),
      |view, window, cx| {
        if view.dirty {
          if !view.try_merge(window, cx) {
            view.externally_modified = true;
            cx.notify();
          }
        } else {
          view.load_current(window, cx);
        }
      },
      window,
      cx,
    );
  }

  fn load_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(path) = self.current_file.as_ref() else { return };
    let content = std::fs::read_to_string(path).unwrap_or_else(|e| format!("Error: {e}"));
    // Single pass: find the first changed line (or None if identical / initial load).
    let first_changed_line = first_differing_line(&self.base_content, &content);
    if first_changed_line.is_none() && !self.base_content.is_empty() && !self.dirty {
      return;
    }
    self.base_content = content.clone();
    let lang: SharedString =
      self.language_override.clone().unwrap_or_else(|| Language::from_path(path).name().into());
    self.editor.update(cx, |state, cx| {
      state.set_highlighter(lang, cx);
      state.set_value_preserving_position(content, window, cx);
      // Scroll to the first changed line so external edits are visible.
      if let Some(line) = first_changed_line {
        state.scroll_to_center_line(line, cx);
      }
    });
    self.dirty = false;
    self.externally_modified = false;
    self.recompute_outline(cx);
    cx.notify();
  }

  fn reload_from_disk(
    &mut self,
    _: &ReloadCodeFromDisk,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.load_current(window, cx);
  }

  /// Attempt a three-way merge of disk changes with buffer edits.
  /// Returns `true` if the merge succeeded (or no real change), `false` on conflict.
  fn try_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    let Some(path) = self.current_file.as_ref() else {
      return false;
    };
    let Ok(theirs) = std::fs::read_to_string(path) else {
      return false;
    };
    if theirs == self.base_content {
      return true;
    }
    let ours = self.editor.read(cx).value().to_string();
    match diffy::merge(&self.base_content, &ours, &theirs) {
      Ok(merged) => {
        self.editor.update(cx, |state, cx| {
          state.set_value_preserving_position(merged, window, cx);
        });
        self.base_content = theirs;
        cx.notify();
        true
      }
      Err(_) => false,
    }
  }
}

impl CodeView {
  pub fn is_dirty(&self, cx: &App) -> bool {
    self.editor.read(cx).value().as_ref() != self.base_content
  }

  pub fn has_conflict(&self) -> bool {
    self.externally_modified
  }

  pub fn editor(&self) -> &Entity<InputState> {
    &self.editor
  }

  /// Set a persistent language override. When set, file reloads use this
  /// language instead of detecting from the file extension.
  pub fn set_language_override(&mut self, lang: impl Into<SharedString>, cx: &mut Context<Self>) {
    let lang: SharedString = lang.into();
    self.language_override = Some(lang.clone());
    self.editor.update(cx, |state, cx| {
      state.set_highlighter(lang, cx);
    });
  }

  pub fn editor_text(&self, cx: &App) -> String {
    super::editor_text(&self.editor, cx)
  }

  pub fn save(&mut self, cx: &mut Context<Self>) {
    let Some(path) = self.current_file.as_ref() else { return };
    self.saving.store(true, Ordering::Relaxed);
    let content = self.editor.read(cx).value();
    if let Err(e) = std::fs::write(path, content.as_ref()) {
      eprintln!("Failed to save {}: {e}", path.display());
    }
    self.base_content = content.to_string();
    self.dirty = false;

    // Clear saving flag after a brief delay to suppress self-triggered watcher event.
    let saving = self.saving.clone();
    cx.spawn(async move |_this: WeakEntity<CodeView>, _cx: &mut AsyncApp| {
      Timer::after(std::time::Duration::from_millis(200)).await;
      saving.store(false, Ordering::Relaxed);
    })
    .detach();
  }

  pub fn current_language(&self) -> Language {
    self.current_file.as_deref().map(Language::from_path).unwrap_or_default()
  }

  /// Return the current breadcrumb labels (ancestor chain at cursor).
  pub fn breadcrumb(&self) -> &[String] {
    &self.breadcrumb
  }

  /// Recompute the outline from the current editor text. Called on text changes.
  fn recompute_outline(&mut self, cx: &mut Context<Self>) {
    let lang = if self.language_override.as_ref().map(|s| s.as_ref()) == Some("todo-markdown") {
      Language::Markdown
    } else {
      self.current_language()
    };
    let text = self.editor.read(cx).value().to_string();
    self.cached_outline = compute_outline(&text, lang);
    self.last_breadcrumb_offset = usize::MAX; // force re-evaluation
    self.update_breadcrumb(cx);
  }

  /// Update breadcrumb based on current cursor position. Only notifies if changed.
  fn update_breadcrumb(&mut self, cx: &mut Context<Self>) {
    if self.cached_outline.is_empty() {
      if !self.breadcrumb.is_empty() {
        self.breadcrumb.clear();
        self.last_breadcrumb_offset = usize::MAX;
        cx.notify();
      }
      return;
    }
    let byte_offset = self.editor.read(cx).selection_byte_range().start;
    if byte_offset == self.last_breadcrumb_offset {
      return;
    }
    self.last_breadcrumb_offset = byte_offset;
    let chain = breadcrumb_at_byte(&self.cached_outline, byte_offset);
    let new_breadcrumb: Vec<String> = chain.into_iter().map(|item| item.name.clone()).collect();
    if new_breadcrumb != self.breadcrumb {
      self.breadcrumb = new_breadcrumb;
      cx.notify();
    }
  }

  pub fn comment_context(&self, project_path: &Path, cx: &App) -> Option<CommentContext> {
    let file_path = self.current_file.as_ref()?;
    let relative = file_path.strip_prefix(project_path).ok().unwrap_or(file_path);
    let (start, end) = super::selection_line_range(&self.editor, cx);
    let prefilled =
      format!("* {}:{} \u{2014} ", relative.display(), super::format_line_range(start, end));
    Some(CommentContext { prefilled })
  }

  pub fn scroll_to_line(&self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
    super::scroll_editor_to_line(&self.editor, line, window, cx);
  }
}

impl super::LineSearchable for CodeView {
  fn editor_text(&self, cx: &App) -> String {
    self.editor_text(cx)
  }
  fn language_name(&self) -> Language {
    self.current_language()
  }
  fn scroll_to_line(&self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
    self.scroll_to_line(line, window, cx)
  }
}

impl Focusable for CodeView {
  fn focus_handle(&self, cx: &App) -> FocusHandle {
    self.editor.read(cx).focus_handle(cx)
  }
}

impl Render for CodeView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    div()
      .id("code-view")
      .key_context("CodeView")
      .track_focus(&self.editor.read(cx).focus_handle(cx))
      .size_full()
      .font_family("Lilex")
      .on_action(cx.listener(Self::reload_from_disk))
      .child(super::external_change_banner(self.externally_modified, cx))
      .child(Input::new(&self.editor).h_full().appearance(false).bordered(false))
  }
}

/// Return the 0-based line index of the first line that differs between `old`
/// and `new`, or `None` when the texts are identical (or either is empty on
/// first load).
fn first_differing_line(old: &str, new: &str) -> Option<u32> {
  if old.is_empty() {
    return None; // initial load — don't scroll
  }
  let mut old_lines = old.lines();
  let mut new_lines = new.lines();
  let mut line = 0u32;
  loop {
    match (old_lines.next(), new_lines.next()) {
      (Some(a), Some(b)) if a == b => line += 1,
      (None, None) => return None, // identical
      _ => return Some(line),
    }
  }
}
