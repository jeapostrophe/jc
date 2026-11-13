//! jcc's Claude panes, as clients of Cira (Cira DESIGN 12, PLAN TMUX-01/02,
//! TMUX-07; none of this runs in jc, whose own lifecycle is `own.rs`): Cira
//! owns each session's lifecycle, and jc asks it for every change
//! (`cira session new|mode|stop|rm|label`), attaches each Claude pane to the
//! job `cira status --json` names, and turns TODO.md marker edits and renames
//! into requests. Each session's shell pane attaches to the shell Cira keeps
//! for it (`cira session shell`), else runs jc's own, marked not shared.
//! Every `cira` and `claude` call runs on a thread of its own.

use super::Workspace;
use crate::views::project_state::ProjectState;
use crate::views::session_state::{Heading, SessionId, SessionState, Start};
use gpui::*;
use jc_core::background::{Poll, Settled};
use jc_core::cira::{At, CallError, Created, NewSession, Status, exit};
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
  /// What jc remembers per Cira id (`sessions.toml`); `None` until loaded, and
  /// for good when it could not be read or kept aside (nothing is acted on).
  pub records: Option<Records>,
  /// Cira ids with a stop, rm or rename request in flight.
  pub in_flight: std::collections::HashSet<String>,
  /// When a rename Cira accepted was answered, per Cira id: a status read
  /// begun before then still has the old label, and is not a change on the
  /// page.
  pub renamed_at: std::collections::HashMap<String, SystemTime>,
  /// jc projects jcc has asked Cira to register (`cira project add`), so
  /// each is asked once: a project gives jcc's todo.md its place.
  pub registering: std::collections::HashSet<PathBuf>,
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

  /// jcc, at start: follow every pane's child, and load `sessions.toml`.
  pub(super) fn start_cira_client(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    for pi in 0..self.projects.len() {
      self.observe_panes(pi, window, cx);
    }
    self.run_blocking(
      window,
      cx,
      jc_core::config::load_sessions,
      // With no records a marker asks nothing (`lifecycle::mark_request`),
      // so starting empty sends no stop or rm once a bad file is kept aside
      // (a marker or rename made while jc was closed is not acted on, and
      // Cira's label wins: `lifecycle::label_sync`). When it
      // could not be read or moved, `records` stays `None`: no marker or
      // label is acted on and nothing is saved over the file.
      |ws, loaded, _window, _cx| match loaded {
        Ok((records, kept)) => {
          if let Some(kept) = kept {
            eprintln!("sessions.toml did not parse; kept it as {}", kept.display());
          }
          ws.cira.records = Some(records);
          ws.kick_cira();
        }
        Err(e) => eprintln!("sessions.toml: {e:#}; acting on no markers or labels"),
      },
    );
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
    if let Some(kick) = &self.cira_kick {
      let _ = kick.send(());
    }
  }

  fn cira_reachable(&self) -> bool {
    self.cira.reachable == Some(true)
  }

  /// Keep what jc remembers, on the saver thread.
  fn save_records(&self) {
    if let (Some(records), Some(tx)) = (&self.cira.records, &self.records_tx) {
      let _ = tx.send(records.clone());
    }
  }

  /// Where a request about Cira session `id` of project `pi` is made from.
  fn at(&self, pi: usize, id: &str) -> jc_core::cira::At {
    jc_core::cira::At {
      project: self.cira.status.as_ref().and_then(|s| s.project_of(id)).map(str::to_string),
      dir: self.projects[pi].path.clone(),
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
    if let Some(status) = &status {
      changed |= self.open_todos(status, window, cx);
    }
    if status.is_some() {
      self.cira.status = status;
    }
    self.cira.read_at = Some(read_at);
    self.retry_unmatched_hooks(window, cx);
    self.drive_sessions(read_at, window, cx);
    self.drive_shells(window, cx);
    if self.cira_reachable() {
      self.reconcile_marks(window, cx);
      self.sync_labels(window, cx);
    }
    if changed {
      cx.notify();
    }
  }

  /// Open jcc's todo.md for each project a read names and whose TODO is not
  /// open yet, and start its sessions. A project no Cira project holds is
  /// registered under its directory's name, as `cira session new` would, so
  /// its TODO opens at a later read. Returns whether one opened.
  fn open_todos(&mut self, status: &Status, window: &mut Window, cx: &mut Context<Self>) -> bool {
    let palette = super::palette_from_window(window);
    let mut opened = false;
    for pi in 0..self.projects.len() {
      if self.projects[pi].todo_open(cx) {
        continue;
      }
      let path = self.projects[pi].path.clone();
      if let Some(todo) = self.flavor.todo_path(&path, Some(status)) {
        self.projects[pi].open_todo(todo, &palette, window, cx);
        opened = true;
        continue;
      }
      let Some(cira) = self.cira_client.clone() else { continue };
      if !self.cira.registering.insert(path.clone()) {
        continue;
      }
      let name = self.projects[pi].name.clone();
      self.run_blocking(
        window,
        cx,
        move || cira.add_project(&name, &path).map_err(|e| (name, path, e)),
        |ws, added, _window, _cx| match added {
          Ok(()) => ws.kick_cira(),
          Err((name, path, e)) => {
            eprintln!("cira project add {name} --path {}: {e}", path.display())
          }
        },
      );
    }
    if opened {
      self.refresh_breadcrumb_observers(cx);
    }
    opened
  }

  /// Bring every Claude pane in line with Cira: migrate a legacy heading,
  /// attach to the job Cira names, or say why not ([`lifecycle::pane_action`]).
  fn drive_sessions(&mut self, read_at: SystemTime, window: &mut Window, cx: &mut Context<Self>) {
    let reachable = self.cira_reachable();
    let now = SystemTime::now();
    let mut migrate = Vec::new();
    let mut lost = Vec::new();
    for project in &mut self.projects {
      for session in project.sessions.values_mut() {
        if session.asking || session.failed {
          continue;
        }
        if let Some(at) = session.lost {
          if reachable && read_at > at {
            lost.push((project.path.clone(), session.id));
          }
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
        if session.bg.is_empty()
          && (session.exited.exited_after(read_at) || session.exited.waiting(now))
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
    for (path, sid) in lost {
      self.find_lost(&path, sid, window, cx);
    }
  }

  /// Ask `cira session shell` for every shell pane that runs nothing and is
  /// past its pause ([`lifecycle::ShellPane`]), and run what the answer says.
  /// Asked again at each attach, so a shell Jay ended with `exit` is made
  /// again. Only a pane that has been laid out asks: one never shown would
  /// attach at jc's default size, and attached clients share the latest
  /// one's size (Cira's tmux.conf), the page's included.
  fn drive_shells(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let now = SystemTime::now();
    let mut ask = Vec::new();
    for (pi, project) in self.projects.iter_mut().enumerate() {
      for session in project.sessions.values_mut() {
        let Some(id) = &session.cira else { continue };
        if session.general_terminal.read(cx).is_sized() && session.shell.ask(id, now) {
          ask.push((pi, project.path.clone(), session.id, id.clone()));
        }
      }
    }
    for (pi, path, sid, id) in ask {
      let Some(cira) = self.cira_client.clone() else { continue };
      let at = self.at(pi, &id);
      let asked = id.clone();
      self.run_blocking(
        window,
        cx,
        move || cira.session_shell(&at, &asked),
        move |ws, answer, _window, cx| {
          let removed = ws
            .cira
            .status
            .as_ref()
            .and_then(|s| s.session(&id))
            .is_some_and(|s| s.removed_at.is_some());
          let Some(session) = ws.session_mut(&path, sid) else { return };
          let Some(child) = session.shell.answered(&id, answer, removed, SystemTime::now()) else {
            return;
          };
          session.run_shell(child, cx);
          if session.shell_not_shared() {
            cx.notify(); // the header says "not shared"
          }
        },
      );
    }
  }

  /// `cira session new` for `sid` went unanswered, and may have made the
  /// session anyway: take the one Cira lists under its label that no heading
  /// names, rather than asking again and making a second.
  fn find_lost(
    &mut self,
    path: &Path,
    sid: SessionId,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(pi) = self.project_index(path) else { return };
    let project = &self.projects[pi];
    let Some(session) = project.sessions.get(&sid) else { return };
    let doc = project.todo_view.read(cx).document();
    let named =
      |id: &str| doc.session_by_id(id).is_some() || doc.deleted.iter().any(|d| d.id == id);
    let found = self
      .cira
      .status
      .as_ref()
      .and_then(|s| s.project_for(path))
      .and_then(|p| lifecycle::lost_session(p, &session.label, named))
      .map(|s| s.id.clone());
    let made = found.map(|session| Created { session, status: None }).ok_or_else(|| {
      CallError::Malformed("Cira did not answer and lists no session made for this".into())
    });
    if let Some(session) = self.session_mut(path, sid) {
      session.lost = None;
    }
    self.session_answered(path, sid, made, false, window, cx);
  }

  // ---------------------------------------------------------------------------
  // Asking Cira for a session
  // ---------------------------------------------------------------------------

  /// Move legacy heading `sid` (`> uuid=U`, a block Jay cut from jc's TODO.md
  /// and pasted into jcc's todo.md) to a session Cira runs: `cira session new --resume U
  /// --wait`, carrying `> dangerous`. A conversation with no transcript (it
  /// never took a prompt) starts fresh. Its heading is rewritten only once
  /// Cira has a job for it, so a failed start is asked again — which retries
  /// it — at the next jcc start or adoption. Not while a `claude` process
  /// still runs U in the foreground (jc's pane, most likely): resuming it in
  /// Cira too would run one conversation twice, so the pane says so and the
  /// next status read asks again.
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
    let uuid = session.key.clone();
    if let Some(session) = self.session_mut(&path, sid) {
      session.asking = true; // nothing else is asked for it until this answers
    }
    self.run_blocking(
      window,
      cx,
      move || jc_core::claude::conversation_running(&uuid),
      move |ws, running, window, cx| {
        let Some(session) = ws.session_mut(&path, sid) else { return };
        session.asking = false;
        if running {
          session.show_once(
            "a `claude` process still runs this conversation (jc?); end it there first. It \
             moves to Cira once none does.",
            cx,
          );
          return;
        }
        ws.request_session(
          path,
          sid,
          req,
          "moving this conversation to a session Cira runs…",
          window,
          cx,
        );
      },
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
    let Some(cira) = self.cira_client.clone() else { return };
    let Some(session) = self.session_mut(&path, sid) else { return };
    session.asking = true;
    session.failed = false;
    session.status(why, cx);
    let cwd = path.clone();
    let work = move || {
      let made = cira.new_session(&req);
      match (made, &req.resume) {
        // Cira refuses a `--resume` it finds no transcript for (exit 5). Its
        // 5 has other causes too, and Cira may not find Claude Code's config:
        // only a conversation jc cannot find either starts fresh, since
        // binding a fresh session drops the heading's only link to it.
        (Err(CallError::Refused { code: exit::INVALID, .. }), Some(uuid))
          if ProjectState::session_dirs(&cwd)
            .is_some_and(|dirs| !jc_core::claude::transcript_in(&dirs, uuid)) =>
        {
          (cira.new_session(&NewSession { resume: None, ..req }), true)
        }
        (made, resume) => (made, resume.is_none()),
      }
    };
    self.run_blocking(window, cx, work, move |ws, (made, fresh), window, cx| {
      ws.session_answered(&path, sid, made, fresh, window, cx)
    });
  }

  /// Cira answered `sid`'s `cira session new` (`fresh`: one without
  /// `--resume`, which is not idempotent).
  fn session_answered(
    &mut self,
    path: &Path,
    sid: SessionId,
    made: Result<Created, CallError>,
    fresh: bool,
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
      Err(CallError::NoAnswer) if fresh => {
        // Asking again could make a second session: look for this one first.
        session.lost = Some(SystemTime::now());
        session.status("Cira did not answer; looking for the session it may have made…", cx);
        return;
      }
      Err(e) => {
        let why = match &e {
          CallError::Refused { code: exit::ERROR, stdout, .. } => {
            let failed = serde_json::from_slice::<Created>(stdout).ok().and_then(|c| c.status);
            let error = failed.as_ref().and_then(|s| s.op_error()).map(str::to_string);
            format!(
              "Cira could not start this session's job: {}. Adopt it again to retry.",
              error.unwrap_or_else(|| e.to_string())
            )
          }
          e => format!("could not ask Cira for a session: {e}"),
        };
        session.failed = true;
        session.adopting = None;
        session.status(&why, cx);
        cx.notify();
        return;
      }
    };
    let id = created.session;
    let doc = todo_view.read(cx).document();
    let heading = match (&session.unbound_key, session.key.is_empty()) {
      (Some(key), _) => doc.index_of(key).map(Some),
      (None, false) => doc.index_by_id(&session.key).map(Some),
      (None, true) => Some(None),
    };
    match heading {
      Some(Some(index)) => todo_view.update(cx, |tv, cx| tv.bind_cira_at(index, &id, window, cx)),
      Some(None) => {
        let label = session.label.clone();
        todo_view.update(cx, |tv, cx| tv.insert_session_heading(&label, &id, window, cx));
      }
      None => {
        // Its heading went while Cira was asked (deleted, or its id line
        // edited): nothing names the new session. One marked `[DELETED]`
        // is removed in Cira too; any other is left for the page.
        let deleted = doc.deleted.iter().any(|d| d.id == session.key);
        eprintln!("cira: the heading for {} went; session {id} is not recorded", session.key);
        self.remove_running_session(pi, sid, window, cx);
        if deleted {
          self.ask_cira(pi, id, |cira, at, id| cira.remove(at, id), |_, _| {}, window, cx);
        }
        return;
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
    let session =
      SessionState::create(id, heading, Start::Cira, &project_path, &palette, window, cx);
    project.sessions.insert(id, session);
    self.observe_panes(pi, window, cx);
    self.switch_to_session(pi, Some(id), window, cx);
    id
  }

  /// A brand-new session: Cira mints it and starts its job; its heading is
  /// written as soon as Cira answers with the id.
  pub(super) fn cira_new_session(
    &mut self,
    pi: usize,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    // Its heading goes into jcc's todo.md, which opens once Cira names the
    // project: until then there is nowhere to write it.
    if !self.projects[pi].todo_open(cx) {
      crate::notify::beep();
      return;
    }
    // Cosmetic only — nothing addresses a session by label (see
    // `jc_core::todo::SessionKey`); this just stops the picker showing
    // several identical rows.
    let label =
      jc_core::todo::unique_label(self.projects[pi].todo_view.read(cx).document(), "New Session");
    self.start_unrecorded(pi, label, None, window, cx);
  }

  /// Continue transcript `uuid`, which no heading names, as a session Cira
  /// runs under a new heading labelled from `summary`.
  pub(super) fn cira_adopt_transcript(
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
    if !self.projects[pi].todo_open(cx) || self.projects[pi].sessions.values().any(in_flight) {
      crate::notify::beep();
      return;
    }
    // The summary is the transcript's first user message, so two sessions that
    // opened with the same prompt would otherwise get the same display name.
    let label =
      jc_core::todo::unique_label(self.projects[pi].todo_view.read(cx).document(), summary);
    // jc's headings are in a file jcc never reads, so a conversation jc runs
    // is not known to be taken until its processes are looked at: resuming
    // it in Cira too would run it twice.
    let path = self.projects[pi].path.clone();
    let check = uuid.clone();
    self.run_blocking(
      window,
      cx,
      move || jc_core::claude::conversation_running(&check),
      move |ws, running, window, cx| {
        let Some(pi) = ws.project_index(&path) else { return };
        if running {
          eprintln!("adopt: a `claude` process still runs {uuid} (jc?); not resuming it in Cira");
          crate::notify::beep();
          return;
        }
        ws.start_unrecorded(pi, label, Some(uuid), window, cx);
      },
    );
  }

  fn start_unrecorded(
    &mut self,
    pi: usize,
    label: String,
    resume: Option<String>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    // A failed start with no heading has nothing to retry: make way.
    let dead: Vec<SessionId> = self.projects[pi]
      .sessions
      .values()
      .filter(|s| s.failed && s.key.is_empty() && s.unbound_key.is_none())
      .map(|s| s.id)
      .collect();
    for sid in dead {
      self.remove_running_session(pi, sid, window, cx);
    }
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
  pub(super) fn cira_adopt_session(
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
        tv.save(window, cx);
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
    session.exited.clear();
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
    let Some(cira) = self.cira_client.clone() else { return };
    let at = self.at(pi, &id);
    self.run_blocking(
      window,
      cx,
      move || cira.set_mode(&at, &id, to, from),
      move |ws, result, _window, cx| {
        let Some(session) = ws.session_mut(&path, sid) else { return };
        let mode = if to { "dangerous" } else { "auto" };
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

  /// Watch every Claude and shell pane in project `pi` that is not watched
  /// yet, so an attach client's exit is noticed ([`Self::on_claude_child`],
  /// [`Self::on_shell_child`]).
  pub(super) fn observe_panes(&mut self, pi: usize, window: &mut Window, cx: &mut Context<Self>) {
    self.observe(
      pi,
      |s| &s.claude_terminal,
      |s| &mut s.claude_child_events,
      Self::on_claude_child,
      window,
      cx,
    );
    self.observe(
      pi,
      |s| &s.general_terminal,
      |s| &mut s.shell_child_events,
      Self::on_shell_child,
      window,
      cx,
    );
  }

  /// Send the child events of each session's `terminal` in project `pi` that
  /// has no subscription in `events` yet to `on`.
  fn observe(
    &mut self,
    pi: usize,
    terminal: fn(&SessionState) -> &Entity<TerminalView>,
    events: fn(&mut SessionState) -> &mut Option<Subscription>,
    on: fn(&mut Self, &Path, SessionId, &ChildEvent, &mut Window, &mut Context<Self>),
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let path = self.projects[pi].path.clone();
    for session in self.projects[pi].sessions.values_mut() {
      if events(session).is_some() {
        continue;
      }
      let (path, sid) = (path.clone(), session.id);
      let sub = cx.subscribe_in(terminal(session), window, move |ws, _, event, window, cx| {
        on(ws, &path, sid, event, window, cx)
      });
      *events(session) = Some(sub);
    }
  }

  /// The shell pane's child exited: Jay ended the shell (`exit`), the tmux
  /// client failed or lost its server, or jc's own shell ended. It asks Cira
  /// again once its pause is over ([`Self::drive_shells`]), or runs jc's own
  /// shell after the shared client kept failing ([`lifecycle::ShellPane`]).
  fn on_shell_child(
    &mut self,
    path: &Path,
    sid: SessionId,
    event: &ChildEvent,
    _window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let ChildEvent::Exited { code, lived, said } = event else { return };
    let Some(session) = self.session_mut(path, sid) else { return };
    let was_own = session.shell_not_shared();
    let child = session.shell.exited(*code, *lived, SystemTime::now(), said.as_deref());
    if let Some(child) = child {
      session.run_shell(child, cx);
    }
    if was_own != session.shell_not_shared() {
      cx.notify(); // the header's "not shared"
    }
    self.kick_cira();
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
    event: &ChildEvent,
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
        session.exited.exited(*lived, SystemTime::now());
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
  pub(super) fn cira_toggle_session_disabled(
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
      tv.save(window, cx);
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
          Some(request) => requests.push((pi, id.to_string(), mark, request)),
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
    for (pi, id, mark, request) in requests {
      let call = move |cira: &jc_core::cira::Cira, at: &At, id: &str| match request {
        MarkRequest::Stop => cira.stop(at, id),
        MarkRequest::Remove => cira.remove(at, id),
      };
      let done = move |ws: &mut Self, id: &str| {
        eprintln!("cira: {request:?} {id}");
        if let Some(record) = ws.record_mut(id) {
          record.mark = Some(mark);
        }
      };
      self.ask_cira(pi, id, call, done, window, cx);
    }
  }

  /// Ask Cira `call` about session `id` of project `pi`, one request per id
  /// at a time, then `done` once it is answered — done, or refused for good
  /// (a removed session asked to stop: asking again would not change that).
  /// What records nothing for now (unreachable, no answer, a store that could
  /// not write) is asked again at a later read.
  fn ask_cira(
    &mut self,
    pi: usize,
    id: String,
    call: impl FnOnce(&jc_core::cira::Cira, &At, &str) -> Result<(), CallError> + Send + 'static,
    done: impl FnOnce(&mut Self, &str) + 'static,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(cira) = self.cira_client.clone() else { return };
    self.cira.in_flight.insert(id.clone());
    let at = self.at(pi, &id);
    let id_for_work = id.clone();
    let work = move || call(&cira, &at, &id_for_work);
    self.run_blocking(window, cx, work, move |ws, result, _window, _cx| {
      ws.cira.in_flight.remove(&id);
      match result {
        Err(e) if e.retry() => return,
        Err(e) => eprintln!("cira: {id}: {e}"),
        Ok(()) => {}
      }
      done(ws, &id);
      ws.save_records();
      ws.kick_cira();
    });
  }

  /// Keep each heading's label and Cira's in step (Q-159): a heading renamed
  /// in TODO.md is sent with `cira session label`; a label Cira changed (the
  /// page) is written into the heading. Cira's wins when both moved. One
  /// heading speaks for each id (`TodoDocument::label_headings`).
  fn sync_labels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let read_at = self.cira.read_at.unwrap_or(SystemTime::UNIX_EPOCH);
    // A read begun after a rename was answered has the new label.
    self.cira.renamed_at.retain(|_, at| read_at <= *at);
    let Some(status) = &self.cira.status else { return };
    let Some(records) = &self.cira.records else { return };
    let mut sends = Vec::new();
    let mut writes = Vec::new();
    let mut agreed = Vec::new();
    for (pi, project) in self.projects.iter().enumerate() {
      let tv = project.todo_view.read(cx);
      if tv.is_dirty(cx) {
        continue;
      }
      for (index, heading) in tv.document().label_headings() {
        let Some(id) = heading.cira() else { continue };
        let Some(listed) = status.session(id) else { continue };
        if self.cira.in_flight.contains(id) || self.cira.renamed_at.contains_key(id) {
          continue;
        }
        let last = records.sessions.get(id).and_then(|r| r.label.as_deref());
        match lifecycle::label_sync(&heading.label, listed.label.as_deref(), last) {
          LabelSync::Agreed if last != Some(heading.label.as_str()) => {
            agreed.push((id.to_string(), heading.label.clone()))
          }
          LabelSync::Agreed => {}
          LabelSync::Send => sends.push((pi, id.to_string(), heading.label.clone())),
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
      // A label jc will not write into a heading (one that reads as a
      // marker) is still recorded as Cira's, so the next read sends the
      // heading's own label back instead.
      if !tv.update(cx, |tv, cx| tv.rename_heading_at(index, &label, window, cx)) {
        eprintln!("cira: session {id}'s label {label:?} is not written into its heading");
      }
      if let Some(r) = self.record_mut(&id) {
        r.label = Some(label);
      }
    }
    if record {
      self.save_records();
    }
    for (pi, id, label) in sends {
      let call = {
        let label = label.clone();
        move |cira: &jc_core::cira::Cira, at: &At, id: &str| cira.rename(at, id, &label)
      };
      let done = move |ws: &mut Self, id: &str| {
        ws.cira.renamed_at.insert(id.to_string(), SystemTime::now());
        if let Some(r) = ws.record_mut(id) {
          r.label = Some(label);
        }
      };
      self.ask_cira(pi, id, call, done, window, cx);
    }
  }
}
