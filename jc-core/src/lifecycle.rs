//! What jc does about a session Cira owns (Cira DESIGN 12, PLAN TMUX-01): when
//! a Claude pane attaches, waits or holds ([`pane_action`]), what a changed
//! TODO.md marker asks of Cira ([`mark_request`]), and which side of a label
//! moved ([`label_sync`]). Pure: the workspace feeds them `cira status --json`
//! and what it remembers ([`Records`], `sessions.toml` in jc's config dir).

use crate::cira::{CallError, JobState, ModePending, Session, SessionShell, Status, exit};
use crate::todo::Mark;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// What a Claude pane does now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneAction {
  /// Run `claude attach <short>`, replacing any client it runs.
  Attach(String),
  /// Leave the pane as it is.
  Keep,
  /// Attach nothing yet; the reason is shown. Re-asked at every status read.
  Wait(String),
  /// Attach nothing until Jay asks (or, for a stopped session, until its job
  /// runs again); the reason is shown.
  Hold(String),
}

/// What the pane should do for `session` (Cira's view of it, `None` when
/// Cira does not list it), given whether Cira answered at all, the short id
/// its attach client runs on (`None` when no client runs), and `since`: when
/// it last attached or Jay last asked it to.
///
/// A running client is never torn down here: Cira stops or forks the job, and
/// the client exits by itself. A pane with no client attaches to Cira's
/// current job, except while an operation is in flight or a fork is due (it
/// would attach to the job being replaced, and attaching resumes a stopped
/// job), and except for a session stopped through Cira since `since`, which
/// attaching would undo.
pub fn pane_action(
  session: Option<&Session>,
  reachable: bool,
  attached: Option<&str>,
  since: Option<SystemTime>,
) -> PaneAction {
  let wait = |why: &str| match attached {
    Some(_) => PaneAction::Keep,
    None => PaneAction::Wait(why.to_string()),
  };
  if !reachable {
    return wait("Cira is down (the window's banner says why); this pane attaches once it answers");
  }
  let Some(s) = session else {
    return wait("Cira does not list this session");
  };
  if s.removed_at.is_some() {
    return match attached {
      Some(_) => PaneAction::Keep,
      None => PaneAction::Hold("removed through Cira".into()),
    };
  }
  if let Some(op) = s.op.as_ref().filter(|_| s.op_running()) {
    return wait(&format!("waiting while Cira runs its {}…", op.op));
  }
  if s.mode_pending == ModePending::Due {
    return wait(if s.dangerous { "becoming dangerous…" } else { "becoming auto…" });
  }
  let Some(job) = &s.job else {
    return match s.op_error() {
      Some(error) if can_start_again(Some(s)) => {
        PaneAction::Hold(format!("{error}. {START_AGAIN} asks Cira to start it again"))
      }
      Some(error) => PaneAction::Hold(error.to_string()),
      None => wait("waiting for Cira to start its job…"),
    };
  };
  match attached {
    Some(short) if short == job.short => return PaneAction::Keep,
    Some(_) => return PaneAction::Attach(job.short.clone()),
    None => {}
  }
  let stopped = s.stopped();
  let stopped_since = stopped.is_some_and(|at| since.is_none_or(|since| at > since));
  // Cira lists jobs only when it may act, so a job a delivery woke can still
  // read `stopped`; a turn its hooks began after the stop says it runs (a
  // turn that merely ended then does not: it may have raced the stop).
  let turn = matches!(s.state, crate::cira::State::Working | crate::cira::State::Dialog);
  let woke = turn && stopped.is_some_and(|stop| s.since().is_some_and(|at| at > stop));
  match job.state {
    JobState::Running => PaneAction::Attach(job.short.clone()),
    JobState::Absent => PaneAction::Hold(gone(s, &job.short)),
    JobState::Failed => {
      PaneAction::Hold(s.op_error().unwrap_or("its job failed to start").to_string())
    }
    _ if woke => PaneAction::Attach(job.short.clone()),
    _ if stopped_since => PaneAction::Hold(
      "stopped through Cira; it attaches again once it runs (a message wakes it), or by \
       \"Attach this session\" in Cmd-Shift-P"
        .into(),
    ),
    JobState::Stopped | JobState::Unknown => PaneAction::Attach(job.short.clone()),
  }
}

/// How Jay asks for [`can_start_again`].
const START_AGAIN: &str = "Cmd-Shift-P → \"Start this session again\"";

/// Whether the pane offers "Start this session again" (`cira session start`,
/// Cira PLAN TMUX-16): Cira runs the session, has not removed it, Claude Code
/// no longer has its job (or Cira's start of one failed: TMUX-25), no other
/// session holds that job or continues it (Cira would start nothing), and no
/// operation or fork is under way. Only Jay's asking starts it, a session he
/// stopped included.
pub fn can_start_again(session: Option<&Session>) -> bool {
  session.is_some_and(|s| {
    s.owned
      && s.removed_at.is_none()
      && s.held_by.is_none()
      && s.continued_by.is_none()
      && !s.op_running()
      && s.mode_pending != ModePending::Due
      && match &s.job {
        Some(job) => job.state == JobState::Absent,
        None => s.op_error().is_some(),
      }
  })
}

/// What a pane whose job is gone says ([`can_start_again`]).
fn gone(s: &Session, short: &str) -> String {
  let gone = format!("its job {short} is gone");
  if let Some(h) = &s.held_by {
    let name = h.label.as_deref().unwrap_or(&h.session);
    return format!("{gone}; session {name} (project {}) holds it now", h.project);
  }
  if let Some(by) = &s.continued_by {
    return format!("{gone}; session {by} continues its conversation");
  }
  if !can_start_again(Some(s)) {
    return gone;
  }
  let mode = if s.dangerous { "dangerous" } else { "auto" };
  let stopped = match s.stopped() {
    Some(_) => " (it was stopped through Cira: this clears the stop)",
    None => "",
  };
  format!(
    "{gone}; {START_AGAIN} asks Cira for a new job continuing its conversation, {mode} as \
     before{stopped}"
  )
}

/// The session `cira session new` made for a heading labelled `label` when jc
/// heard no answer: one Cira runs in that project under that label, not
/// removed, that no heading names yet (`named`).
pub fn lost_session<'a>(
  project: &'a crate::cira::Project,
  label: &str,
  named: impl Fn(&str) -> bool,
) -> Option<&'a Session> {
  project.sessions.iter().find(|s| {
    s.owned && s.removed_at.is_none() && s.label.as_deref() == Some(label) && !named(&s.id)
  })
}

/// `cira session new`'s answer, with a refusal that names a session taken as
/// made: under `--wait`, Cira's exit 1 says it made the session but did not
/// start its job, and prints the session with that start's error. It is the
/// heading's session all the same (Cira PLAN TMUX-25): its pane holds on the
/// error ([`pane_action`]) until "Start this session again"
/// ([`can_start_again`]); left unnamed, the next read would give it a heading
/// of its own ([`todo_sync`]).
pub fn session_made(
  answer: Result<crate::cira::Created, CallError>,
) -> Result<crate::cira::Created, CallError> {
  match answer {
    Err(CallError::Refused { code: exit::ERROR, ref stdout, .. }) => {
      serde_json::from_slice(stdout).map_err(|_| answer.unwrap_err())
    }
    answer => answer,
  }
}

/// What jcc's todo.md must change to show what Cira runs for its project
/// (Cira PLAN TMUX-09).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TodoSync {
  /// Sessions to add a heading for: `(id, label)`.
  pub add: Vec<(String, String)>,
  /// Ids of headings whose session Cira removed, to mark `[DELETED]`.
  pub retire: Vec<String>,
}

