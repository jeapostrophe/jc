use crate::views::code_view::CodeView;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::{InputEvent, Rope};
use jc_core::todo::{self, TodoDocument, TodoProblem};
use std::path::{Path, PathBuf};

/// TodoView wraps a [`CodeView`] opened on the project's `TODO.md` file,
/// adding parsing, highlighting, validation, and event emission on changes.
pub struct TodoView {
  code_view: Entity<CodeView>,
  file_path: PathBuf,
  project_path: PathBuf,
  document: TodoDocument,
  problems: Vec<TodoProblem>,
  active_label: Option<String>,
  _editor_subscription: Subscription,
}

impl TodoView {
  pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let file_path = project_path.join("TODO.md");
    let open_path = file_path.clone();
    let code_view = cx.new(|cx| {
      let mut cv = CodeView::new(window, cx);
      cv.set_language_override("todo-markdown", cx);
      cv.open_file(open_path, window, cx);
      cv
    });

    // Subscribe to editor changes.
    let editor_entity = code_view.read(cx).editor().clone();
    let _editor_subscription =
      cx.subscribe(&editor_entity, |this: &mut Self, _, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
          let text = this.code_view.read(cx).editor_text(cx);
          this.document = todo::parse(&text);
          this.apply_session_highlights(cx);
          // Skip re-validation on every keystroke (it scans the filesystem).
          cx.notify();
        }
      });

    // Initial parse and validate.
    let text = code_view.read(cx).editor_text(cx);
    let document = todo::parse(&text);
    let problems = todo::validate(&document, &project_path, &text);

    let mut view = Self {
      code_view,
      file_path,
      project_path,
      document,
      problems,
      active_label: None,
      _editor_subscription,
    };
    view.apply_diagnostics(cx);
    view
  }

  pub fn file_path(&self) -> &Path {
    &self.file_path
  }

  pub fn code_view(&self) -> &Entity<super::code_view::CodeView> {
    &self.code_view
  }

  pub fn is_dirty(&self, cx: &App) -> bool {
    self.code_view.read(cx).is_dirty(cx)
  }

  pub fn editor_text(&self, cx: &App) -> String {
    self.code_view.read(cx).editor_text(cx)
  }

  pub fn save(&self, cx: &mut Context<Self>) {
    self.code_view.update(cx, |cv, cx| cv.save(cx));
  }

  pub fn scroll_to_line(&self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
    self.code_view.update(cx, |cv, cx| cv.scroll_to_line(line, window, cx));
  }
}

impl super::LineSearchable for TodoView {
  fn editor_text(&self, cx: &App) -> String {
    self.editor_text(cx)
  }
  fn language_name(&self) -> crate::language::Language {
    crate::language::Language::Markdown
  }
  fn scroll_to_line(&self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
    self.code_view.update(cx, |cv, cx| cv.scroll_to_line(line, window, cx));
  }
}

impl TodoView {
  pub fn document(&self) -> &TodoDocument {
    &self.document
  }

  pub fn problems(&self) -> &[TodoProblem] {
    &self.problems
  }

  /// Set the active session label. The active session's headings get
  /// highlighted with the `@type` / `@function` theme colors while
  /// other sessions use default markdown heading colors.
  pub fn set_active_label(&mut self, label: Option<&str>, cx: &mut Context<Self>) {
    let changed = self.active_label.as_deref() != label;
    self.active_label = label.map(|s| s.to_string());
    if changed {
      self.apply_session_highlights(cx);
    }
  }

  /// Insert a `## <label>\n> uuid=<uuid>\n\n### WAIT\n` heading into the TODO.md,
  /// save, and revalidate.
  pub fn insert_session_heading(
    &mut self,
    uuid: &str,
    label: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let text = self.code_view.read(cx).editor_text(cx);
    let new_text = todo::insert_session_heading(&text, &self.document, uuid, label);
    self.code_view.update(cx, |cv, cx| {
      cv.editor().update(cx, |state, cx| {
        state.set_value_preserving_position(new_text, window, cx);
      });
    });
    self.revalidate(cx);
    self.save(cx);
  }

  pub fn insert_at_cursor(&self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
    let insert_text = format!("{text}\n");
    self.code_view.update(cx, |cv, cx| {
      cv.editor().update(cx, |state, cx| {
        state.insert(insert_text, window, cx);
      });
    });
  }

  pub fn insert_comment(
    &self,
    session_label: &str,
    comment: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(offset) = self.document.comment_insert_offset(session_label) else {
      return;
    };
    self.code_view.update(cx, |cv, cx| {
      let mut text = cv.editor_text(cx);
      text.insert_str(offset, comment);
      cv.editor().update(cx, |state, cx| {
        state.set_value_preserving_position(text, window, cx);
      });
    });
  }

