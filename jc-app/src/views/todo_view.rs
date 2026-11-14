use crate::views::code_view::CodeView;
use chrono::NaiveDateTime;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::{InputEvent, RopeExt as _};
use jc_core::flavor::Flavor;
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

/// TodoView wraps a [`CodeView`] opened on this app's TODO for a project
/// (`Flavor::todo_path`: jc's is the project's `TODO.md`, jcc's its todo.md in
/// Cira's store), adding parsing, highlighting, validation, and event emission
/// on changes. jcc's opens only once Cira names the project; until then it
/// holds no file, and nothing in it is saved.
pub struct TodoView {
  code_view: Entity<CodeView>,
  document: TodoDocument,
  /// Whose sessions the document holds (`jc_core::flavor::Flavor::section`).
  flavor: Flavor,
  /// jcc, while no file is open: why, if more than waiting for Cira.
  waiting: Option<String>,
  active_key: Option<String>,
  _editor_subscription: Subscription,
}

impl TodoView {
  pub fn new(todo: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let code_view = cx.new(|cx| {
      let mut cv = CodeView::new(window, cx);
      cv.set_language_override("todo-markdown", cx);
      if let Some(todo) = todo {
        cv.open_file(todo, window, cx);
      }
      cv
    });

    // Subscribe to editor changes.
    let editor_entity = code_view.read(cx).editor().clone();
    let _editor_subscription =
      cx.subscribe(&editor_entity, |this: &mut Self, _, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
          let text = this.code_view.read(cx).editor_text(cx);
          this.document = todo::parse(&text, this.flavor);
          this.apply_session_highlights(cx);
          cx.notify();
        }
      });

    // Initial parse.
    let text = code_view.read(cx).editor_text(cx);
    let flavor = Flavor::current();
    let document = todo::parse(&text, flavor);

    Self { code_view, document, flavor, waiting: None, active_key: None, _editor_subscription }
  }

  /// Open `todo`, for a view made with none (jcc, once Cira names the project).
  pub fn open(&mut self, todo: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
    self.waiting = None;
    self.code_view.update(cx, |cv, cx| cv.open_file(todo, window, cx));
    self.reparse(cx);
  }

  /// Say why the TODO is not open (jcc). Returns whether that changed.
  pub fn set_waiting(&mut self, why: Option<String>, cx: &mut Context<Self>) -> bool {
    let changed = self.waiting != why;
    if changed {
      self.waiting = why;
      cx.notify();
    }
    changed
  }

  /// The banner over this TODO (`waiting_banner`).
  pub fn banner(&self, cx: &App) -> Option<String> {
    let cira_down = crate::views::cira_reach(cx).down().is_some();
    waiting_banner(self.is_open(cx), self.waiting.as_deref(), cira_down)
  }

  pub fn is_open(&self, cx: &App) -> bool {
    self.code_view.read(cx).file_path().is_some()
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

  /// Save, merging in what another writer (the other app, Jay's editor)
  /// saved meanwhile, and re-parse what the merge brought in. Nothing is
  /// written while the file is conflicted ([`CodeView::save`]).
  pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    let saved = self.code_view.update(cx, |cv, cx| cv.save(window, cx));
    self.reparse(cx);
    saved
  }

  pub fn scroll_to_line(&self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
    self.code_view.update(cx, |cv, cx| cv.scroll_to_line(line, window, cx));
  }

  /// 0-based line of the last line in the WAIT body of the session bound to
  /// `uuid` — where the cursor goes so the user can type.
  pub fn wait_line(&self, uuid: &str, cx: &App) -> Option<u32> {
    let wait = self.document.session_by_id(uuid)?.wait.as_ref()?;
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

  /// Set the active session's heading id. Its headings get highlighted with the
  /// `@type` / `@function` theme colors while other sessions use default
  /// markdown heading colors.
  pub fn set_active_key(&mut self, key: Option<&str>, cx: &mut Context<Self>) {
    let changed = self.active_key.as_deref() != key;
    self.active_key = key.map(|s| s.to_string());
    if changed {
      self.apply_session_highlights(cx);
    }
  }

  /// Insert a `## <label>` heading with id `id` (a uuid for jc, a Cira id for
  /// jcc: [`todo::insert_session_heading`]), and save.
  pub fn insert_session_heading(
    &mut self,
    label: &str,
    id: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let text = self.code_view.read(cx).editor_text(cx);
    let new_text = todo::insert_session_heading(&text, &self.document, label, id);
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
    let Some(file_path) = self.code_view.read(cx).file_path().map(PathBuf::from) else { return };
    let text = self.editor_text(cx);
    let Some(new_text) = todo::truncate_all_sessions(&text, self.flavor, todo::MAX_MESSAGES) else {
      return;
    };

    // The sweep is not undoable -- `set_value_preserving_position` replaces the
    // buffer with history suppressed, and the save lands before the window is
    // even shown -- and TODO.md is typically not in version control. Keep one
    // copy of what the log looked like before it was first bounded. Only the
    // first sweep writes it; later ones are no-ops anyway, and never clobber it.
    let mut backup = file_path.clone().into_os_string();
    backup.push(".bak");
    let backup = std::path::PathBuf::from(backup);
    if !backup.exists()
      && let Err(e) = std::fs::write(&backup, &text)
    {
      eprintln!("Failed to back up {} before truncating: {e}", file_path.display());
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
      // Reachable whenever memory and file disagree (a hand-edited id line, a
      // heading renamed since the picker snapshot). Cmd-Shift-Backspace then does nothing at all, so say
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
    uuid: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Option<(String, usize, Option<NaiveDateTime>)> {
    let text = self.editor_text(cx);
    let selection = self.code_view.read(cx).editor().read(cx).selection_byte_range();
    let session = self.document.session_by_id(uuid)?;
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
    self.save(window, cx);
    Some((result.message_text, result.message_index, result.schedule))
  }

  /// Whether the session bound to `uuid` currently has a pending scheduled
  /// `### Message N @jc(...)` marker. Reads the cached `self.document`, which is re-parsed on every
  /// editor change, so a marker the user just added or removed is already
  /// reflected — no re-parse needed on this per-send hot path.
  pub fn has_pending_schedule(&self, uuid: &str) -> bool {
    self.document.session_by_id(uuid).and_then(|s| s.pending_scheduled()).is_some()
  }

  /// Record the just-sent message `index` of the session bound to `id` as a
  /// scheduled send due at `when` (see [`todo::defer_message`]), and save.
  pub fn defer_message(
    &mut self,
    id: &str,
    index: usize,
    when: NaiveDateTime,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let text = self.editor_text(cx);
    if let Some(new_text) = todo::defer_message(&text, &self.document, id, index, when) {
      self.set_text_and_save(new_text, window, cx);
    }
  }

  /// Fire a scheduled send for the session bound to `uuid`. jc-core [`todo::fire_scheduled`] owns the
  /// policy (has the time arrived? cancelled? rescheduled?) and text rewrite;
  /// this only applies the rewritten text and maps to a [`ScheduledFire`] for the
  /// workspace to act on.
  pub fn deliver_scheduled(
    &mut self,
    uuid: &str,
    now_local: NaiveDateTime,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> ScheduledFire {
    let text = self.editor_text(cx);
    match todo::fire_scheduled(&text, self.flavor, uuid, now_local, now_unix_secs()) {
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
    self.save(window, cx);
  }

  /// Ensure the session has a WAIT section, inserting one if missing.
  /// Returns true if a WAIT section was added.
  pub fn ensure_wait(&mut self, uuid: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
    let text = self.editor_text(cx);
    if let Some(new_text) = todo::insert_wait_section(&text, &self.document, uuid) {
      self.set_text(new_text, window, cx);
      self.save(window, cx);
      true
    } else {
      false
    }
  }

  /// Give the heading at `index` in the parsed document its Cira id (see
  /// [`todo::bind_cira_at`]) and save. Positional because the heading's old
  /// id (a legacy `> uuid=`, or none) is about to stop being its address.
  pub fn bind_cira_at(
    &mut self,
    index: usize,
    cira: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let text = self.editor_text(cx);
    if let Some(new_text) = todo::bind_cira_at(&text, &self.document, index, cira) {
      self.set_text_and_save(new_text, window, cx);
    }
  }

  /// Write `new_uuid` onto jc's session at `index` in the parsed document
  /// ([`todo::update_session_uuid_at`]). Positional because the caller minting
  /// a uuid has no uuid to key on yet. The caller saves.
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

  /// Rename the heading at `index` (keeping its `[D]`) and save; `false`
  /// when the label is not one jc writes into a heading
  /// ([`todo::rename_heading_at`]).
  pub fn rename_heading_at(
    &mut self,
    index: usize,
    label: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> bool {
    let text = self.editor_text(cx);
    let Some(new_text) = todo::rename_heading_at(&text, &self.document, index, label) else {
      return false;
    };
    self.set_text_and_save(new_text, window, cx);
    true
  }

  /// Re-parse the document and refresh highlights. Called by [`Self::set_text`]
  /// after every jc-authored edit; not run on every keystroke, since it re-parses.
  pub fn reparse(&mut self, cx: &mut Context<Self>) {
    let text = self.code_view.read(cx).editor_text(cx);
    self.document = todo::parse(&text, self.flavor);
    self.apply_session_highlights(cx);
  }

  /// Apply foreground highlights to the active session's headings.
  /// h2 (`## Label`) → `@type` color, h3 (`### Message` / `### WAIT`) → `@function`
  /// color, except a pending scheduled `### Message N @jc(...)` heading → `@keyword`
  /// color so it stands out as not-yet-delivered.
  fn apply_session_highlights(&self, cx: &mut Context<Self>) {
    let session = self.active_key.as_deref().and_then(|uuid| self.document.session_by_id(uuid));

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
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let waiting = self.banner(cx);
    div()
      .id("todo-view")
      .size_full()
      .flex()
      .flex_col()
      .when_some(waiting, |el, why| {
        let theme = cx.theme();
        el.child(
          div()
            .px_2()
            .py_1()
            .bg(theme.warning)
            .text_sm()
            .text_color(theme.warning_foreground)
            .child(why),
        )
      })
      .child(self.code_view.clone())
  }
}

/// The banner over a TODO: none once it is open, nor while Cira is down,
/// which the window's Cira banner says; else why it is not open.
fn waiting_banner(open: bool, why: Option<&str>, cira_down: bool) -> Option<String> {
  (!open && !cira_down).then(|| {
    why.unwrap_or("jcc's TODO opens once Cira names this project (`cira status`)").to_string()
  })
}

impl Focusable for TodoView {
  fn focus_handle(&self, cx: &App) -> FocusHandle {
    self.code_view.read(cx).focus_handle(cx)
  }
}

#[cfg(test)]
mod tests {
  use super::TodoView;
  use crate::views::{cira_banner, read_cira};
  use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};
  use jc_core::cira::{Down, Status};

  /// Every banner the window shows about Cira: its own, and each TODO's.
  fn banners(views: &[Entity<TodoView>], cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|_, cx| {
      cira_banner(cx)
        .into_iter()
        .chain(views.iter().filter_map(|tv| tv.read(cx).banner(cx)))
        .collect()
    })
  }

  fn todo_view(cx: &mut VisualTestContext) -> Entity<TodoView> {
    cx.update(|window, cx| cx.new(|cx| TodoView::new(None, window, cx)))
  }

  /// With `cira` not installed, jcc opened with three banners for one cause
  /// (Q-165); and a project opened during the outage added another (review
  /// P1).
  #[gpui::test]
  fn one_cause_shows_one_banner(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let cx = cx.add_empty_window();
    let mut views = vec![todo_view(cx), todo_view(cx)];
    let down = Err(Down::NotInstalled);
    let raised = (0..3).filter(|_| cx.update(|_, cx| read_cira(&down, views.clone(), cx))).count();
    assert_eq!(raised, 1, "a poll that fails the same way again raises nothing new");
    views.push(todo_view(cx));
    let shown = banners(&views, cx);
    assert_eq!(shown.len(), 1, "one banner for one cause: {shown:?}");
    assert!(shown[0].contains("not found on PATH"), "{shown:?}");

    // Back: the Cira banner clears, and each TODO not open yet says why.
    let up = Ok(Status::parse(br#"{"home":"/store","projects":[]}"#).unwrap());
    assert!(cx.update(|_, cx| read_cira(&up, views.clone(), cx)));
    let shown = banners(&views, cx);
    assert_eq!(shown.len(), 3, "{shown:?}");
    assert!(shown.iter().all(|b| b.contains("opens once Cira names")), "{shown:?}");
  }
}
