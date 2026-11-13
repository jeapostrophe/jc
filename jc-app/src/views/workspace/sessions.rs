//! Claude panes as clients of Cira (Cira DESIGN 12, PLAN TMUX-01/02): Cira
//! owns each session's lifecycle, and jc asks it for every change
//! (`cira session new|mode|stop|rm|label`), attaches each Claude pane to the
//! job `cira status --json` names, and turns TODO.md marker edits and renames
//! into requests. Every `cira` and `claude` call runs on a thread of its own.

use super::Workspace;
use crate::views::session_state::{Heading, SessionId, SessionState};
use gpui::*;
use jc_core::background::{Poll, Settled};
use jc_core::cira::{CallError, Created, NewSession, Status, exit};
use jc_core::lifecycle::{self, LabelSync, MarkRequest, PaneAction, Records};
use jc_core::todo::{SessionKey, TodoSession};
use jc_terminal::{ChildEvent, TerminalView};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Cira as jc last read it.
#[derive(Default)]
pub(super) struct CiraView {
  /// The last `cira status --json` that answered, and when its read began.
  pub status: Option<Status>,
  pub read_at: Option<SystemTime>,
  /// Whether the last read reached Cira; `None` before the first.
  pub reachable: Option<bool>,
  /// What jc remembers per Cira id (`sessions.toml`); `None` until loaded.
  pub records: Option<Records>,
  /// Cira ids with a stop, rm or rename request in flight.
  pub in_flight: std::collections::HashSet<String>,
  /// When a rename Cira accepted was answered, per Cira id: a status read
  /// begun before then still has the old label, and is not a change on the
  /// page.
  pub renamed_at: std::collections::HashMap<String, SystemTime>,
}