  /// Mark a session heading as deleted in the TODO text.
  pub fn mark_session_deleted(
    &mut self,
    label: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let text = self.editor_text(cx);
    if let Some(new_text) = todo::mark_session_deleted(&text, &self.document, label) {
      self.code_view.update(cx, |cv, cx| {
        cv.editor().update(cx, |state, cx| {
          state.set_value_preserving_position(new_text, window, cx);
        });
      });
      self.revalidate(cx);
    }
  }


  /// Extract the selected text (or entire WAIT body if no selection) from the
  /// active session's WAIT section, wrap it in a new `### Message N` heading,
  /// and update the editor. Returns `(message_text, message_index)` on success.
  pub fn send_selection(
    &mut self,
    label: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Option<(String, usize)> {
    let text = self.editor_text(cx);
    let selection = self.code_view.read(cx).editor().read(cx).selection_byte_range();
    let session = self.document.session_by_label(label)?;
    let result = todo::send_from_wait(&text, session, selection)?;
    self.code_view.update(cx, |cv, cx| {
      cv.editor().update(cx, |state, cx| {
        state.set_value_preserving_position(result.new_text, window, cx);
      });
    });
    self.revalidate(cx);
    self.save(cx);
    Some((result.message_text, result.message_index))
  }

  /// Update a session's UUID in the TODO document text.
  pub fn update_session_uuid(
    &mut self,
    label: &str,
    new_uuid: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let text = self.editor_text(cx);
    if let Some(new_text) = todo::update_session_uuid(&text, &self.document, label, new_uuid) {
      self.code_view.update(cx, |cv, cx| {
        cv.editor().update(cx, |state, cx| {
          state.set_value_preserving_position(new_text, window, cx);
        });
      });
      self.revalidate(cx);
    }
  }

  /// Re-validate and refresh diagnostics. Call after loading or when the user
  /// explicitly requests a check (not on every keystroke, since it hits the FS).
  pub fn revalidate(&mut self, cx: &mut Context<Self>) {
    let text = self.code_view.read(cx).editor_text(cx);
    self.document = todo::parse(&text);
    self.problems = todo::validate(&self.document, &self.project_path, &text);
    self.apply_diagnostics(cx);
    self.apply_session_highlights(cx);
  }

  /// Push current problems as editor diagnostics.
  fn apply_diagnostics(&mut self, cx: &mut Context<Self>) {
    self.code_view.update(cx, |cv, cx| {
      cv.editor().update(cx, |state, cx| {
        let rope = Rope::from(state.value().as_ref());
        if let Some(diag_set) = state.diagnostics_mut() {
          diag_set.reset(&rope);
          // No more InvalidSessionSlug diagnostics — validation only checks unsent waits.
          cx.notify();
        }
      });
    });
  }

  /// Apply foreground highlights to the active session's headings.
  /// h2 (`## Label`) → `@type` color, h3 (`### Message` / `### WAIT`) → `@function` color.
  fn apply_session_highlights(&self, cx: &mut Context<Self>) {
    let session =
      self.active_label.as_deref().and_then(|label| self.document.session_by_label(label));

    let Some(session) = session else {
      self.code_view.update(cx, |cv, cx| {
        cv.editor().update(cx, |state, cx| {
          state.set_extra_highlights(Vec::new(), cx);
        });
      });
      return;
    };

    let theme = &cx.theme().highlight_theme;
    let h2_style = theme.style("type").unwrap_or_default();
    let h3_style = theme.style("function").unwrap_or_default();

    let mut highlights = Vec::new();

    // Highlight the session heading (## Label).
    highlights.push((session.heading_byte_range.clone(), h2_style));

    // Highlight all ### Message and ### WAIT headings within this session.
    for msg in &session.messages {
      highlights.push((msg.heading_byte_range.clone(), h3_style));
    }
    if let Some(wait) = &session.wait {
      highlights.push((wait.heading_byte_range.clone(), h3_style));
    }

    self.code_view.update(cx, |cv, cx| {
      cv.editor().update(cx, |state, cx| {
        state.set_extra_highlights(highlights, cx);
      });
    });
  }
}

impl Render for TodoView {
  fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
    div().id("todo-view").size_full().child(self.code_view.clone())
  }
}

impl Focusable for TodoView {
  fn focus_handle(&self, cx: &App) -> FocusHandle {
    self.code_view.read(cx).focus_handle(cx)
  }
}