impl TodoSync {
  /// `text` with `retire`'s headings marked `[DELETED]` and a heading under
  /// the section for each of `add`.
  pub fn apply(&self, text: &str, flavor: crate::flavor::Flavor) -> String {
    let mut text = text.to_string();
    for id in &self.retire {
      let doc = crate::todo::parse(&text, flavor);
      let index = doc.index_by_id(id);
      if let Some(marked) = index.and_then(|i| crate::todo::mark_deleted_at(&text, &doc, i)) {
        text = marked;
      }
    }
    for (id, label) in &self.add {
      let doc = crate::todo::parse(&text, flavor);
      text = crate::todo::insert_session_heading(&text, &doc, label, id);
    }
    text
  }
}

/// Whether `Workspace::sync_todos` may change a TODO now: it is `open`, has no
/// unsaved edits (`dirty`), and none of its sessions is being asked for or
/// looked for (`asking`), since Cira lists such a session before jcc writes
/// its heading.
pub fn may_sync_todo(open: bool, dirty: bool, asking: bool) -> bool {
  open && !dirty && !asking
}

/// Compare `doc` (jcc's todo.md) with `project`, the Cira project it belongs
/// to, and `status`: a live session Cira runs there (`owned`; one known only
/// from its hooks has no job to attach) that no heading names gets one, under
/// its label (its id when the label cannot be a heading:
/// [`crate::todo::writable_label`]); a heading whose session Cira removed is
/// retired. A heading names a session by its id, `[DELETED]` included, or,
/// before its migration, by the conversation it continues (`> uuid=`); one jcc
/// already runs a pane for (`running`: its heading was cut, to be pasted) is
/// not added again.
pub fn todo_sync(
  project: &crate::cira::Project,
  status: &Status,
  doc: &crate::todo::TodoDocument,
  running: impl Fn(&str) -> bool,
) -> TodoSync {
  let headed = |id: &str| doc.session_by_id(id).is_some() || doc.deleted.iter().any(|d| d.id == id);
  let named = |s: &Session| headed(&s.id) || s.uuid.as_deref().is_some_and(headed);
  let add = project
    .sessions
    .iter()
    .filter(|s| s.owned && s.removed_at.is_none() && !named(s) && !running(&s.id))
    .map(|s| {
      let label = s.label.as_deref().filter(|l| crate::todo::writable_label(l));
      (s.id.clone(), label.unwrap_or(&s.id).to_string())
    })
    .collect();
  let mut retire: Vec<String> = Vec::new();
  for id in doc.sessions.iter().filter_map(|h| h.cira()) {
    let removed = status.session(id).is_some_and(|s| s.removed_at.is_some());
    if removed && !retire.iter().any(|r| r == id) {
      retire.push(id.to_string());
    }
  }
  TodoSync { add, retire }
}

/// Bounds of the pause before a pane attaches again after its client exited:
/// [`ATTACH_MIN`] after a client that ran at least [`ATTACH_STABLE`], doubling
/// up to [`ATTACH_MAX`] while it keeps exiting sooner.
pub const ATTACH_MIN: Duration = Duration::from_secs(1);
pub const ATTACH_MAX: Duration = Duration::from_secs(30);
pub const ATTACH_STABLE: Duration = Duration::from_secs(10);

/// The pause before attaching again, after a client that `lived` and the
/// pause `previous` used before it.
pub fn attach_delay(previous: Option<Duration>, lived: Duration) -> Duration {
  match previous {
    Some(previous) if lived < ATTACH_STABLE => (previous * 2).clamp(ATTACH_MIN, ATTACH_MAX),
    _ => ATTACH_MIN,
  }
}

/// When a pane's child last exited and the pause [`attach_delay`] gave it:
/// the Claude pane's attach client and the shell pane's both wait it out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Backoff(Option<(SystemTime, Duration)>);

impl Backoff {
  /// A child that ran for `lived` exited at `now`.
  pub fn exited(&mut self, lived: Duration, now: SystemTime) {
    let previous = self.0.map(|(_, delay)| delay);
    self.0 = Some((now, attach_delay(previous, lived)));
  }

  /// Still waiting out the pause at `now`.
  pub fn waiting(&self, now: SystemTime) -> bool {
    self.0.is_some_and(|(exited, delay)| now < exited + delay)
  }

  /// The last exit came after `at`: a status read begun at `at` predates it.
  pub fn exited_after(&self, at: SystemTime) -> bool {
    self.0.is_some_and(|(exited, _)| exited > at)
  }

  /// Start the next child at once (Jay asked).
  pub fn clear(&mut self) {
    self.0 = None;
  }
}

/// What the Terminal pane runs (Cira DESIGN 12, PLAN TMUX-02).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ShellRun {
  /// Nothing: it asks `cira session shell` once its pause is over.
  #[default]
  Idle,
  /// A `cira session shell` is in flight.
  Asking,
  /// Attached to the session's shell in Cira's tmux server.
  Shared,
  /// jc's own shell, "not shared": Cira gave none.
  Own,
  /// Nothing, for good: the session was removed through Cira.
  Held,
}

/// What the Terminal pane does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellChild {
  /// Attach to the session's shell in Cira's tmux server: this argument list.
  Shared(Vec<String>),
  /// Run jc's own shell, marked "not shared", for this reason.
  Own(String),
  /// The session was removed: run nothing, for this reason.
  Held(String),
  /// Cira refused for a reason that may pass: run nothing, ask again after
  /// the pause.
  Retry(String),
}

/// The Terminal pane's child for Cira's answer; `removed` when Cira lists the
/// session removed. jc only attaches: it never makes or ends a shell.
pub fn shell_child(answer: Result<SessionShell, CallError>, removed: bool) -> ShellChild {
  match answer {
    Ok(shell) if !shell.argv.is_empty() => ShellChild::Shared(shell.argv),
    Ok(_) => ShellChild::Own("Cira's answer names no command".into()),
    // Exit 5 also covers an id tmux cannot name and an ambiguous one: only a
    // removal is for good.
    Err(CallError::Refused { code: exit::INVALID, message, .. }) => {
      let why = message.trim().to_string();
      if removed { ShellChild::Held(why) } else { ShellChild::Retry(why) }
    }
    Err(e) => ShellChild::Own(e.to_string()),
  }
}

/// How many times in a row the shared client may fail within
/// [`ATTACH_STABLE`] before the pane runs jc's own shell instead.
pub const SHARED_FAILURES: u32 = 3;

/// The Terminal pane's state: what it runs, its pause, and the ask in flight.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellPane {
  run: ShellRun,
  backoff: Backoff,
  /// The Cira id the ask in flight is for.
  asked: Option<String>,
  /// Quick failed exits of the shared client in a row.
  failures: u32,
}

impl ShellPane {
  pub fn run(&self) -> ShellRun {
    self.run
  }

  /// Ask Cira for session `id`'s shell now? Only a pane running nothing,
  /// past its pause; it is then asking.
  pub fn ask(&mut self, id: &str, now: SystemTime) -> bool {
    if self.run != ShellRun::Idle || self.backoff.waiting(now) {
      return false;
    }
    self.run = ShellRun::Asking;
    self.asked = Some(id.to_string());
    true
  }

  /// Cira's answer to the ask for `id` arrived at `now` (`removed`: Cira
  /// lists the session removed). The child to run, or `None` when nothing
  /// waits for this answer.
  pub fn answered(
    &mut self,
    id: &str,
    answer: Result<SessionShell, CallError>,
    removed: bool,
    now: SystemTime,
  ) -> Option<ShellChild> {
    if self.run != ShellRun::Asking || self.asked.as_deref() != Some(id) {
      return None;
    }
    self.asked = None;
    let child = shell_child(answer, removed);
    self.run = match child {
      ShellChild::Shared(_) => ShellRun::Shared,
      ShellChild::Own(_) => ShellRun::Own,
      ShellChild::Held(_) => ShellRun::Held,
      ShellChild::Retry(_) => {
        self.backoff.exited(Duration::ZERO, now);
        ShellRun::Idle
      }
    };
    Some(child)
  }

