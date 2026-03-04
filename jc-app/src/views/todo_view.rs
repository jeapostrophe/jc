use crate::views::code_view::CodeView;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::highlighter::{Diagnostic, DiagnosticSeverity};
use gpui_component::input::{InputEvent, Position, Rope};
use jc_core::todo::{self, TodoDocument, TodoProblem, TodoSession};
use std::ops::Range;
use std::path::{Path, PathBuf};

/// TodoView wraps a [`CodeView`] opened on the project's `TODO.md` file,
/// adding parsing, highlighting, validation, and event emission on changes.
pub struct TodoView {
  code_view: Entity<CodeView>,
  file_path: PathBuf,
  project_path: PathBuf,
  document: TodoDocument,
  problems: Vec<TodoProblem>,
  active_slug: Option<String>,
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
      active_slug: None,
      _editor_subscription,
    };
    view.apply_diagnostics(cx);
    view
  }

  pub fn file_path(&self) -> &Path {
    &self.file_path
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
    self.scroll_to_line(line, window, cx)
  }
}

impl TodoView {
  pub fn document(&self) -> &TodoDocument {
    &self.document
  }

  pub fn problems(&self) -> &[TodoProblem] {
    &self.problems
  }

  /// Set the active session slug. The active session's headings get
  /// highlighted with the `@type` / `@function` theme colors while
  /// other sessions use default markdown heading colors.
  pub fn set_active_slug(&mut self, slug: Option<&str>, cx: &mut Context<Self>) {
    let changed = self.active_slug.as_deref() != slug;
    self.active_slug = slug.map(|s| s.to_string());
    if changed {
      self.apply_session_highlights(cx);
    }
  }

  /// Insert a `## Session <slug>: <label>` heading into the TODO.md,
  /// save, and revalidate.
  pub fn insert_session_heading(
    &mut self,
    slug: &str,
    label: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let text = self.code_view.read(cx).editor_text(cx);
    let new_text = todo::insert_session_heading(&text, &self.document, slug, label);
    self.code_view.update(cx, |cv, cx| {
      cv.editor().update(cx, |state, cx| {
        state.set_value(new_text, window, cx);
      });
    });
    self.revalidate(cx);
    self.save(cx);
  }

  pub fn insert_comment(
    &self,
    session_slug: &str,
    comment: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(offset) = self.document.comment_insert_offset(session_slug) else {
      return;
    };
    self.code_view.update(cx, |cv, cx| {
      let text = cv.editor_text(cx);
      let mut new_text = text;
      new_text.insert_str(offset, comment);
      cv.editor().update(cx, |state, cx| {
        state.set_value(new_text, window, cx);
      });
    });
  }

  /// Extract the selected text (or entire WAIT body if no selection) from the
  /// active session's WAIT section, wrap it in a new `### Message N` heading,
  /// and update the editor. Returns `(message_text, message_index)` on success.
  pub fn send_selection(
    &mut self,
    slug: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Option<(String, usize)> {
    let text = self.editor_text(cx);
    let selection = self.code_view.read(cx).editor().read(cx).selection_byte_range();
    let session = self.document.session_by_slug(slug)?;
    let result = todo::send_from_wait(&text, session, selection)?;
    self.code_view.update(cx, |cv, cx| {
      cv.editor().update(cx, |state, cx| {
        state.set_value(result.new_text, window, cx);
      });
    });
    self.revalidate(cx);
    self.save(cx);
    Some((result.message_text, result.message_index))
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

  /// Push current problems as editor diagnostics (wavy underlines on invalid slugs).
  fn apply_diagnostics(&mut self, cx: &mut Context<Self>) {
    self.code_view.update(cx, |cv, cx| {
      cv.editor().update(cx, |state, cx| {
        let rope = Rope::from(state.value().as_ref());
        if let Some(diag_set) = state.diagnostics_mut() {
          diag_set.reset(&rope);
          for problem in &self.problems {
            match problem {
              TodoProblem::UnsentWait { .. } => {}
              TodoProblem::InvalidSessionSlug { slug, line, .. } => {
                // Position is 0-based; our line numbers are 1-based.
                // Slug starts at column 11 ("## Session ".len()).
                let col_start = "## Session ".len() as u32;
                let col_end = col_start + slug.len() as u32;
                let line_0 = line.saturating_sub(1);
                diag_set.push(
                  Diagnostic::new(
                    Position::new(line_0, col_start)..Position::new(line_0, col_end),
                    format!("No JSONL session found for slug '{slug}'"),
                  )
                  .with_severity(DiagnosticSeverity::Error),
                );
              }
            }
          }
          cx.notify();
        }
      });
    });
  }

  /// Apply foreground highlights to the active session's headings.
  /// h2 (`## Session`) → `@type` color, h3 (`### Message` / `### WAIT`) → `@function` color.
  fn apply_session_highlights(&self, cx: &mut Context<Self>) {
    let slug = match &self.active_slug {
      Some(s) => s.as_str(),
      None => {
        // Clear highlights.
        self.code_view.update(cx, |cv, cx| {
          cv.editor().update(cx, |state, cx| {
            state.set_extra_highlights(Vec::new(), cx);
          });
        });
        return;
      }
    };

    let session = match self.document.session_by_slug(slug) {
      Some(s) => s,
      None => {
        self.code_view.update(cx, |cv, cx| {
          cv.editor().update(cx, |state, cx| {
            state.set_extra_highlights(Vec::new(), cx);
          });
        });
        return;
      }
    };

    let theme = &cx.theme().highlight_theme;
    let h2_style = theme.style("type").unwrap_or_default();
    let h3_style = theme.style("function").unwrap_or_default();

    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();

    // Highlight the session heading (## Session slug: label).
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

/// Compute the full byte range of a session (heading through content,
/// up to the next session's heading or end of document).
#[allow(dead_code)]
fn session_full_range(session: &TodoSession, doc: &TodoDocument, text_len: usize) -> Range<usize> {
  let start = session.heading_byte_range.start;
  let end = doc
    .sessions
    .iter()
    .find(|s| s.heading_byte_range.start > start)
    .map(|next| next.heading_byte_range.start)
    .unwrap_or(text_len);
  start..end
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