impl Workspace {
  /// Run `work` on its own thread (it may block on `cira` for minutes), then
  /// `done` with its result on the main thread, if the workspace still exists.
  pub(super) fn run_blocking<T: Send + 'static>(
    &self,
    window: &mut Window,
    cx: &mut Context<Self>,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
  ) {
    let (tx, rx) = flume::bounded(1);
    std::thread::spawn(move || {
      let _ = tx.send(work());
    });
    cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| {
      if let Ok(value) = rx.recv_async().await {
        let _ = this.update_in(cx, |ws, window, cx| done(ws, value, window, cx));
      }
    })
    .detach();
  }

  pub(super) fn project_index(&self, path: &Path) -> Option<usize> {
    self.projects.iter().position(|p| p.path == path)
  }

  pub(super) fn session_mut(&mut self, path: &Path, sid: SessionId) -> Option<&mut SessionState> {
    let pi = self.project_index(path)?;
    self.projects[pi].sessions.get_mut(&sid)
  }

  /// Ask for a fresh `cira status --json` now.
  pub(super) fn kick_cira(&self) {
    let _ = self.cira_kick.send(());
  }

  fn cira_reachable(&self) -> bool {
    self.cira.reachable == Some(true)
  }

  /// Keep what jc remembers, on the saver thread.
  fn save_records(&self) {
    if let Some(records) = &self.cira.records {
      let _ = self.records_tx.send(records.clone());
    }
  }

  /// The record for Cira id `id`, created empty.
  fn record_mut(&mut self, id: &str) -> Option<&mut lifecycle::Record> {
    Some(self.cira.records.as_mut()?.sessions.entry(id.to_string()).or_default())
  }

  // ---------------------------------------------------------------------------
  // Each status read
  // ---------------------------------------------------------------------------

  /// A `cira status --json` read that began at `read_at` (`None`: Cira is
  /// unreachable): show it, then act on it.
  pub(super) fn on_cira(
    &mut self,
    read_at: SystemTime,
    status: Option<Status>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let mut changed = false;
    if self.cira.reachable != Some(status.is_some()) {
      self.cira.reachable = Some(status.is_some());
      eprintln!("cira: {}", if status.is_some() { "reachable" } else { "unreachable" });
      changed = true;
    }
    for project in &mut self.projects {
      changed |= project.apply_cira(status.as_ref());
    }
    if status.is_some() {
      self.cira.status = status;
    }
    self.cira.read_at = Some(read_at);
    self.retry_unmatched_hooks(window, cx);
    self.drive_sessions(read_at, window, cx);
    if self.cira_reachable() {
      self.reconcile_marks(window, cx);
      self.sync_labels(window, cx);
    }
    if changed {
      cx.notify();
    }
  }

  /// Bring every Claude pane in line with Cira: migrate a legacy heading,
  /// attach to the job Cira names, or say why not ([`lifecycle::pane_action`]).
  fn drive_sessions(&mut self, read_at: SystemTime, window: &mut Window, cx: &mut Context<Self>) {
    let reachable = self.cira_reachable();
    let now = SystemTime::now();
    let mut migrate = Vec::new();
    for project in &mut self.projects {
      for session in project.sessions.values_mut() {
        if session.asking || session.failed {
          continue;
        }
        if session.cira.is_none() {
          // A legacy heading: migrated once Cira answers. One still being
          // written (a new session) has no key yet.
          if !session.key.is_empty() {
            if reachable {
              migrate.push((project.path.clone(), session.id));
            } else {
              session
                .show_once("Cira is unreachable; this session moves to Cira once it answers", cx);
            }
          }
          continue;
        }
        if let Some((exited, delay)) = session.exited
          && session.bg.is_empty()
          && (exited > read_at || now < exited + delay)
        {
          continue; // a read from before the exit, or still backing off
        }
        let attached = (!session.bg.is_empty()).then_some(session.bg.as_str());
        match lifecycle::pane_action(
          session.cira_session.as_ref(),
          reachable,
          attached,
          session.attach_since,
        ) {
          PaneAction::Attach(short) => session.attach(&short, &self.claude, cx),
          PaneAction::Keep => {}
          PaneAction::Wait(why) | PaneAction::Hold(why) => session.show_once(&why, cx),
        }
      }
    }
    for (path, sid) in migrate {
      self.migrate(path, sid, window, cx);
    }
  }

  // ---------------------------------------------------------------------------
  // Asking Cira for a session
  // ---------------------------------------------------------------------------

  /// Move legacy heading `sid` (`> uuid=U`) to a session Cira runs: `cira
  /// session new --resume U --wait`, carrying `> dangerous`. A conversation
  /// with no transcript (it never took a prompt) starts fresh. Its heading is
  /// rewritten only once Cira has a job for it, so a failed start is asked
  /// again — which retries it — at the next jc start or adoption.
  fn migrate(
    &mut self,
    path: PathBuf,
    sid: SessionId,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(pi) = self.project_index(&path) else { return };
    let doc = self.projects[pi].todo_view.read(cx).document();
    let Some(session) = self.projects[pi].sessions.get(&sid) else { return };
    let dangerous = doc.session_by_id(&session.key).is_some_and(|h| h.dangerous);
    let req = NewSession {
      cwd: path.clone(),
      label: session.label.clone(),
      resume: Some(session.key.clone()),
      dangerous,
      wait: true,
    };
    self.request_session(
      path,
      sid,
      req,
      "moving this conversation to a session Cira runs…",
      window,
      cx,
    );
  }

  /// Ask Cira for `sid`'s session (`req`), then write its id into the
  /// heading: the unbound or legacy heading the session came from, else a
  /// new one.
  fn request_session(
    &mut self,
    path: PathBuf,
    sid: SessionId,
    req: NewSession,
    why: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(session) = self.session_mut(&path, sid) else { return };
    session.asking = true;
    session.failed = false;
    session.status(why, cx);
    let cira = self.cira_client.clone();
    let work = move || {
      let made = cira.new_session(&req);
      match made {
        // `--resume` of a conversation Claude Code has no transcript of.
        Err(CallError::Refused { code: exit::INVALID, .. }) if req.resume.is_some() => {
          cira.new_session(&NewSession { resume: None, wait: false, ..req })
        }
        made => made,
      }
    };
    self.run_blocking(window, cx, work, move |ws, made, window, cx| {
      ws.session_answered(&path, sid, made, window, cx)
    });
  }

  fn session_answered(
    &mut self,
    path: &Path,
    sid: SessionId,
    made: Result<Created, CallError>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(pi) = self.project_index(path) else { return };
    let project = &mut self.projects[pi];
    let is_active = project.active_session == Some(sid);
    let todo_view = project.todo_view.clone();
    let Some(session) = project.sessions.get_mut(&sid) else { return };
    session.asking = false;
    let created = match made {
      Ok(created) => created,
      Err(e) => {
        let job_failed = matches!(e, CallError::Refused { code: exit::ERROR, .. });
        session.failed = true;
        session.status(
          &if job_failed {
            format!("Cira could not start this session's job: {e}. Adopt it again to retry.")
          } else {
            format!("could not ask Cira for a session: {e}")
          },
          cx,
        );
        cx.notify();
        return;
      }
    };
    let id = created.session;
    let doc = todo_view.read(cx).document();
    let index = match (&session.unbound_key, session.key.is_empty()) {
      (Some(key), _) => doc.index_of(key),
      (None, false) => doc.index_by_id(&session.key),
      (None, true) => None,
    };
    match index {
      Some(index) => todo_view.update(cx, |tv, cx| tv.bind_cira_at(index, &id, window, cx)),
      None if session.key.is_empty() && session.unbound_key.is_none() => {
        let label = session.label.clone();
        todo_view.update(cx, |tv, cx| tv.insert_session_heading(&label, &id, window, cx));
      }
      None => {
        eprintln!("cira: the heading for {} moved; session {id} is not recorded", session.key)
      }
    }
    session.unbound_key = None;
    session.adopting = None;
    session.attach_since = Some(SystemTime::now());
    session.bind(&id, cx);
    let label = session.label.clone();
    if is_active {
      todo_view.update(cx, |tv, cx| tv.set_active_key(Some(&id), cx));
    }
    if let Some(record) = self.record_mut(&id) {
      record.mark = Some(jc_core::todo::Mark::Active);
      record.label = Some(label);
    }
    self.save_records();
    self.kick_cira();
    cx.notify();
  }

  /// Add a session whose pane waits for Cira, and switch to it.
  fn add_pending_session(
    &mut self,
    pi: usize,
    heading: Heading,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> SessionId {
    let project_path = self.projects[pi].path.clone();
    let palette = super::palette_from_window(window);
    let project = &mut self.projects[pi];
    let id = project.next_session_id;
    project.next_session_id += 1;
    let session = SessionState::create(id, heading, &project_path, &palette, window, cx);
    project.sessions.insert(id, session);
    self.observe_claude_panes(pi, window, cx);
    self.switch_to_session(pi, Some(id), window, cx);
    id
  }

  /// A brand-new session: Cira mints it and starts its job; its heading is
  /// written as soon as Cira answers with the id.
  pub(super) fn create_new_session(
    &mut self,
    pi: usize,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    // Cosmetic only — nothing addresses a session by label (see
    // `jc_core::todo::SessionKey`); this just stops the picker showing
    // several identical rows.
    let label =
      jc_core::todo::unique_label(self.projects[pi].todo_view.read(cx).document(), "New Session");
    self.start_unrecorded(pi, label, None, window, cx);
  }

  /// Continue transcript `uuid`, which no heading names, as a session Cira
  /// runs under a new heading labelled from `summary`.
  pub(super) fn adopt_transcript(
    &mut self,
    pi: usize,
    uuid: String,
    summary: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    // A second confirm while the first is asked would make a second session
    // on the same conversation (Cira answers the same one, but jc would write
    // two headings).
    let in_flight = |s: &SessionState| s.adopting.as_deref() == Some(uuid.as_str());
    if self.projects[pi].sessions.values().any(in_flight) {
      crate::notify::beep();
      return;
    }
    // The summary is the transcript's first user message, so two sessions that
    // opened with the same prompt would otherwise get the same display name.
    let label =
      jc_core::todo::unique_label(self.projects[pi].todo_view.read(cx).document(), summary);
    self.start_unrecorded(pi, label, Some(uuid), window, cx);
  }

  fn start_unrecorded(
    &mut self,
    pi: usize,
    label: String,
    resume: Option<String>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let heading = Heading { key: String::new(), label: label.clone(), cira: None };
    let sid = self.add_pending_session(pi, heading, window, cx);
    let path = self.projects[pi].path.clone();
    if let Some(session) = self.projects[pi].sessions.get_mut(&sid) {
      session.adopting = resume.clone();
    }
    let wait = resume.is_some();
    let req = NewSession { cwd: path.clone(), label, resume, dangerous: false, wait };
    self.request_session(path, sid, req, "asking Cira for a session…", window, cx);
  }

  /// Adopt a TODO.md heading that has no pane: attach it to its Cira session,
  /// migrating a legacy `> uuid=` heading; give an unbound one (an older jc's
  /// blank `> uuid=`) a fresh session, which is something Jay chose to do,
  /// never done at startup. The heading is re-resolved against the *live*
  /// document, so a key taken from a picker snapshot cannot write to a
  /// heading that has moved since. Adopting is Jay asking, so it attaches
  /// even a session stopped through Cira.
  pub(super) fn adopt_session(
    &mut self,
    pi: usize,
    key: &SessionKey,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let todo_view = self.projects[pi].todo_view.clone();
    let resolved = todo_view.read(cx).document().session_of(key).cloned();

    // Refuse to launch a session whose heading isn't in the file. It could
    // never be label-synced or disabled afterwards, and the heading would stay
    // adoptable — so a second press would start a second session for it.
    let Some(heading) = resolved else {
      eprintln!("adopt: no TODO heading for {key:?}; not launching");
      // The picker already dropped `pre_picker_focus` on the assumption that
      // this call would focus something. It won't, so do it here or the next
      // keystroke goes nowhere.
      self.panes[self.active_pane_index].read(cx).focus_content(window);
      return;
    };

    // A heading already being asked for gets one session; one whose request
    // failed makes way for a new try.
    let project = &self.projects[pi];
    if project.sessions.values().any(|s| s.unbound_key.as_ref() == Some(key) && !s.failed) {
      crate::notify::beep();
      return;
    }
    let failed: Vec<SessionId> = project
      .sessions
      .values()
      .filter(|s| {
        s.failed
          && (s.unbound_key.as_ref() == Some(key)
            || (!heading.id.is_empty() && s.key == heading.id))
      })
      .map(|s| s.id)
      .collect();
    for sid in failed {
      self.remove_running_session(pi, sid, window, cx);
    }

    // Clear `[D]` if the heading was dormant (attaching resumes the stopped
    // session). Its record moves back to active at the next reconcile.
    if heading.is_disabled() {
      todo_view.update(cx, |tv, cx| {
        tv.toggle_session_disabled(key, window, cx);
        tv.save(cx);
      });
    }

    let sid = self.add_pending_session(pi, Heading::of(&heading), window, cx);
    let path = self.projects[pi].path.clone();
    let Some(session) = self.projects[pi].sessions.get_mut(&sid) else { return };
    session.attach_since = Some(SystemTime::now());
    if heading.id.is_empty() {
      session.unbound_key = Some(key.clone());
      let req = NewSession {
        cwd: path.clone(),
        label: heading.label.clone(),
        resume: None,
        dangerous: false,
        wait: false,
      };
      self.request_session(path, sid, req, "asking Cira for a session…", window, cx);
    }
    self.kick_cira();
  }

  /// Attach the active session's pane again: Jay asked, which overrides a
  /// stop made through Cira and retries a failed migration.
  pub(super) fn attach_active_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let pi = self.active_project_index;
    let Some(session) = self.projects[pi].active_session_mut() else { return };
    session.attach_since = Some(SystemTime::now());
    session.failed = false;
    session.exited = None;
    session.shown = None;
    self.kick_cira();
    self.panes[self.active_pane_index].read(cx).focus_content(window);
  }

  /// Ask Cira to run the active session in the other mode (Cira Q-155): a
  /// fork once it is idle, which the pane follows to the new job.
  pub(super) fn toggle_active_dangerous(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let pi = self.active_project_index;
    self.panes[self.active_pane_index].read(cx).focus_content(window);
    let path = self.projects[pi].path.clone();
    let Some(session) = self.projects[pi].active_session() else { return };
    let (Some(id), Some(from)) = (session.cira.clone(), session.dangerous()) else {
      crate::notify::beep();
      return;
    };
    let sid = session.id;
    let to = !from;
    let cira = self.cira_client.clone();
    let id_for_work = id.clone();
    self.run_blocking(
      window,
      cx,
      move || cira.set_mode(&id_for_work, to, from),
      move |ws, result, _window, cx| {
        let Some(session) = ws.session_mut(&path, sid) else { return };
        let mode = if to { "dangerous" } else { "normal" };
        match result {
          Ok(()) => session.status(&format!("asked Cira to make this session {mode}"), cx),
          Err(CallError::Refused { code: exit::CONFLICT, message, .. }) => {
            session.status(&format!("its mode changed elsewhere: {}", message.trim()), cx)
          }
          Err(e) => session.status(&format!("could not make this session {mode}: {e}"), cx),
        }
        ws.kick_cira();
      },
    );
  }

  // ---------------------------------------------------------------------------
  // The Claude pane's attach client
  // ---------------------------------------------------------------------------

  /// Watch every Claude pane in project `pi` that is not watched yet, so an
  /// attach client's exit is noticed (see [`Self::on_claude_child`]).
  pub(super) fn observe_claude_panes(
    &mut self,
    pi: usize,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let path = self.projects[pi].path.clone();
    let unwatched: Vec<(SessionId, Entity<TerminalView>)> = self.projects[pi]
      .sessions
      .iter()
      .filter(|(_, s)| s.claude_child_events.is_none())
      .map(|(&sid, s)| (sid, s.claude_terminal.clone()))
      .collect();
    for (sid, terminal) in unwatched {
      let path = path.clone();
      let sub = cx.subscribe_in(&terminal, window, move |ws, _, event: &ChildEvent, window, cx| {
        ws.on_claude_child(&path, sid, *event, window, cx);
      });
      if let Some(session) = self.projects[pi].sessions.get_mut(&sid) {
        session.claude_child_events = Some(sub);
      }
    }
  }

  /// An attach client started or exited. Either way the session is not known
  /// to be reachable until `claude agents --json` or a hook says so. After an
  /// exit (Ctrl+Z, Esc from the agents view, Cira stopping or forking the
  /// job) the pane attaches again only once a status read made after the
  /// exit says to what.
  fn on_claude_child(
    &mut self,
    path: &Path,
    sid: SessionId,
    event: ChildEvent,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(session) = self.session_mut(path, sid) else { return };
    session.live = false;
    match event {
      ChildEvent::Spawned => self.watch_live(path.to_path_buf(), sid, window, cx),
      ChildEvent::Exited { lived, .. } => {
        session.bg.clear();
        session.shown = None;
        let previous = session.exited.map(|(_, delay)| delay);
        session.exited = Some((SystemTime::now(), lifecycle::attach_delay(previous, lived)));
        self.kick_cira();
      }
    }
  }

  /// Poll `claude agents --json` until the job behind `sid`'s pane runs, then
  /// mark it live, learn its uuid, and send what waited for it. Read-only:
  /// whether the job exists at all is Cira's to say.
  fn watch_live(
    &mut self,
    path: PathBuf,
    sid: SessionId,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(session) = self.session_mut(&path, sid) else { return };
    if session.bg.is_empty() || session.watching_live {
      return;
    }
    session.watching_live = true;
    let short = session.bg.clone();
    let claude = self.claude.clone();
    self.run_blocking(
      window,
      cx,
      move || claude.wait_settled(&short, Poll::START).map(|a| (short, a)),
      move |ws, result, _window, _cx| {
        let Some(session) = ws.session_mut(&path, sid) else { return };
        session.watching_live = false;
        match result {
          Ok((short, Settled::Live(agent))) if session.bg == short => {
            session.live = true;
            if let Some(uuid) = agent.session_id {
              session.uuid = uuid;
            }
          }
          Ok(_) => {}
          Err(e) => eprintln!("claude agents: {e:#}"),
        }
      },
    );
  }

  // ---------------------------------------------------------------------------
  // TODO.md markers and labels
  // ---------------------------------------------------------------------------

  /// Toggle the `[D]` marker on the heading `key` names. Disabling a running
  /// session detaches its pane; the marker change then becomes `cira session
  /// stop` ([`Self::reconcile_marks`]). Never addressed by label: with two
  /// same-labelled headings a label-keyed toggle marks the wrong one and
  /// stops a live session (see `SessionKey`).
  pub(super) fn toggle_session_disabled(
    &mut self,
    pi: usize,
    key: &SessionKey,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let project = &mut self.projects[pi];
    let todo_view = project.todo_view.clone();
    // Check if the session is currently adopted (running). An unbound heading
    // never is, so this is `None` for an `Unbound` key.
    let adopted = match key {
      SessionKey::Id(id) => project.session_by_key(id).map(|(sid, s)| (sid, s.asking)),
      SessionKey::Unbound { .. } => None,
    };
    // A request in flight must finish writing the heading's id first.
    if adopted.is_some_and(|(_, asking)| asking) {
      crate::notify::beep();
      self.panes[self.active_pane_index].read(cx).focus_content(window);
      return;
    }

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
    if is_now_disabled && let Some((sid, _)) = adopted {
      switched = self.remove_running_session(pi, sid, window, cx);
    }
    if !switched {
      self.panes[self.active_pane_index].read(cx).focus_content(window);
    }
    self.reconcile_marks(window, cx);
    cx.notify();
  }

  /// Turn each change of a heading's marker into one request to Cira (Cira
  /// DESIGN 12): `[D]` → `cira session stop`, `[DELETED]` → `cira session
  /// rm`, remembered per Cira id so an edit made while jc was closed applies
  /// at its next start. A heading that is `[D]` or `[DELETED]` has no pane.
  /// A TODO with unsaved edits waits: a half-typed marker is not a request.
  fn reconcile_marks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let mut detach = Vec::new();
    let mut requests = Vec::new();
    let mut settled = Vec::new();
    for (pi, project) in self.projects.iter().enumerate() {
      let tv = project.todo_view.read(cx);
      if tv.is_dirty(cx) {
        continue;
      }
      let doc = tv.document();
      let deleted = |key: &str| doc.deleted.iter().any(|d| d.id == key);
      for session in project.sessions.values() {
        if session.key.is_empty() || session.asking {
          continue;
        }
        let gone = match doc.session_by_id(&session.key) {
          Some(h) => h.is_disabled(),
          None => deleted(&session.key),
        };
        if gone {
          detach.push((pi, session.id));
        }
      }
      let Some(records) = &self.cira.records else { continue };
      for (id, mark) in doc.marks() {
        if self.cira.in_flight.contains(id) {
          continue;
        }
        let last = records.sessions.get(id).and_then(|r| r.mark);
        match lifecycle::mark_request(last, mark) {
          Some(request) => requests.push((id.to_string(), mark, request)),
          None if last != Some(mark) => settled.push((id.to_string(), mark)),
          None => {}
        }
      }
    }
    for (pi, sid) in detach {
      self.remove_running_session(pi, sid, window, cx);
    }
    if !settled.is_empty() {
      for (id, mark) in settled {
        if let Some(record) = self.record_mut(&id) {
          record.mark = Some(mark);
        }
      }
      self.save_records();
    }
    for (id, mark, request) in requests {
      self.cira.in_flight.insert(id.clone());
      let cira = self.cira_client.clone();
      let id_for_work = id.clone();
      let work = move || match request {
        MarkRequest::Stop => cira.stop(&id_for_work),
        MarkRequest::Remove => cira.remove(&id_for_work),
      };
      self.run_blocking(window, cx, work, move |ws, result, _window, _cx| {
        ws.cira.in_flight.remove(&id);
        match result {
          // Unreachable: asked again at the next read.
          Err(CallError::Unreachable) => return,
          Ok(()) => eprintln!("cira: {request:?} {id}"),
          // Refused (a removed session asked to stop, one Cira does not
          // list): it is answered, and asking again would not change that.
          Err(e) => eprintln!("cira: {request:?} {id}: {e}"),
        }
        if let Some(record) = ws.record_mut(&id) {
          record.mark = Some(mark);
        }
        ws.save_records();
        ws.kick_cira();
      });
    }
  }

  /// Keep each heading's label and Cira's in step (Q-159): a heading renamed
  /// in TODO.md is sent with `cira session label`; a label Cira changed (the
  /// page) is written into the heading. Cira's wins when both moved.
  fn sync_labels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(status) = &self.cira.status else { return };
    let Some(records) = &self.cira.records else { return };
    let read_at = self.cira.read_at.unwrap_or(SystemTime::UNIX_EPOCH);
    let mut sends = Vec::new();
    let mut writes = Vec::new();
    let mut agreed = Vec::new();
    for (pi, project) in self.projects.iter().enumerate() {
      let tv = project.todo_view.read(cx);
      if tv.is_dirty(cx) {
        continue;
      }
      for (index, heading) in tv.document().sessions.iter().enumerate() {
        let Some(id) = heading.cira() else { continue };
        let Some(listed) = status.session(id) else { continue };
        let stale = self.cira.renamed_at.get(id).is_some_and(|at| read_at <= *at);
        if self.cira.in_flight.contains(id) || stale {
          continue;
        }
        let last = records.sessions.get(id).and_then(|r| r.label.as_deref());
        match lifecycle::label_sync(&heading.label, listed.label.as_deref(), last) {
          LabelSync::Agreed if last != Some(heading.label.as_str()) => {
            agreed.push((id.to_string(), heading.label.clone()))
          }
          LabelSync::Agreed => {}
          LabelSync::Send => sends.push((id.to_string(), heading.label.clone())),
          LabelSync::Write(label) => writes.push((pi, index, id.to_string(), label)),
        }
      }
    }
    let record = !agreed.is_empty() || !writes.is_empty();
    for (id, label) in agreed {
      if let Some(r) = self.record_mut(&id) {
        r.label = Some(label);
      }
    }
    for (pi, index, id, label) in writes {
      let tv = self.projects[pi].todo_view.clone();
      tv.update(cx, |tv, cx| tv.rename_heading_at(index, &label, window, cx));
      if let Some(r) = self.record_mut(&id) {
        r.label = Some(label);
      }
    }
    if record {
      self.save_records();
    }
    for (id, label) in sends {
      self.cira.in_flight.insert(id.clone());
      let cira = self.cira_client.clone();
      let (id_w, label_w) = (id.clone(), label.clone());
      self.run_blocking(
        window,
        cx,
        move || cira.rename(&id_w, &label_w),
        move |ws, result, _window, _cx| {
          ws.cira.in_flight.remove(&id);
          match result {
            Err(CallError::Unreachable) => return,
            Ok(()) => {}
            Err(e) => eprintln!("cira: rename {id}: {e}"),
          }
          ws.cira.renamed_at.insert(id.clone(), SystemTime::now());
          if let Some(r) = ws.record_mut(&id) {
            r.label = Some(label);
          }
          ws.save_records();
          ws.kick_cira();
        },
      );
    }
  }
}
