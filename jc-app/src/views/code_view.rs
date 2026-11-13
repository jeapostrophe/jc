use crate::file_watcher::watch_dir;
use crate::language::Language;
use crate::outline::{OutlineItem, breadcrumb_at_byte, compute_outline};
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use jc_core::shared_file::{DiskChange, Saved, SharedFile};
use std::path::{Path, PathBuf};

actions!(code_view, [ReloadCodeFromDisk]);

pub fn init(cx: &mut App) {
  cx.bind_keys([KeyBinding::new("cmd-r", ReloadCodeFromDisk, Some("CodeView"))]);
}

pub struct CodeView {
  editor: Entity<InputState>,
  /// The open file, and what this pane last read from or wrote to it.
  file: Option<SharedFile>,
  language_override: Option<SharedString>,
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
        this.recompute_outline(cx);
      }
    });

    let observer = cx.observe(&editor, |this: &mut Self, _, cx| {
      this.update_breadcrumb(cx);
    });

    Self {
      editor,
      file: None,
      language_override: None,
      _subscription: subscription,
      _observer: observer,
      _watcher: None,
      cached_outline: Vec::new(),
      breadcrumb: Vec::new(),
      last_breadcrumb_offset: usize::MAX,
    }
  }

  pub fn file_path(&self) -> Option<&Path> {
    self.file.as_ref().map(SharedFile::path)
  }

  pub fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
    self.setup_watcher(&path, window, cx);
    self.file = Some(SharedFile::new(path));
    self.load_current(window, cx);
  }

  /// Watch the file a symlink names, not the link: a TODO.md linked into a
  /// dot-files repo changes there.
  fn setup_watcher(&mut self, path: &Path, window: &Window, cx: &mut Context<Self>) {
    let path = jc_core::replace::resolve(path).unwrap_or_else(|_| path.to_path_buf());
    let Some(watched_file) = path.file_name() else { return };
    let watched_file = watched_file.to_os_string();
    let Some(parent) = path.parent() else { return };

    self._watcher = watch_dir(
      parent,
      move |p| p.ends_with(&watched_file),
      |view, window, cx| view.disk_changed(window, cx),
      window,
      cx,
    );
  }

  /// The watcher saw the file change: [`SharedFile::disk_changed`] says
  /// whether it was this pane's own save, someone else's to show (merged with
  /// unsaved edits), or a conflict.
  fn disk_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let ours = self.editor.read(cx).value().to_string();
    let Some(file) = self.file.as_mut() else { return };
    let old_base = file.base().to_string();
    match file.disk_changed(&ours) {
      DiskChange::Unchanged => {}
      DiskChange::Replace(text) => {
        // An unedited pane scrolls to the change so it is seen.
        let scroll = (ours == old_base).then(|| first_differing_line(&old_base, &text)).flatten();
        self.editor.update(cx, |state, cx| {
          state.set_value_preserving_position(text, window, cx);
          if let Some(line) = scroll {
            state.scroll_to_center_line(line, cx);
          }
        });
        self.recompute_outline(cx);
        cx.notify();
      }
      DiskChange::Conflict => cx.notify(),
    }
  }

  /// Show the disk's text, dropping unsaved edits and any conflict (Cmd-R,
  /// and opening a file).
  fn load_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let ours = self.editor.read(cx).value().to_string();
    let Some(file) = self.file.as_mut() else { return };
    let old_base = file.base().to_string();
    let was_conflicted = file.has_conflict();
    let content = file.reload();
    // Single pass: find the first changed line (or None if identical / initial load).
    let first_changed_line = first_differing_line(&old_base, &content);
    if first_changed_line.is_none() && !old_base.is_empty() && ours == old_base {
      if was_conflicted {
        cx.notify();
      }
      return;
    }
    let lang: SharedString = self
      .language_override
      .clone()
      .unwrap_or_else(|| Language::from_path(file.path()).name().into());
    self.editor.update(cx, |state, cx| {
      state.set_highlighter(lang, cx);
      state.set_value_preserving_position(content, window, cx);
      // Scroll to the first changed line so external edits are visible.
      if let Some(line) = first_changed_line {
        state.scroll_to_center_line(line, cx);
      }
    });
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
}

impl CodeView {
  pub fn is_dirty(&self, cx: &App) -> bool {
    self.file.as_ref().is_some_and(|f| self.editor.read(cx).value().as_ref() != f.base())
  }

  /// Unsaved edits conflict with a change on disk: nothing is saved until
  /// Cmd-R reloads ([`SharedFile::save`]).
  pub fn has_conflict(&self) -> bool {
    self.file.as_ref().is_some_and(SharedFile::has_conflict)
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

  /// Save the buffer, merged with whatever another writer saved since this
  /// pane last read the file ([`SharedFile::save`]); a merge shows in the
  /// buffer. `false` when nothing was written: the file is conflicted (the
  /// banner says so) or could not be written.
  pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    let ours = self.editor.read(cx).value().to_string();
    let Some(file) = self.file.as_mut() else { return false };
    match file.save(&ours) {
      Ok(Saved::Written(text)) => {
        if text != ours {
          self.editor.update(cx, |state, cx| {
            state.set_value_preserving_position(text, window, cx);
          });
          self.recompute_outline(cx);
          cx.notify();
        }
        true
      }
      Ok(Saved::Conflict) => {
        cx.notify();
        false
      }
      Err(e) => {
        eprintln!("Failed to save {}: {e}", file.path().display());
        false
      }
    }
  }

  pub fn current_language(&self) -> Language {
    self.file_path().map(Language::from_path).unwrap_or_default()
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
      .child(super::external_change_banner(self.has_conflict(), cx))
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