  /// Its child exited at `now` with `code` after `lived`; `said` is the last
  /// line it printed. Jc's own shell, when the shared client has now failed
  /// quickly [`SHARED_FAILURES`] times in a row (a tmux that cannot reach its
  /// server, or one upgraded under it); otherwise it asks again after its
  /// pause. An `exit` (code 0) is never a failure.
  pub fn exited(
    &mut self,
    code: Option<u32>,
    lived: Duration,
    now: SystemTime,
    said: Option<&str>,
  ) -> Option<ShellChild> {
    let shared = match self.run {
      ShellRun::Shared => true,
      ShellRun::Own => false,
      _ => return None,
    };
    self.run = ShellRun::Idle;
    self.backoff.exited(lived, now);
    let failed = shared && code != Some(0) && lived < ATTACH_STABLE;
    self.failures = if failed { self.failures + 1 } else { 0 };
    if self.failures < SHARED_FAILURES {
      return None;
    }
    self.failures = 0;
    self.backoff.clear();
    self.run = ShellRun::Own;
    let said = said.map_or_else(|| "it printed nothing".to_string(), |s| format!("\"{s}\""));
    Some(ShellChild::Own(format!("tmux exited {SHARED_FAILURES} times in a row ({said})")))
  }
}

/// A request a heading's marker change makes of Cira.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkRequest {
  /// `cira session stop`.
  Stop,
  /// `cira session rm`.
  Remove,
}

/// What a heading whose marker jc last acted on as `last` asks now that it
/// says `now`. Edge-triggered (Cira DESIGN 12): only a change asks anything,
/// so a stale `[D]` never stops a session started again from the page. With
/// no record jc has nothing to compare, and the marker becomes the record.
pub fn mark_request(last: Option<Mark>, now: Mark) -> Option<MarkRequest> {
  match (last?, now) {
    (last, now) if last == now => None,
    (_, Mark::Deleted) => Some(MarkRequest::Remove),
    (Mark::Active, Mark::Disabled) => Some(MarkRequest::Stop),
    _ => None,
  }
}

/// Which side of a session's label moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelSync {
  /// They agree.
  Agreed,
  /// The heading was renamed: send it to Cira (`cira session label`).
  Send,
  /// Cira's label changed: write it into the heading.
  Write(String),
}

/// The heading of session `id` says `heading`, Cira says `cira`, and jc
/// last recorded Cira's label as `last`. The label is Cira's (Q-159), so when
/// both moved, or jc has no record, Cira's wins. Where Cira has no label, or
/// one that cannot be a heading ([`crate::todo::writable_label`]), the
/// heading shows the id (`todo_sync`), which agrees with it.
pub fn label_sync(heading: &str, id: &str, cira: Option<&str>, last: Option<&str>) -> LabelSync {
  let shown = cira.filter(|l| crate::todo::writable_label(l)).unwrap_or(id);
  if heading == shown {
    return LabelSync::Agreed;
  }
  match cira {
    None => LabelSync::Send,
    Some(cira) if last == Some(cira) => LabelSync::Send,
    Some(_) => LabelSync::Write(shown.to_string()),
  }
}

/// What jc remembers per Cira session id across restarts, so a marker edit
/// or a rename made while it was closed is acted on at its next start.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Records {
  /// What was shown last, which the next start shows (Cira PLAN TMUX-14).
  pub focus: Option<Focus>,
  pub sessions: BTreeMap<String, Record>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
  /// The marker jc last acted on.
  pub mark: Option<Mark>,
  /// The label the heading and Cira last agreed on.
  pub label: Option<String>,
  /// When its pane last attached, in Unix milliseconds, while it stays
  /// attached ([`note_attach`]): the next start attaches it again unless
  /// Cira recorded a stop after it ([`since_at_start`]).
  pub attached_at: Option<u64>,
}

/// The project, and the session of it by Cira id, that jcc showed last.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Focus {
  pub project: PathBuf,
  pub session: Option<String>,
}

/// The `since` a pane starts with ([`pane_action`]; Cira PLAN TMUX-14): when
/// it last attached, if it was attached when jcc last ran. So it attaches
/// again although its job no longer runs (Claude Code retires an idle one,
/// which Cira does not record as a stop), unless Cira's `stopped_at` (a
/// `cira session stop` from Jay, the page or the CLI) is later (Q-275);
/// any other is held if it was ever stopped through Cira.
pub fn since_at_start(record: Option<&Record>) -> Option<SystemTime> {
  let ms = record?.attached_at?;
  Some(SystemTime::UNIX_EPOCH + Duration::from_millis(ms))
}

/// Note in `record` whether `action`, decided at `now`, leaves its pane
/// attached: an attach does, from `now`; a hold (stopped or removed through
/// Cira, a failed start) does not; waiting or keeping changes nothing.
/// Whether the record changed.
pub fn note_attach(record: &mut Record, action: &PaneAction, now: SystemTime) -> bool {
  let attached_at = match action {
    PaneAction::Attach(_) => {
      Some(now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_millis() as u64)
    }
    PaneAction::Hold(_) => None,
    PaneAction::Keep | PaneAction::Wait(_) => return false,
  };
  std::mem::replace(&mut record.attached_at, attached_at) != attached_at
}

/// The index of the project shown at start, given each project's path and
/// whether its todo.md is open: the one focused last, else the first open
/// one (the focused one is gone, or its todo.md does not open: another
/// project holds it, or Cira refused it); `None` while none is open.
pub fn start_project(focus: Option<&Focus>, projects: &[(&Path, bool)]) -> Option<usize> {
  let focused = focus.and_then(|f| projects.iter().position(|(p, _)| *p == f.project));
  focused.filter(|&i| projects[i].1).or_else(|| projects.iter().position(|(_, open)| *open))
}

/// What a jcc project whose todo.md is not open yet does with a status read
/// ([`plan_todos`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoPlan {
  /// Open this file and start its sessions.
  Open(PathBuf),
  /// This file is already the TODO of jc project `project` (an enclosing
  /// directory in the same Cira project): opening it twice would start every
  /// session twice, so it stays closed here.
  HeldBy { todo: PathBuf, project: PathBuf },
  /// No Cira project holds the directory yet: register one.
  Register,
  /// No Cira project holds it, and jcc registers none (`NoRegister`).
  Unregistered(NoRegister),
  /// Its todo.md is open, but Cira no longer names it for this project (the
  /// project was removed): close it, so no save makes it again.
  Unlisted,
}

/// Why jcc registers no Cira project for a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoRegister {
  /// It is the home directory, which Cira refuses (Cira PLAN SES-07).
  Home,
  /// It was in jcc's list before this run: jcc registers a project only when
  /// it is added (`jcc <dir>`, or to state.toml), so one Jay removed in Cira
  /// does not come back at the next start.
  NotAdded,
}

/// What the TODO pane says when `cira project add NAME` did not register
/// the directory. Cira refusing it (exit 5: a home directory, say, Cira PLAN
/// SES-07) is an answer, not a failure.
pub fn unregistered_why(name: &str, e: &CallError) -> String {
  match e {
    CallError::Refused { code: exit::INVALID, message, .. } => {
      format!("Cira registers no project here: {}", message.trim())
    }
    e => format!("jcc's TODO cannot open: `cira project add {name}` failed: {e}"),
  }
}

/// What the TODO pane says for a directory jcc registers no project for.
pub fn unregistered_text(why: NoRegister) -> &'static str {
  match why {
    NoRegister::Home => {
      "jcc keeps no TODO for the home directory: Cira registers no project there. Open a \
       project's directory instead."
    }
    NoRegister::NotAdded => {
      "Cira lists no project for this directory. jcc registers one only when you add it: \
       `jcc .` in it."
    }
  }
}

/// A jcc project as [`plan_todos`] sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoProject {
  pub path: PathBuf,
  /// The todo.md it has open.
  pub open: Option<PathBuf>,
  /// It was added to jcc's list in this run (or `jcc <dir>` named it).
  pub added: bool,
}

