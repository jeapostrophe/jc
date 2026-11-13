//! jc's own session lifecycle (`Flavor::Jc`; Cira PLAN TMUX-07): each Claude
//! pane runs `claude --session-id|--resume <uuid>` in jc's own terminal, the
//! heading's `> uuid=` is the session's address, and hooks match it by that
//! uuid. None of this runs in jcc, whose lifecycle is Cira's (`sessions.rs`);
//! nothing here calls `cira`.

use super::Workspace;
use crate::views::session_state::{Heading, SessionId, SessionState, Start};
use gpui::*;
use jc_core::claude::Launch;
use jc_core::hooks::{HookEvent, HookEventKind};
use jc_core::todo::{SessionKey, TodoSession};

impl Workspace {
  /// Launch a brand new Claude session under a uuid jc mints itself, so the
  /// session is identified from the first instant rather than being detected
  /// after the fact.
  pub(super) fn own_new_session(
    &mut self,
    project_idx: usize,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    // A label is a display name, not an address — see `SessionKey`. This just
    // stops the picker showing several identical rows.
    let label = jc_core::todo::unique_label(
      self.projects[project_idx].todo_view.read(cx).document(),
      "New Session",
    );
    let uuid = uuid::Uuid::new_v4().to_string();
    let todo_view = self.projects[project_idx].todo_view.clone();
    todo_view.update(cx, |tv, cx| tv.insert_session_heading(&label, &uuid, window, cx));
    self.own_start(project_idx, uuid, label, Launch::New, false, window, cx);
  }

  /// Adopt transcript `uuid`, which no heading names, under a new heading
  /// labelled from `summary`.
  pub(super) fn own_adopt_transcript(
    &mut self,
    pi: usize,
    uuid: String,
    summary: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    // The summary is the transcript's first user message, so two sessions that
    // opened with the same prompt would otherwise get the same display name.
    // Addressing is by uuid either way (see `jc_core::todo::SessionKey`); this
    // is for the picker's sake.
    let todo_view = self.projects[pi].todo_view.clone();
    let label = jc_core::todo::unique_label(todo_view.read(cx).document(), summary);
    todo_view.update(cx, |tv, cx| tv.insert_session_heading(&label, &uuid, window, cx));
    self.own_adopt_session(pi, &SessionKey::Id(uuid), window, cx);
  }

  /// Adopt a TODO.md session that isn't running yet. The launch flag comes from
  /// whether the transcript is still on disk (`ProjectState::launch_for`), so a
  /// session Claude has garbage-collected comes back as a fresh conversation
  /// under its own uuid instead of a dead pane.
  ///
  /// `key` may be a `SessionKey::Unbound` — a heading an older jc left with a
  /// blank `> uuid=`. Adoption is the one operation that mints a uuid, and it
  /// happens here rather than at startup so binding a legacy heading to a new
  /// (empty) conversation is something the user chose to do. The heading is
  /// re-resolved against the *live* document, so a key taken from a picker
  /// snapshot cannot write to a heading that has moved since.
  pub(super) fn own_adopt_session(
    &mut self,
    project_idx: usize,
    key: &SessionKey,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let todo_view = self.projects[project_idx].todo_view.clone();

    // Bind an unbound heading now, in one write. A minted uuid has no
    // conversation *by construction*, so it is `Launch::New` outright — asking
    // `launch_for` would get `Resume` when the filesystem can't be seen at all
    // (`$HOME` unset) and spawn `claude --resume <fresh-uuid>` into a dead pane,
    // permanently, since the heading is bound by then.
    let resolved = todo_view.update(cx, |tv, cx| {
      let index = tv.document().index_of(key)?;
      let heading = &tv.document().sessions[index];
      // The display name comes off the heading we just resolved, never from the
      // picker snapshot: the two can disagree, and the file is the truth.
      let label = heading.label.clone();
      let uuid = heading.id.clone();
      if !uuid.is_empty() {
        return Some((uuid, label, None));
      }
      let minted = uuid::Uuid::new_v4().to_string();
      tv.update_session_uuid_at(index, &minted, window, cx);
      Some((minted, label, Some(Launch::New)))
    });

    // Refuse to launch a session whose uuid isn't in the file. It could never
    // be label-synced or disabled afterwards (both key on the uuid), and the
    // still-unbound heading would stay adoptable — so a second press would
    // spawn a second process for the same heading.
    let Some((uuid, label, minted_launch)) = resolved else {
      eprintln!("adopt: no TODO heading for {key:?}; not launching");
      // The picker already dropped `pre_picker_focus` on the assumption that
      // this call would focus something. It won't, so do it here or the next
      // keystroke goes nowhere.
      self.panes[self.active_pane_index].read(cx).focus_content(window);
      return;
    };

    // Clear `[D]` if the heading was dormant, then save once for both edits —
    // each save is a full-document copy and a blocking write on the main thread.
    let key = SessionKey::Id(uuid.clone());
    let is_disabled =
      todo_view.read(cx).document().session_of(&key).is_some_and(TodoSession::is_disabled);
    todo_view.update(cx, |tv, cx| {
      if is_disabled {
        tv.toggle_session_disabled(&key, window, cx);
      }
      tv.save(cx);
    });

    let project = &mut self.projects[project_idx];
    let dangerous =
      project.todo_view.read(cx).document().session_by_id(&uuid).is_some_and(|s| s.dangerous);
    let launch = minted_launch.unwrap_or_else(|| project.launch_for(&uuid));
    self.own_start(project_idx, uuid, label, launch, dangerous, window, cx);
  }

