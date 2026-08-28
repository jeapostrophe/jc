use crate::views::code_view::CodeView;
use chrono::NaiveDateTime;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::{InputEvent, RopeExt as _};
use jc_core::todo::{self, SessionKey, TodoDocument};
use std::path::PathBuf;

/// Current Unix time in whole seconds, used for `> last=` timestamps.
fn now_unix_secs() -> u64 {
  std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// Outcome of firing a scheduled-send timer, computed against the live TODO text.
pub enum ScheduledFire {
  /// Deliver this (current) message body to the Claude terminal now.
  Deliver(String),
  /// The scheduled time was pushed into the future — re-arm for this instant.
  Reschedule(NaiveDateTime),
  /// The marker was removed (or the session/body is gone) — do nothing.
  Cancelled,
}

/// TodoView wraps a [`CodeView`] opened on the project's `TODO.md` file,
/// adding parsing, highlighting, validation, and event emission on changes.
pub struct TodoView {
  code_view: Entity<CodeView>,
  file_path: PathBuf,
  document: TodoDocument,
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
          cx.notify();
        }
      });

    // Initial parse.
    let text = code_view.read(cx).editor_text(cx);
    let document = todo::parse(&text);

    Self { code_view, file_path, document, active_label: None, _editor_subscription }
  }

  pub fn code_view(&self) -> &Entity<super::code_view::CodeView> {
    &self.code_view
  }

  pub fn editor(&self, cx: &App) -> Entity<gpui_component::input::InputState> {
    self.code_view.read(cx).editor().clone()
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

  /// Return the line number of the last line in the WAIT body for `label`.
  pub fn wait_line(&self, label: &str, cx: &App) -> Option<u32> {
    let wait = self.document.session_by_label(label)?.wait.as_ref()?;
    Some(wait.body_end_line(&self.editor_text(cx)))
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

  /// Insert a `## <label>\n> uuid=<uuid>\n\n### WAIT\n` heading and save.
  pub fn insert_session_heading(
    &mut self,
    uuid: &str,
    label: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let text = self.code_view.read(cx).editor_text(cx);
    let new_text = todo::insert_session_heading(&text, &self.document, uuid, label);
    self.set_text_and_save(new_text, window, cx);
  }

  /// Bound every session's message log to the most recent
  /// [`todo::MAX_MESSAGES`], dropping older entries and saving if any went.
  ///
  /// Sends bound only the session written to; this is the startup sweep that
  /// catches sessions which have gone quiet. Callers must run it before reading
  /// the document for anything else.
  ///
  /// Unlike its neighbours here it saves rather than leaving that to the caller,
  /// so the backup below and the destructive write it exists to protect have one
  /// owner -- a `.bak` must never outlive a rewrite that did not reach disk.
  pub fn truncate_logs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let text = self.editor_text(cx);
    let Some(new_text) = todo::truncate_all_sessions(&text, todo::MAX_MESSAGES) else {
      return;
    };

    // The sweep is not undoable -- `set_value_preserving_position` replaces the
    // buffer with history suppressed, and the save lands before the window is
    // even shown -- and TODO.md is typically not in version control. Keep one
    // copy of what the log looked like before it was first bounded. Only the
    // first sweep writes it; later ones are no-ops anyway, and never clobber it.
    let mut backup = self.file_path.clone().into_os_string();
    backup.push(".bak");
    let backup = std::path::PathBuf::from(backup);
    if !backup.exists()
      && let Err(e) = std::fs::write(&backup, &text)
    {
      eprintln!("Failed to back up {} before truncating: {e}", self.file_path.display());
    }

    self.set_text_and_save(new_text, window, cx);
  }

  /// Toggle the `[D]` prefix on the heading `key` names.
  pub fn toggle_session_disabled(
    &mut self,
    key: &SessionKey,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(index) = self.document.index_of(key) else {
      // Reachable whenever memory and file disagree (a `/clear` whose heading
      // write missed, a hand-edited `> uuid=`, a heading renamed since the
      // picker snapshot). Cmd-Shift-Backspace then does nothing at all, so say
      // why rather than appearing to be ignored.
      eprintln!("toggle-disabled: no TODO heading for {key:?}");
      return;
    };
    let text = self.editor_text(cx);
    if let Some(new_text) = todo::toggle_session_disabled_at(&text, &self.document, index) {
      self.set_text(new_text, window, cx);
    }
  }

  /// Extract the selected text (or everything before the cursor in the WAIT
  /// body) from the active session's WAIT section into a new `### Message N`
  /// heading, and update the editor. Returns the message text and — when the
  /// WAIT began with a `@jc(HH:MM)` marker — the resolved scheduled delivery
  /// time (in which case the caller must defer delivery rather than sending
  /// immediately).
  pub fn send_selection(
    &mut self,
    label: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Option<(String, Option<NaiveDateTime>)> {
    let text = self.editor_text(cx);
    let selection = self.code_view.read(cx).editor().read(cx).selection_byte_range();
    let session = self.document.session_by_label(label)?;
    let now = now_unix_secs();
    let now_local = chrono::Local::now().naive_local();
    let result = todo::send_from_wait(&text, session, selection, Some(now), now_local)?;
    let wait_body_offset = result.wait_body_offset;
    self.code_view.update(cx, |cv, cx| {
      cv.editor().update(cx, |state, cx| {
        state.set_value_preserving_position(result.new_text, window, cx);
        let pos = state.text().offset_to_position(wait_body_offset);
        state.set_cursor_position(pos, window, cx);
      });
    });
    self.reparse(cx);
    self.save(cx);
    Some((result.message_text, result.schedule))
  }

  /// Whether `label` currently has a pending scheduled `### Message N @jc(...)`
  /// marker. Reads the cached `self.document`, which is re-parsed on every
  /// editor change, so a marker the user just added or removed is already
  /// reflected — no re-parse needed on this per-send hot path.
  pub fn has_pending_schedule(&self, label: &str) -> bool {
    self.document.session_by_label(label).and_then(|s| s.pending_scheduled()).is_some()
  }

  /// Fire a scheduled send for `label`. jc-core [`todo::fire_scheduled`] owns the
  /// policy (has the time arrived? cancelled? rescheduled?) and text rewrite;
  /// this only applies the rewritten text and maps to a [`ScheduledFire`] for the
  /// workspace to act on.
  pub fn deliver_scheduled(
    &mut self,
    label: &str,
    now_local: NaiveDateTime,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> ScheduledFire {
    let text = self.editor_text(cx);
    match todo::fire_scheduled(&text, label, now_local, now_unix_secs()) {
      todo::FireOutcome::Deliver { new_text, body } => {
        self.set_text_and_save(new_text, window, cx);
        ScheduledFire::Deliver(body)
      }
      todo::FireOutcome::Reschedule(when) => ScheduledFire::Reschedule(when),
      todo::FireOutcome::Cancelled { new_text } => {
        if let Some(new_text) = new_text {
          self.set_text_and_save(new_text, window, cx);
        }
        ScheduledFire::Cancelled
      }
    }
  }

  /// Replace the editor contents (preserving cursor position), then re-parse
  /// and save.
  /// Replace the buffer's text and re-parse. Every jc-authored edit goes
  /// through here, so there is one place to change how edits are applied.
  fn set_text(&mut self, new_text: String, window: &mut Window, cx: &mut Context<Self>) {
    self.code_view.update(cx, |cv, cx| {
      cv.editor().update(cx, |state, cx| {
        state.set_value_preserving_position(new_text, window, cx);
      });
    });
    self.reparse(cx);
  }

  fn set_text_and_save(&mut self, new_text: String, window: &mut Window, cx: &mut Context<Self>) {
    self.set_text(new_text, window, cx);
    self.save(cx);
  }

  /// Ensure the session has a WAIT section, inserting one if missing.
  /// Returns true if a WAIT section was added.
  pub fn ensure_wait(&mut self, label: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
    let text = self.editor_text(cx);
    if let Some(new_text) = todo::insert_wait_section(&text, &self.document, label) {
      self.set_text(new_text, window, cx);
      self.save(cx);
      true
    } else {
      false
    }
  }

  /// Repair duplicate labels in one rewrite. Returns whether anything changed.
  pub fn dedupe_labels(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    let text = self.editor_text(cx);
    let Some(new_text) = todo::dedupe_labels(&text, &self.document) else { return false };
    self.set_text_and_save(new_text, window, cx);
    true
  }

  /// Write `new_uuid` onto the session at `index` in the parsed document.
  /// Positional because the caller minting a UUID has no UUID to key on yet.
  pub fn update_session_uuid_at(
    &mut self,
    index: usize,
    new_uuid: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let text = self.editor_text(cx);
    if let Some(new_text) = todo::update_session_uuid_at(&text, &self.document, index, new_uuid) {
      self.set_text(new_text, window, cx);
    }
  }

  /// Re-parse the document and refresh highlights. Called by [`Self::set_text`]
  /// after every jc-authored edit; not run on every keystroke, since it re-parses.
  pub fn reparse(&mut self, cx: &mut Context<Self>) {
    let text = self.code_view.read(cx).editor_text(cx);
    self.document = todo::parse(&text);
    self.apply_session_highlights(cx);
  }

  /// Apply foreground highlights to the active session's headings.
  /// h2 (`## Label`) → `@type` color, h3 (`### Message` / `### WAIT`) → `@function`
  /// color, except a pending scheduled `### Message N @jc(...)` heading → `@keyword`
  /// color so it stands out as not-yet-delivered.
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
    let scheduled_style = theme.style("keyword").unwrap_or_default();

    let mut highlights = Vec::new();

    // Highlight the session heading (## Label).
    highlights.push((session.heading_byte_range.clone(), h2_style));

    // Highlight all ### Message and ### WAIT headings within this session.
    // A pending scheduled message gets a distinct color until it's delivered.
    for msg in &session.messages {
      let style = if msg.schedule.is_some() { scheduled_style } else { h3_style };
      highlights.push((msg.heading_byte_range.clone(), style));
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