/// jcc: for each project, in order, what a status read means for it. One
/// todo.md is never open in two projects: the first to claim it, or the one
/// that already has it, keeps it. A directory no Cira project holds is
/// registered only when it was `added`, and never at `home`.
pub fn plan_todos(
  status: &Status,
  projects: &[TodoProject],
  home: Option<&Path>,
) -> Vec<(usize, TodoPlan)> {
  // Who holds each todo.md: those open already, then each claim in order.
  let mut held: Vec<(PathBuf, PathBuf)> =
    projects.iter().filter_map(|p| Some((p.open.clone()?, p.path.clone()))).collect();
  let mut plans = Vec::new();
  for (i, TodoProject { path, open, added }) in projects.iter().enumerate() {
    let todo = status.todo_for(path);
    if let Some(open) = open {
      if todo.as_ref() != Some(open) {
        plans.push((i, TodoPlan::Unlisted));
      }
      continue;
    }
    let Some(todo) = todo else {
      let plan = if home == Some(path.as_path()) {
        TodoPlan::Unregistered(NoRegister::Home)
      } else if !added {
        TodoPlan::Unregistered(NoRegister::NotAdded)
      } else {
        TodoPlan::Register
      };
      plans.push((i, plan));
      continue;
    };
    match held.iter().find(|(t, _)| *t == todo) {
      Some((_, project)) => plans.push((i, TodoPlan::HeldBy { todo, project: project.clone() })),
      None => {
        held.push((todo.clone(), path.clone()));
        plans.push((i, TodoPlan::Open(todo)));
      }
    }
  }
  plans
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn one_todo_md_is_never_open_in_two_projects() {
    // jc projects ~/w/cira and ~/w/cira/worktrees/x are both in Cira project `cira`.
    let status =
      Status::parse(br#"{"home":"/store","projects":[{"name":"cira","path":"/w/cira"}]}"#).unwrap();
    let todo = PathBuf::from("/store/projects/cira/todo.md");
    let outer = PathBuf::from("/w/cira");
    let inner = PathBuf::from("/w/cira/worktrees/x");
    let other = PathBuf::from("/elsewhere");

    let project = |path: &PathBuf, open: Option<&PathBuf>| TodoProject {
      path: path.clone(),
      open: open.cloned(),
      added: true,
    };
    let plans = plan_todos(
      &status,
      &[project(&outer, None), project(&inner, None), project(&other, None)],
      None,
    );
    assert_eq!(plans[0], (0, TodoPlan::Open(todo.clone())));
    assert_eq!(
      plans[1],
      (1, TodoPlan::HeldBy { todo: todo.clone(), project: outer.clone() }),
      "two views on one todo.md would start and attach every session twice"
    );
    assert_eq!(plans[2], (2, TodoPlan::Register));

    // Whichever project already has it open keeps it.
    let plans = plan_todos(&status, &[project(&outer, None), project(&inner, Some(&todo))], None);
    assert_eq!(plans, [(0, TodoPlan::HeldBy { todo, project: inner })]);
  }

  #[test]
  fn ciras_refusal_to_register_reads_as_its_answer() {
    let refused = CallError::Refused {
      code: exit::INVALID,
      message: "a project cannot be the home directory\n".into(),
      stdout: Vec::new(),
    };
    assert_eq!(
      unregistered_why("jay", &refused),
      "Cira registers no project here: a project cannot be the home directory"
    );
    let failed = unregistered_why("p", &CallError::NoAnswer);
    assert!(failed.starts_with("jcc's TODO cannot open: `cira project add p`"), "{failed}");
  }

  /// Cira SES-07: Cira refuses a project at the home directory, and one Jay
  /// removed in Cira (`cira project rm`) must not come back at jcc's next
  /// start, nor its todo.md be written again.
  #[test]
  fn jcc_registers_only_what_it_was_given_and_never_home() {
    let status =
      Status::parse(br#"{"home":"/store","projects":[{"name":"cira","path":"/w/cira"}]}"#).unwrap();
    let p = |path: &str, open: Option<&str>, added: bool| TodoProject {
      path: path.into(),
      open: open.map(PathBuf::from),
      added,
    };
    let home = Path::new("/Users/jay");
    let plans = plan_todos(
      &status,
      &[
        p("/Users/jay", None, true),
        p("/w/listed-before", None, false),
        p("/w/new", None, true),
        p("/w/removed", Some("/store/projects/removed/todo.md"), false),
        p("/w/cira", Some("/store/projects/cira/todo.md"), false),
      ],
      Some(home),
    );
    assert_eq!(
      plans,
      [
        (0, TodoPlan::Unregistered(NoRegister::Home)),
        (1, TodoPlan::Unregistered(NoRegister::NotAdded)),
        (2, TodoPlan::Register),
        (3, TodoPlan::Unlisted),
      ]
    );
  }
  use crate::cira::Status;

  /// A session as `cira status --json` lists it, from the real fixture's
  /// shape with `life` merged in.
  fn session(life: serde_json::Value) -> Session {
    let mut s = serde_json::json!({
      "id": "c-1", "state": "idle", "state_since": "1970-01-01T00:01:00Z",
      "owned": true, "dangerous": false, "mode_pending": "none",
    });
    for (k, v) in life.as_object().unwrap() {
      s[k] = v.clone();
    }
    let status = serde_json::json!({ "projects": [{ "name": "p", "sessions": [s] }] });
    Status::parse(status.to_string().as_bytes()).unwrap().session("c-1").unwrap().clone()
  }

  fn job(short: &str, state: &str) -> serde_json::Value {
    serde_json::json!({ "short": short, "dangerous": false, "state": state })
  }

  fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
  }

  const STOPPED_AT_200: &str = "1970-01-01T00:03:20Z";

  fn attach(short: &str) -> PaneAction {
    PaneAction::Attach(short.into())
  }

  /// TMUX-09 (Jay, 2026-11-14): "The ported session doesn't show up at all
  /// in jcc" (made with `cira session new --resume`, so no heading), and
  /// "First", removed through Cira, kept its heading.
  #[test]
  fn jccs_todo_follows_the_sessions_cira_runs_for_its_project() {
    let status = serde_json::json!({ "projects": [
      { "name": "p", "path": "/w/p", "sessions": [
        { "id": "c-new", "label": "orchestrator", "owned": true },
        { "id": "c-had", "label": "had", "owned": true },
        { "id": "c-first", "label": "First", "owned": true, "removed_at": "1970-01-01T00:01:00Z" },
        { "id": "c-gone", "label": "gone", "owned": true, "removed_at": "1970-01-01T00:01:00Z" },
        { "id": "c-hooked", "label": "foreground", "owned": false },
        { "id": "c-legacy", "uuid": "u-1", "label": "legacy", "owned": true },
        { "id": "c-marker", "label": "[DELETED] x", "owned": true },
        { "id": "c-dead", "label": "dead", "owned": true },
      ]},
    ]});
    let mut status = status;
    for s in status["projects"][0]["sessions"].as_array_mut().unwrap() {
      for (k, v) in
        [("state", "idle"), ("state_since", "1970-01-01T00:01:00Z"), ("mode_pending", "none")]
      {
        s[k] = v.into();
      }
      s["dangerous"] = false.into();
    }
    let status = Status::parse(status.to_string().as_bytes()).unwrap();
    let text = "# Cira\n## had\n> cira=c-had\n\n### WAIT\n\
                ## First\n> cira=c-first\n\n### Message 0\nhello\n### WAIT\n\
                ## legacy\n> uuid=u-1\n\n### WAIT\n\
                ## [DELETED] dead\n> cira=c-dead\n";
    let doc = crate::todo::parse(text, crate::flavor::Flavor::Jcc);
    let sync = todo_sync(&status.projects[0], &status, &doc, |_| false);
    assert_eq!(
      sync.add,
      [("c-new".to_string(), "orchestrator".to_string()), ("c-marker".into(), "c-marker".into())],
      "every live session Cira runs here that no heading names (a legacy heading names its \
       conversation's; a hook-only session has no job; a label that reads as a marker is not one)"
    );
    assert_eq!(sync.retire, ["c-first"], "a heading whose session Cira removed");

    // Applied, the file names every session once and the sync is done.
    let out = sync.apply(text, crate::flavor::Flavor::Jcc);
    let after = crate::todo::parse(&out, crate::flavor::Flavor::Jcc);
    assert!(out.contains("## [DELETED] First\n> cira=c-first\n\n### Message 0\nhello\n"), "{out}");
    assert!(out.contains("## orchestrator\n> cira=c-new\n"), "{out}");
    assert!(after.session_by_id("c-had").is_some() && after.session_by_id("c-first").is_none());
    assert_eq!(todo_sync(&status.projects[0], &status, &after, |_| false), TodoSync::default());

    // Review P2: a session jcc already runs a pane for, whose heading was cut
    // (to be pasted elsewhere), gets no second heading and no second pane.
    let cut = todo_sync(&status.projects[0], &status, &doc, |id| id == "c-new");
    assert_eq!(cut.add, [("c-marker".to_string(), "c-marker".to_string())]);
  }

  /// Cira's answer to a `cira session new --wait` whose job it would not
  /// start ("held": jc still ran the conversation, tbm 2026-11-16), and the
  /// status that lists the session it made: no job, no uuid yet, the start's
  /// error.
  fn refused_start(id: &str, label: &str, resume: &str) -> (CallError, Status) {
    let held = format!(
      "held: conversation {resume} runs in another Claude Code process, and resuming it \
       would start a copy; end that process, then ask again (`cira session new --resume`)"
    );
    let listed = serde_json::json!({
      "id": id, "label": label, "owned": true, "dangerous": false,
      "state": "idle", "state_since": "1970-01-01T00:01:00Z", "mode_pending": "none",
      "op": { "op": "start", "running": false, "error": held },
    });
    let stdout = serde_json::json!({ "session": id, "status": listed }).to_string().into_bytes();
    let refused = CallError::Refused { code: exit::ERROR, message: held.clone(), stdout };
    let status = serde_json::json!({ "projects": [
      { "name": "tbm", "path": "/w/tbm", "sessions": [listed] },
    ]});
    (refused, Status::parse(status.to_string().as_bytes()).unwrap())
  }

  /// What jcc's todo.md holds after `Workspace::session_answered` takes
  /// `answer` for the heading `asked` (`None`: a new session, labelled
  /// `label`) and the next status read is synced into it, and the panes it
  /// then runs: the one that asked, plus one per heading the sync adds.
  fn after_answer(
    text: &str,
    asked: Option<crate::todo::SessionKey>,
    label: &str,
    answer: CallError,
    status: &Status,
  ) -> (crate::todo::TodoDocument, usize) {
    let jcc = crate::flavor::Flavor::Jcc;
    let doc = crate::todo::parse(text, jcc);
    // The app's heading choice (`session_answered`): the heading asked for,
    // else a new one. A refusal leaves the file alone and the pane failed.
    let (text, key) = match session_made(Err(answer)) {
      Ok(made) => {
        let text = match &asked {
          Some(key) => {
            let index = doc.index_of(key).expect("the heading that asked");
            crate::todo::bind_cira_at(text, &doc, index, &made.session).unwrap()
          }
          None => crate::todo::insert_session_heading(text, &doc, label, &made.session),
        };
        (text, made.session)
      }
      Err(_) => {
        let key = match asked {
          Some(crate::todo::SessionKey::Id(id)) => id,
          _ => String::new(),
        };
        (text.to_string(), key)
      }
    };
    let doc = crate::todo::parse(&text, jcc);
    let sync = todo_sync(&status.projects[0], status, &doc, |id| id == key);
    let panes = 1 + sync.add.len();
    (crate::todo::parse(&sync.apply(&text, jcc), jcc), panes)
  }

  /// Cira PLAN TMUX-25 (tbm, 2026-11-16): Onboarding, moved from jc, showed
  /// twice in jcc once a retry worked. Cira's refusal had made the session;
  /// with no heading naming it (its job never ran, so it has no uuid to match
  /// the legacy `> uuid=`), the next read gave it a heading and a pane of
  /// its own beside the failed one.
  #[test]
  fn a_refused_move_leaves_one_heading_and_one_session() {
    let uuid = "7623c801-099a-4824-b212-4dcb1b3d1c7d";
    let (refused, status) = refused_start("cira-99550bde1b9e", "Onboarding", uuid);
    let text = format!("# Cira\n## Onboarding\n> uuid={uuid}\n\n### WAIT\n");
    let key = crate::todo::SessionKey::Id(uuid.into());
    let (doc, panes) = after_answer(&text, Some(key), "Onboarding", refused, &status);
    let headings: Vec<_> = doc.sessions.iter().map(|s| (s.label.as_str(), s.id.as_str())).collect();
    assert_eq!(
      (headings.as_slice(), panes),
      ([("Onboarding", "cira-99550bde1b9e")].as_slice(), 1),
      "the moved heading names the session Cira made for it, and nothing else does"
    );
  }

  /// TMUX-25's review: a new session, and an unbound heading adopted, that
  /// Cira refuses to start likewise get one heading and one pane.
  #[test]
  fn a_refused_new_session_or_unbound_adopt_leaves_one_heading_and_one_session() {
    let (refused, status) = refused_start("c-new", "New Session", "");
    let (doc, panes) = after_answer("# Cira\n", None, "New Session", refused, &status);
    let headings: Vec<_> = doc.sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!((headings.as_slice(), panes), (["c-new"].as_slice(), 1), "a new session");

    let (refused, status) = refused_start("c-old", "Old", "");
    let text = "# Cira\n## Old\n> cira=\n\n### WAIT\n";
    let key = crate::todo::SessionKey::Unbound { index: 0, label: "Old".into() };
    let (doc, panes) = after_answer(text, Some(key), "Old", refused, &status);
    let headings: Vec<_> = doc.sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!((headings.as_slice(), panes), (["c-old"].as_slice(), 1), "an unbound heading");
  }

  /// A session whose start Cira refused has no job to attach: its pane holds
  /// on Cira's error and offers "Start this session again", which asks Cira
  /// for the job it would not start (`cira session start`).
  #[test]
  fn a_session_whose_start_failed_can_be_started_again() {
    let (_, status) = refused_start("c-1", "x", "u-1");
    let s = status.session("c-1");
    assert!(can_start_again(s), "a failed start with no job");
    match pane_action(s, true, None, None) {
      PaneAction::Hold(why) => {
        assert!(why.starts_with("held: conversation u-1"), "{why}");
        assert!(why.contains("\"Start this session again\""), "{why}");
      }
      other => panic!("{other:?}"),
    }
  }

  /// Review: the gating of `Workspace::sync_todos`.
  #[test]
  fn a_todo_is_synced_only_when_open_saved_and_not_asking() {
    assert!(may_sync_todo(true, false, false));
    assert!(!may_sync_todo(false, false, false), "not open yet");
    assert!(!may_sync_todo(true, true, false), "unsaved edits");
    assert!(!may_sync_todo(true, false, true), "a session is being asked for");
  }

  #[test]
  fn a_pane_attaches_to_the_job_cira_names() {
    let s = session(serde_json::json!({ "job": job("aaaa1111", "running") }));
    assert_eq!(pane_action(Some(&s), true, None, None), attach("aaaa1111"));
    assert_eq!(pane_action(Some(&s), true, Some("aaaa1111"), None), PaneAction::Keep);
    // A stopped or retired job: attach resumes it (no stop was asked).
    let s = session(serde_json::json!({ "job": job("aaaa1111", "stopped") }));
    assert_eq!(pane_action(Some(&s), true, None, Some(at(100))), attach("aaaa1111"));
  }

  #[test]
  fn a_forked_job_is_attached_by_its_new_short_id() {
    // A mode change forked the job: the client on the old one is replaced,
    // never re-run on the short id it had.
    let s = session(serde_json::json!({ "job": job("bbbb2222", "running"), "dangerous": true }));
    assert_eq!(pane_action(Some(&s), true, Some("aaaa1111"), None), attach("bbbb2222"));
  }

  #[test]
  fn nothing_is_attached_while_an_operation_is_in_flight_or_a_fork_is_due() {
    let running_op = serde_json::json!({ "op": "mode", "running": true, "at": "x" });
    let s = session(serde_json::json!({ "job": job("aaaa1111", "stopped"), "op": running_op }));
    assert!(matches!(pane_action(Some(&s), true, None, None), PaneAction::Wait(_)));
    // A client still on the job is left for the operation to end.
    assert_eq!(pane_action(Some(&s), true, Some("aaaa1111"), None), PaneAction::Keep);
    let s =
      session(serde_json::json!({ "job": job("aaaa1111", "stopped"), "mode_pending": "due" }));
    assert!(matches!(pane_action(Some(&s), true, None, None), PaneAction::Wait(_)));
    // Waiting for idle: the job still runs in its old mode, so attach.
    let s = session(serde_json::json!({
      "job": job("aaaa1111", "running"), "mode_pending": "waiting_for_idle"
    }));
    assert_eq!(pane_action(Some(&s), true, None, None), attach("aaaa1111"));
  }

  /// Cira PLAN TMUX-14, Q-275: a session whose pane was attached when jcc
  /// quit attaches again at the next start, even though its job no longer
  /// runs (retired idle); not one stopped on purpose through Cira since
  /// (`stopped_at` after the attach), nor one held when jcc last decided.
  #[test]
  fn a_session_attached_at_quit_attaches_again_unless_stopped_since() {
    let stopped_on_purpose = session(serde_json::json!({
      "job": job("aaaa1111", "stopped"), "stopped_at": STOPPED_AT_200
    }));
    let retired = session(serde_json::json!({ "job": job("aaaa1111", "stopped") }));
    let start = |attached_at: u64| {
      let mut record = Record::default();
      assert!(note_attach(&mut record, &attach("aaaa1111"), at(attached_at)), "an attach is noted");
      assert!(!note_attach(&mut record, &PaneAction::Keep, at(attached_at + 50)));
      since_at_start(Some(&record))
    };
    // Attached at 100, quit, then retired idle: attached again.
    assert_eq!(
      pane_action(Some(&retired), true, None, start(100)),
      attach("aaaa1111"),
      "not restored"
    );
    // Attached at 100, quit, then stopped through Cira at 200: left stopped.
    assert!(
      matches!(pane_action(Some(&stopped_on_purpose), true, None, start(100)), PaneAction::Hold(_)),
      "a stop made through Cira after jcc quit was undone"
    );
    // Stopped at 200, then attached again at 300 (Jay asked): attached again.
    assert_eq!(pane_action(Some(&stopped_on_purpose), true, None, start(300)), attach("aaaa1111"));
    // Held when last decided: held at start too.
    let mut record = Record::default();
    note_attach(&mut record, &attach("aaaa1111"), at(100));
    assert!(note_attach(&mut record, &PaneAction::Hold("stopped".into()), at(250)));
    let since = since_at_start(Some(&record));
    assert!(matches!(
      pane_action(Some(&stopped_on_purpose), true, None, since),
      PaneAction::Hold(_)
    ));
    let none = since_at_start(None);
    assert!(matches!(
      pane_action(Some(&stopped_on_purpose), true, None, none),
      PaneAction::Hold(_)
    ));
  }

  #[test]
  fn jcc_starts_on_the_project_it_showed_last() {
    let (a, b) = (Path::new("/a"), Path::new("/b"));
    let focus = |p: &str| Focus { project: p.into(), session: Some("c-1".into()) };
    assert_eq!(start_project(Some(&focus("/b")), &[(a, true), (b, true)]), Some(1));
    assert_eq!(start_project(Some(&focus("/gone")), &[(a, true), (b, true)]), Some(0));
    assert_eq!(start_project(None, &[(a, true), (b, true)]), Some(0));
    // Review P3: the focused project's todo.md does not open (another project
    // holds it): the start shows another rather than waiting for good.
    assert_eq!(start_project(Some(&focus("/a")), &[(a, false), (b, true)]), Some(1));
    assert_eq!(start_project(Some(&focus("/a")), &[(a, false), (b, false)]), None);
  }

  #[test]
  fn a_session_stopped_through_cira_is_not_resumed_by_attaching() {
    let s = session(serde_json::json!({
      "job": job("aaaa1111", "stopped"), "stopped_at": STOPPED_AT_200
    }));
    assert!(matches!(pane_action(Some(&s), true, None, Some(at(100))), PaneAction::Hold(_)));
    assert!(matches!(pane_action(Some(&s), true, None, None), PaneAction::Hold(_)), "jc start");
    // Jay asked after the stop (adopting it, the palette's attach): attach.
    assert_eq!(pane_action(Some(&s), true, None, Some(at(300))), attach("aaaa1111"));
    // A delivery woke it: follow it.
    let s = session(serde_json::json!({
      "job": job("aaaa1111", "running"), "stopped_at": STOPPED_AT_200
    }));
    assert_eq!(pane_action(Some(&s), true, None, Some(at(100))), attach("aaaa1111"));
    // Woken, with Cira's listing taken before the respawn (measured against a
    // dev daemon, 2026-11-12: the job still read `stopped`): its hooks moved
    // its state after the stop, so it runs.
    let s = session(serde_json::json!({
      "job": job("aaaa1111", "stopped"), "stopped_at": STOPPED_AT_200,
      "state": "working", "state_since": "1970-01-01T00:05:00Z"
    }));
    assert_eq!(pane_action(Some(&s), true, None, Some(at(100))), attach("aaaa1111"));
    // ...but a session that only ended since is not running, and neither is
    // one whose turn ended just after the stop (found in review: a Stop hook
    // racing the stop would have been taken as a wake).
    for state in ["gone", "idle"] {
      let s = session(serde_json::json!({
        "job": job("aaaa1111", "stopped"), "stopped_at": STOPPED_AT_200,
        "state": state, "state_since": "1970-01-01T00:05:00Z"
      }));
      assert!(
        matches!(pane_action(Some(&s), true, None, Some(at(100))), PaneAction::Hold(_)),
        "{state}"
      );
    }
    // A job Claude Code no longer lists is held even if hooks moved after the stop.
    let s = session(serde_json::json!({
      "job": job("aaaa1111", "absent"), "stopped_at": STOPPED_AT_200,
      "state": "working", "state_since": "1970-01-01T00:05:00Z"
    }));
    assert!(matches!(pane_action(Some(&s), true, None, Some(at(100))), PaneAction::Hold(_)));
  }

  /// Cira PLAN TMUX-16 (Jay, 2026-11-14): jcc printed "its job eb3c2f7e is
  /// gone" with nothing to do there, and he had to leave for a terminal. The
  /// pane offers the way back and says what Cira will do, in the mode the
  /// session has; it never starts it by itself, a stop through Cira
  /// included, which the offer says it clears.
  #[test]
  fn a_gone_job_offers_to_start_it_again_and_says_what_cira_will_do() {
    let hold = |s: &Session| match pane_action(Some(s), true, None, Some(at(100))) {
      PaneAction::Hold(why) => why,
      other => panic!("not held: {other:?}"),
    };
    let s = session(serde_json::json!({ "job": job("eb3c2f7e", "absent") }));
    let why = hold(&s);
    assert!(
      why.contains("eb3c2f7e") && why.contains("Start this session again") && why.contains("auto"),
      "{why}"
    );
    assert!(can_start_again(Some(&s)));
    let s = session(serde_json::json!({ "job": job("eb3c2f7e", "absent"), "dangerous": true }));
    assert!(hold(&s).contains("dangerous"), "{}", hold(&s));
    let s = session(serde_json::json!({
      "job": job("eb3c2f7e", "absent"), "stopped_at": STOPPED_AT_200
    }));
    assert!(hold(&s).contains("clears the stop"), "{}", hold(&s));
    assert!(can_start_again(Some(&s)), "Jay may start a session he stopped");
    // Nothing to offer: a running job, a removed session, one Cira does not run.
    let running = session(serde_json::json!({ "job": job("eb3c2f7e", "running") }));
    let removed = session(serde_json::json!({
      "job": job("eb3c2f7e", "absent"), "removed_at": STOPPED_AT_200
    }));
    let foreign = session(serde_json::json!({ "job": job("eb3c2f7e", "absent"), "owned": false }));
    let op = serde_json::json!({ "op": "mode", "running": true, "at": "x" });
    let busy = session(serde_json::json!({ "job": job("eb3c2f7e", "absent"), "op": op }));
    let due =
      session(serde_json::json!({ "job": job("eb3c2f7e", "absent"), "mode_pending": "due" }));
    for s in [&running, &removed, &foreign, &busy, &due] {
      assert!(!can_start_again(Some(s)), "{s:?}");
    }
    assert!(!can_start_again(None));
    // Review P1: another session holds its job or continues it (Cira PLAN SES-12): Cira
    // would answer `held` and start nothing, so nothing is offered, and the pane names that
    // session instead.
    let holder = serde_json::json!({ "project": "p", "session": "c-2", "label": "newer" });
    let held = session(serde_json::json!({ "job": job("eb3c2f7e", "absent"), "held_by": holder }));
    let continued =
      session(serde_json::json!({ "job": job("eb3c2f7e", "absent"), "continued_by": "c-3" }));
    for (s, other) in [(&held, "newer"), (&continued, "c-3")] {
      assert!(!can_start_again(Some(s)), "{s:?}");
      let why = hold(s);
      assert!(!why.contains("Start this session again") && why.contains(other), "{why}");
    }
  }

  #[test]
  fn removed_failed_and_gone_jobs_are_held() {
    let s = session(serde_json::json!({
      "job": job("aaaa1111", "absent"), "removed_at": STOPPED_AT_200
    }));
    assert!(matches!(pane_action(Some(&s), true, None, None), PaneAction::Hold(_)));
    let s = session(serde_json::json!({ "job": job("aaaa1111", "absent") }));
    assert!(matches!(pane_action(Some(&s), true, None, None), PaneAction::Hold(_)));
    let failed = serde_json::json!({ "op": "start", "running": false, "error": "held", "at": "x" });
    let s = session(serde_json::json!({ "op": failed }));
    let start_again = format!("held. {START_AGAIN} asks Cira to start it again");
    assert_eq!(pane_action(Some(&s), true, None, None), PaneAction::Hold(start_again));
    // No job yet and nothing failed: its start is on its way.
    let s = session(serde_json::json!({}));
    assert!(matches!(pane_action(Some(&s), true, None, None), PaneAction::Wait(_)));
  }

  #[test]
  fn while_cira_is_unreachable_nothing_new_is_attached() {
    let s = session(serde_json::json!({ "job": job("aaaa1111", "running") }));
    assert!(matches!(pane_action(None, false, None, None), PaneAction::Wait(_)));
    assert!(matches!(pane_action(Some(&s), false, None, None), PaneAction::Wait(_)));
    // A client already attached keeps running: the session lives on.
    assert_eq!(pane_action(None, false, Some("aaaa1111"), None), PaneAction::Keep);
    assert!(matches!(pane_action(None, true, None, None), PaneAction::Wait(_)), "unknown to Cira");
  }

  #[test]
  fn attaching_again_backs_off_while_the_client_keeps_exiting_quickly() {
    let quick = Duration::from_millis(200);
    assert_eq!(attach_delay(None, quick), ATTACH_MIN);
    let mut delay = attach_delay(None, quick);
    for _ in 0..10 {
      delay = attach_delay(Some(delay), quick);
    }
    assert_eq!(delay, ATTACH_MAX, "doubles up to the cap");
    assert_eq!(attach_delay(Some(ATTACH_MAX), ATTACH_STABLE), ATTACH_MIN);
  }

  fn listed_shell(argv: &[&str]) -> SessionShell {
    // The answer a real `cira session shell --json` gave (make-shell.sh).
    let mut shell: SessionShell = serde_json::from_slice(SHELL_FIXTURE).unwrap();
    shell.argv = argv.iter().map(|w| w.to_string()).collect();
    shell
  }

  const SHELL_FIXTURE: &[u8] = include_bytes!("../tests/fixtures/cira/session-shell.json");

  fn refused(code: i32, message: &str) -> CallError {
    CallError::Refused { code, message: format!("{message}\n"), stdout: vec![] }
  }

  #[test]
  fn a_pane_waits_out_its_pause_after_its_child_exited() {
    let mut b = Backoff::default();
    assert!(!b.waiting(at(100)), "never exited");
    b.exited(Duration::from_millis(10), at(100));
    assert!(b.waiting(at(100)) && !b.waiting(at(101)), "ATTACH_MIN after a first quick exit");
    assert!(b.exited_after(at(99)) && !b.exited_after(at(100)));
    b.exited(Duration::from_millis(10), at(200));
    assert!(b.waiting(at(201)) && !b.waiting(at(202)), "doubles while it keeps exiting");
    b.clear();
    assert!(!b.waiting(at(200)));
  }

  #[test]
  fn the_terminal_pane_attaches_to_ciras_shell_and_says_when_it_cannot() {
    let argv = ["/t/tmux", "-u", "-S", "/h/tmux.sock", "attach-session", "-t", "=s-c-1"];
    let shared = ShellChild::Shared(argv.iter().map(|w| w.to_string()).collect());
    let real: SessionShell = serde_json::from_slice(SHELL_FIXTURE).unwrap();
    assert!(matches!(shell_child(Ok(real), false), ShellChild::Shared(a) if a.len() == 7));
    assert_eq!(shell_child(Ok(listed_shell(&argv)), false), shared);
    assert!(matches!(shell_child(Ok(listed_shell(&[])), false), ShellChild::Own(_)));
    assert_eq!(
      shell_child(Err(CallError::Unreachable(crate::cira::Down::NotInstalled)), false),
      ShellChild::Own(crate::cira::Down::NotInstalled.to_string())
    );
    let no_tmux = refused(exit::ERROR, "no tmux");
    assert!(
      matches!(shell_child(Err(no_tmux), false), ShellChild::Own(w) if w.contains("no tmux"))
    );
  }

  #[test]
  fn only_a_removal_holds_the_pane_and_other_refusals_are_asked_again() {
    // Exit 5 is also an ambiguous or un-nameable id: not for good.
    let removed = refused(exit::INVALID, "session c-1 was removed; it has no shell");
    assert!(matches!(shell_child(Err(removed.clone()), true), ShellChild::Held(_)));
    assert!(matches!(shell_child(Err(removed), false), ShellChild::Retry(_)), "status lags");
    let ambiguous = refused(exit::INVALID, "c-1 names two sessions");
    let mut pane = ShellPane::default();
    assert!(pane.ask("c-1", at(100)));
    assert!(matches!(
      pane.answered("c-1", Err(ambiguous), false, at(100)),
      Some(ShellChild::Retry(_))
    ));
    assert_eq!(pane.run(), ShellRun::Idle);
    assert!(!pane.ask("c-1", at(100)), "waits out its pause");
    assert!(pane.ask("c-1", at(101)), "then asks again");
  }

  #[test]
  fn the_shell_pane_asks_once_and_takes_only_the_answer_it_waits_for() {
    let argv = ["/t/tmux", "attach-session", "-t", "=s-c-1"];
    let mut pane = ShellPane::default();
    assert!(pane.ask("c-1", at(100)));
    assert!(!pane.ask("c-1", at(100)), "one ask in flight");
    // An answer for another id (the session was rebound) is not taken.
    assert_eq!(pane.answered("c-2", Ok(listed_shell(&argv)), false, at(100)), None);
    assert!(matches!(
      pane.answered("c-1", Ok(listed_shell(&argv)), false, at(100)),
      Some(ShellChild::Shared(_))
    ));
    assert_eq!(pane.run(), ShellRun::Shared);
    // A second answer to the same ask finds the pane running: stale.
    assert_eq!(pane.answered("c-1", Ok(listed_shell(&argv)), false, at(100)), None);
    assert!(!pane.ask("c-1", at(200)), "a running shell is never replaced");
    // Jay's `exit`: it asks again after its pause.
    assert_eq!(pane.exited(Some(0), Duration::from_secs(60), at(300), None), None);
    assert!(!pane.ask("c-1", at(300)) && pane.ask("c-1", at(301)));
    // A removal holds it for good.
    let removed = refused(exit::INVALID, "removed");
    assert!(matches!(pane.answered("c-1", Err(removed), true, at(301)), Some(ShellChild::Held(_))));
    assert!(!pane.ask("c-1", at(9999)));
    assert_eq!(pane.exited(None, Duration::ZERO, at(9999), None), None, "nothing runs");
  }

  #[test]
  fn only_a_genuinely_failed_ask_gives_jcs_own_shell_and_its_exit_asks_cira_again() {
    let mut pane = ShellPane::default();
    assert!(pane.ask("c-1", at(100)));
    assert!(matches!(
      pane.answered(
        "c-1",
        Err(CallError::Unreachable(crate::cira::Down::NotInstalled)),
        false,
        at(100)
      ),
      Some(ShellChild::Own(_))
    ));
    assert_eq!(pane.run(), ShellRun::Own);
    assert!(!pane.ask("c-1", at(200)), "jc's own shell runs until it exits");
    assert_eq!(pane.exited(Some(0), Duration::from_secs(60), at(300), None), None);
    assert!(pane.ask("c-1", at(301)), "then Cira is asked again");
  }

  #[test]
  fn a_shared_client_that_keeps_failing_gives_way_to_jcs_own_shell_saying_why() {
    let argv = ["/t/tmux", "attach-session", "-t", "=s-c-1"];
    let mut pane = ShellPane::default();
    let mut t = 100;
    let mut fail = |pane: &mut ShellPane| {
      assert!(pane.ask("c-1", at(t + 60)), "asks after its pause");
      pane.answered("c-1", Ok(listed_shell(&argv)), false, at(t + 60));
      t += 120;
      pane.exited(Some(1), Duration::from_millis(50), at(t), Some("server exited unexpectedly"))
    };
    for _ in 1..SHARED_FAILURES {
      assert_eq!(fail(&mut pane), None);
    }
    match fail(&mut pane) {
      Some(ShellChild::Own(why)) => {
        assert!(why.contains("server exited unexpectedly"), "{why}");
        assert!(why.contains(&SHARED_FAILURES.to_string()), "{why}");
      }
      other => panic!("expected jc's own shell, got {other:?}"),
    }
    assert_eq!(pane.run(), ShellRun::Own);
    // A client that ran a while, or Jay's `exit`, starts the count again.
    let mut pane = ShellPane::default();
    for (code, lived) in
      [(Some(1), 50), (Some(0), 50), (Some(1), 50), (Some(1), 60_000), (Some(1), 50)]
    {
      assert!(pane.ask("c-1", at(t + 60)));
      pane.answered("c-1", Ok(listed_shell(&argv)), false, at(t + 60));
      t += 120;
      assert_eq!(
        pane.exited(code, Duration::from_millis(lived), at(t), None),
        None,
        "{code:?} {lived}"
      );
    }
  }

  #[test]
  fn a_marker_asks_cira_only_when_it_changes() {
    use Mark::*;
    assert_eq!(mark_request(Some(Active), Disabled), Some(MarkRequest::Stop));
    assert_eq!(mark_request(Some(Active), Deleted), Some(MarkRequest::Remove));
    assert_eq!(mark_request(Some(Disabled), Deleted), Some(MarkRequest::Remove));
    // The same marker again (every JC start with a `[D]` heading) asks nothing.
    assert_eq!(mark_request(Some(Disabled), Disabled), None);
    assert_eq!(mark_request(Some(Deleted), Deleted), None);
    // Back to active: attaching resumes it; removal is terminal.
    assert_eq!(mark_request(Some(Disabled), Active), None);
    assert_eq!(mark_request(Some(Deleted), Disabled), None);
    // No record (a first start, or a `sessions.toml` that did not parse):
    // nothing to compare, so no `[D]` or `[DELETED]` heading asks anything.
    assert_eq!(mark_request(None, Disabled), None);
    assert_eq!(mark_request(None, Deleted), None);
  }

  #[test]
  fn the_side_that_moved_wins_and_cira_wins_a_tie() {
    assert_eq!(label_sync("A", "c", Some("A"), Some("old")), LabelSync::Agreed);
    assert_eq!(label_sync("B", "c", Some("A"), Some("A")), LabelSync::Send, "renamed in TODO.md");
    assert_eq!(
      label_sync("A", "c", Some("B"), Some("A")),
      LabelSync::Write("B".into()),
      "on the page"
    );
    assert_eq!(label_sync("C", "c", Some("B"), Some("A")), LabelSync::Write("B".into()), "both");
    assert_eq!(label_sync("C", "c", Some("B"), None), LabelSync::Write("B".into()), "no record");
    assert_eq!(label_sync("C", "c", None, None), LabelSync::Send, "Cira has no label");
  }

  /// Review P3: a heading jcc added under the session's id, because Cira has
  /// no label for it or one that cannot be a heading, is what jcc shows for
  /// Cira's label, not a rename to send back.
  #[test]
  fn a_fallback_heading_is_not_sent_as_a_rename() {
    assert_eq!(label_sync("c-1", "c-1", None, None), LabelSync::Agreed, "no label");
    assert_eq!(
      label_sync("c-1", "c-1", Some("[DELETED] x"), Some("[DELETED] x")),
      LabelSync::Agreed
    );
    assert_eq!(label_sync("c-1", "c-1", Some("[DELETED] x"), None), LabelSync::Agreed);
    assert_eq!(label_sync("mine", "c-1", None, None), LabelSync::Send, "Jay named it");
    assert_eq!(label_sync("c-1", "c-1", Some("page"), None), LabelSync::Write("page".into()));
  }

  #[test]
  fn a_session_made_while_jc_heard_no_answer_is_found_by_its_label() {
    let status = |sessions: serde_json::Value| {
      let raw = serde_json::json!({ "projects": [{ "name": "p", "sessions": sessions }] });
      Status::parse(raw.to_string().as_bytes()).unwrap()
    };
    let s = |id: &str, label: &str, extra: serde_json::Value| {
      let mut v = serde_json::json!({
        "id": id, "label": label, "state": "idle", "state_since": "1970-01-01T00:01:00Z", "owned": true
      });
      for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
      }
      v
    };
    let st = status(serde_json::json!([
      s("c-named", "New Session", serde_json::json!({})),
      s("c-gone", "New Session", serde_json::json!({ "removed_at": STOPPED_AT_200 })),
      s("c-other", "Other", serde_json::json!({})),
      s("c-lost", "New Session", serde_json::json!({})),
    ]));
    let named = |id: &str| id == "c-named";
    let found = lost_session(&st.projects[0], "New Session", named).map(|s| s.id.as_str());
    assert_eq!(found, Some("c-lost"), "owned, not removed, named by no heading");
    assert_eq!(lost_session(&st.projects[0], "Missing", named).map(|s| s.id.as_str()), None);
  }

  #[test]
  fn records_round_trip_through_toml() {
    let mut records = Records::default();
    records.sessions.insert(
      "c-1".into(),
      Record {
        mark: Some(Mark::Disabled),
        label: Some("a \"quoted\" label".into()),
        attached_at: Some(1_700_000_000_123),
      },
    );
    records.focus = Some(Focus { project: "/w/b".into(), session: Some("c-1".into()) });
    records.sessions.insert("c-2".into(), Record::default());
    let text = toml::to_string_pretty(&records).unwrap();
    assert_eq!(toml::from_str::<Records>(&text).unwrap(), records, "{text}");
    assert_eq!(toml::from_str::<Records>("").unwrap(), Records::default());
  }
}