  /// Run `claude` for heading `uuid` in a new session and show it.
  #[allow(clippy::too_many_arguments)]
  fn own_start(
    &mut self,
    project_idx: usize,
    uuid: String,
    label: String,
    launch: Launch,
    dangerous: bool,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let palette = super::palette_from_window(window);
    let project = &mut self.projects[project_idx];
    let id = project.next_session_id;
    project.next_session_id += 1;
    let session = SessionState::create(
      id,
      Heading { key: uuid, label, cira: None },
      Start::Own { launch, dangerous },
      &project.path,
      &palette,
      window,
      cx,
    );
    project.sessions.insert(id, session);
    self.switch_to_session(project_idx, Some(id), window, cx);
  }

  /// Toggle the `[D]` marker on the heading `key` names, detaching its terminal
  /// if it is running and just became disabled. Never addressed by label: with
  /// two same-labelled headings a label-keyed toggle marks the wrong one and
  /// detaches a live session (see `SessionKey`).
  pub(super) fn own_toggle_session_disabled(
    &mut self,
    project_idx: usize,
    key: &SessionKey,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let project = &mut self.projects[project_idx];
    let todo_view = project.todo_view.clone();

    // Check if the session is currently adopted (running). An unbound heading
    // never is, so this is `None` for an `Unbound` key.
    let adopted_id: Option<SessionId> = match key {
      SessionKey::Id(uuid) => project.session_by_key(uuid).map(|(id, _)| id),
      SessionKey::Unbound { .. } => None,
    };

    todo_view.update(cx, |tv, cx| {
      tv.toggle_session_disabled(key, window, cx);
      tv.save(cx);
    });

    let is_now_disabled =
      todo_view.read(cx).document().session_of(key).is_some_and(TodoSession::is_disabled);
    // Whether control ended up in `switch_to_session`, which focuses the pane it
    // lands on. When it doesn't, nothing else will: the picker's confirm handler
    // has already dropped `pre_picker_focus`, so focus would be stranded on the
    // dismissed picker input and keystrokes would go nowhere.
    let mut switched = false;
    if is_now_disabled && let Some(id) = adopted_id {
      switched = self.remove_running_session(project_idx, id, window, cx);
    }
    if !switched {
      // Disabling a background session, or re-enabling one: the view didn't
      // move, so put focus back where it was before the picker opened.
      self.panes[self.active_pane_index].read(cx).focus_content(window);
    }
    cx.notify();
  }

  /// Apply a hook event to the session whose uuid it names. uuids are assigned
  /// by jc at launch (`--session-id`), so a hook for an unknown uuid belongs to
  /// a session jc doesn't manage (a jcc session in the same project, say) and
  /// is ignored. Returns the matched `(project name, session label)`.
  pub(super) fn own_apply_hook(&mut self, event: &HookEvent, cx: &App) -> Option<(String, String)> {
    let (pi, sid) = self.session_for_hook(&event.session_id)?;
    let project = &mut self.projects[pi];
    let project_name = project.name.clone();
    let session = project.sessions.get_mut(&sid)?;
    match event.kind {
      HookEventKind::PromptSubmit => session.mark_user_input(cx),
      HookEventKind::Stop
      | HookEventKind::StopFailure
      | HookEventKind::PermissionPrompt
      | HookEventKind::IdlePrompt => session.busy = false,
      HookEventKind::SessionStart | HookEventKind::SessionClear { .. } => return None,
    }
    Some((project_name, session.label.clone()))
  }

  /// Handle a `/clear` event: the old session ended and a new one started in
  /// the same Claude process. Update the session's uuid to the new one.
  /// No terminal relaunch needed — `/clear` resets the conversation but the
  /// Claude process keeps running in the same terminal.
  pub(super) fn own_session_clear(
    &mut self,
    old_session_id: &str,
    new_session_id: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some((pi, sid)) = self.session_for_hook(old_session_id) else {
      eprintln!("hook: session-clear for unknown uuid {old_session_id}");
      return;
    };
    eprintln!("hook: session cleared, uuid {old_session_id} -> {new_session_id}");

    // Update TODO.md FIRST, and only adopt the new uuid in memory if that
    // succeeded. The two must not diverge: a session whose in-memory uuid has no
    // heading stops syncing its label, can't be disabled (both look the heading
    // up by uuid), and on the next start resumes whatever the file still says —
    // a wrong conversation, not merely a dead one.
    //
    // The heading is located by the OLD uuid, not by label: two headings can
    // share a label, and writing to the first would point it at this session's
    // conversation while leaving this one behind.
    let project = &mut self.projects[pi];
    let todo_view = project.todo_view.clone();
    let written = todo_view.update(cx, |tv, cx| {
      let Some(index) = tv.document().index_by_id(old_session_id) else { return false };
      tv.update_session_uuid_at(index, new_session_id, window, cx);
      tv.save(cx);
      true
    });
    if !written {
      eprintln!(
        "hook: session-clear could not find a heading for uuid {old_session_id}; \
         leaving the session on it so memory and TODO.md stay in step"
      );
      return;
    }
    let is_active = project.active_session == Some(sid);
    if let Some(session) = project.sessions.get_mut(&sid) {
      session.uuid = new_session_id.to_string();
      session.key = new_session_id.to_string();
    }
    // The TODO view highlights the active session by uuid, so the swap has to
    // be told to it too — otherwise the active heading's colouring goes out
    // until the next session switch.
    if is_active {
      todo_view.update(cx, |tv, cx| tv.set_active_key(Some(new_session_id), cx));
    }
  }
}
