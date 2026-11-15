//! jc as a client of Cira (the mailbox between Jay and his sessions, which owns
//! each session's lifecycle: Cira DESIGN 12). jc reads each project's pending
//! counts and each session's state and lifecycle from `cira status --json`,
//! polled on a thread of its own ([`poll`]), and asks for every change with
//! `cira session new|mode|stop|rm|label` ([`Cira::new_session`] and its
//! siblings).
//!
//! Cira's JSON is a contract whose fields are added, never renamed or dropped
//! (Cira's `FOR-AGENTS.md`), so unknown fields are ignored and a state this
//! build does not know reads as [`State::Unknown`].
//!
//! [`marker`] turns that state, and jc's own hooks ([`HookState`]), into the
//! session picker's mark.

use crate::hooks::HookEventKind;
use serde::Deserialize;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// `cira status --json`, as much of it as jc uses.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Status {
  /// The store's directory (`CIRA_HOME`, else `~/.cira`), as the daemon
  /// resolved it.
  #[serde(default)]
  pub home: Option<PathBuf>,
  #[serde(default)]
  pub projects: Vec<Project>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Project {
  pub name: String,
  #[serde(default)]
  pub path: Option<PathBuf>,
  #[serde(default)]
  pub open: OpenCounts,
  #[serde(default)]
  pub sessions: Vec<Session>,
}

/// What is waiting on Jay in a project.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct OpenCounts {
  #[serde(default)]
  pub blocks: u64,
  #[serde(default)]
  pub questions: u64,
  #[serde(default)]
  pub notifications: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Session {
  /// Its Cira id: the `CIRA_SESSION` Cira minted (`> cira=`), else a uuid.
  pub id: String,
  pub state: State,
  /// When it entered `state`, RFC 3339.
  pub state_since: String,
  /// The background short id its hooks last reported.
  #[serde(default)]
  pub short: Option<String>,
  #[serde(default)]
  pub uuid: Option<String>,
  /// Its name, Cira's (Q-159); jc shows it and sends renames.
  #[serde(default)]
  pub label: Option<String>,
  /// Cira runs its job: it was made by `cira session new`.
  #[serde(default)]
  pub owned: bool,
  /// The desired mode: `--dangerously-skip-permissions`.
  #[serde(default)]
  pub dangerous: bool,
  /// Its hooks report bypass while Cira wants it auto.
  #[serde(default)]
  pub drift: bool,
  /// The job Cira bound to it: what `claude attach` takes.
  #[serde(default)]
  pub job: Option<Job>,
  #[serde(default)]
  pub mode_pending: ModePending,
  /// The lifecycle operation running now, or the last that failed.
  #[serde(default)]
  pub op: Option<Op>,
  /// When `cira session stop` was last asked of it, RFC 3339.
  #[serde(default)]
  pub stopped_at: Option<String>,
  /// When it was removed, RFC 3339: terminal.
  #[serde(default)]
  pub removed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Job {
  pub short: String,
  /// The flags it was started with.
  #[serde(default)]
  pub dangerous: bool,
  /// What `claude agents --json --all` last said of it.
  pub state: JobState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
  Running,
  Stopped,
  Failed,
  /// Claude Code no longer lists it: removed.
  Absent,
  /// Not listed since the daemon started, or a state added after this build.
  #[serde(other)]
  Unknown,
}

/// Whether the job owes a fork into the desired mode (Cira DESIGN 12).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModePending {
  #[default]
  None,
  WaitingForIdle,
  /// The daemon forks it now.
  Due,
  /// Deferred by a stop until the job runs again.
  Stopped,
  #[serde(other)]
  Unknown,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Op {
  /// `start`, `mode`, `stop`, `remove` or `respawn`.
  pub op: String,
  /// Running now; otherwise it failed, and `error` says why.
  pub running: bool,
  #[serde(default)]
  pub error: Option<String>,
}

/// A session's state as Cira records it (Cira DESIGN 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
  Working,
  /// At its prompt, its turn over.
  Idle,
  /// Its turn over, with background agents still running.
  Waiting,
  /// Showing a permission dialog.
  Dialog,
  Gone,
  /// A state added to Cira after this build.
  #[serde(other)]
  Unknown,
}

impl Status {
  pub fn parse(json: &[u8]) -> serde_json::Result<Self> {
    serde_json::from_slice(json)
  }

  /// The name of the Cira project that lists session `id`.
  pub fn project_of(&self, id: &str) -> Option<&str> {
    self.projects.iter().find(|p| p.sessions.iter().any(|s| s.id == id)).map(|p| p.name.as_str())
  }

  /// The session with Cira id `id`, in whichever project holds it.
  pub fn session(&self, id: &str) -> Option<&Session> {
    self.projects.iter().flat_map(|p| &p.sessions).find(|s| s.id == id)
  }

  /// The Cira project a jc project at `path` belongs to: the deepest one whose
  /// directory contains it — Cira's own rule for the project of a working
  /// directory, so a jc project inside a registered repository (a worktree,
  /// a subdirectory) counts that repository's blocks.
  /// jcc's TODO for the jc project at `path`: `todo.md` beside the scratch pad
  /// of the Cira project holding it (`<home>/projects/<name>/todo.md`; Cira
  /// DESIGN, "Scratch pads"). jcc makes it on first use.
  pub fn todo_for(&self, path: &Path) -> Option<PathBuf> {
    let name = &self.project_for(path)?.name;
    Some(self.home.as_ref()?.join("projects").join(name).join("todo.md"))
  }

  pub fn project_for(&self, path: &Path) -> Option<&Project> {
    self
      .projects
      .iter()
      .filter_map(|p| Some((p, p.path.as_deref()?)))
      .filter(|(_, dir)| path.starts_with(dir))
      .max_by_key(|(_, dir)| dir.components().count())
      .map(|(p, _)| p)
  }
}

impl Project {
  /// The session a jc session with no Cira id yet (a legacy heading) is, by
  /// its short id, else its current uuid. Empty values match nothing. One with
  /// a Cira id is found by that alone ([`Status::session`]).
  pub fn session(&self, short: &str, uuid: &str) -> Option<&Session> {
    let by = |value: &str, field: fn(&Session) -> Option<&str>| {
      (!value.is_empty()).then(|| self.sessions.iter().find(|s| field(s) == Some(value))).flatten()
    };
    by(short, |s| s.short.as_deref()).or_else(|| by(uuid, |s| s.uuid.as_deref()))
  }
}

fn rfc3339(at: &str) -> Option<SystemTime> {
  chrono::DateTime::parse_from_rfc3339(at).ok().map(SystemTime::from)
}

impl Session {
  /// `state_since` as a time, `None` when it does not parse.
  pub fn since(&self) -> Option<SystemTime> {
    rfc3339(&self.state_since)
  }

  /// `stopped_at` as a time.
  pub fn stopped(&self) -> Option<SystemTime> {
    self.stopped_at.as_deref().and_then(rfc3339)
  }

  /// A lifecycle operation is running on it now.
  pub fn op_running(&self) -> bool {
    self.op.as_ref().is_some_and(|o| o.running)
  }

  /// Why its last operation failed, if it did.
  pub fn op_error(&self) -> Option<&str> {
    self.op.as_ref().filter(|o| !o.running).and_then(|o| o.error.as_deref())
  }
}

/// The picker's per-project count, `4 blocks`: empty when there are none or
/// Cira is unreachable.
pub fn blocks_label(open: Option<OpenCounts>) -> String {
  match open.map_or(0, |o| o.blocks) {
    0 => String::new(),
    1 => "1 block".to_string(),
    n => format!("{n} blocks"),
  }
}

/// The TODO pane header's suffix, as in `TODO · 4 blocks`.
pub fn blocks_suffix(open: Option<OpenCounts>) -> String {
  let label = blocks_label(open);
  if label.is_empty() { label } else { format!(" · {label}") }
}

// ---------------------------------------------------------------------------
// The session marker
// ---------------------------------------------------------------------------

/// The Cmd-P picker's mark for a running session (Cira PLAN TMUX-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
  /// `!`: a permission dialog is waiting, seen or not.
  Dialog,
  /// `*`: it finished a turn since it was last on screen.
  Finished,
  /// `…`: its turn is over and background agents are still running.
  Waiting,
  /// `x`: the session process has ended.
  Gone,
}

impl Marker {
  pub fn glyph(self) -> &'static str {
    match self {
      Marker::Dialog => "!",
      Marker::Finished => "*",
      Marker::Waiting => "…",
      Marker::Gone => "x",
    }
  }
}

/// What jc's own hooks last said about a session: the fallback for a session
/// Cira does not know, and the only source of an API-error stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookState {
  Working,
  Idle,
  Dialog,
  /// `StopFailure`: the turn ended on an API error.
  Failed,
}

impl HookState {
  /// The state a jc hook reports, if it reports one.
  pub fn from_hook(kind: &HookEventKind) -> Option<Self> {
    match kind {
      HookEventKind::PromptSubmit => Some(HookState::Working),
      HookEventKind::Stop => Some(HookState::Idle),
      HookEventKind::PermissionPrompt => Some(HookState::Dialog),
      HookEventKind::StopFailure => Some(HookState::Failed),
      // A start or `/clear` is jc attaching or the user resetting, not a turn
      // ending; `idle_prompt` repeats a `Stop` a minute later, and taking it
      // as one would mark a session finished that finished long ago.
      HookEventKind::SessionStart
      | HookEventKind::SessionClear { .. }
      | HookEventKind::IdlePrompt => None,
    }
  }

  /// A turn ended: `Stop` or `StopFailure`.
  fn ends_turn(self) -> bool {
    matches!(self, HookState::Idle | HookState::Failed)
  }
}

/// The hook state after `next` arrives at `at`, from `last`. A turn end is
/// always taken with its own time — two turns can end in a row, the second
/// one automatic, once background agents report — while any other state keeps
/// the time it began.
pub fn note_hook(
  last: Option<(HookState, SystemTime)>,
  next: HookState,
  at: SystemTime,
) -> (HookState, SystemTime) {
  match last {
    Some((state, began)) if state == next && !next.ends_turn() => (state, began),
    _ => (next, at),
  }
}

/// The marker for a session, from Cira's view of it (`cira`: its state and
/// since when), jc's hooks (`hook`: the last state they reported and when),
/// and when it was last on screen (`seen`).
///
/// Cira's state wins, but for a turn end jc's hooks saw after it: Cira missed
/// that one (it was down, or the turn ended on an API error, which Cira has no
/// state for). Cira's `waiting` and `gone` still win, since the same `Stop`
/// reached Cira, which alone knows about background agents and process ends.
/// `*` needs a turn end after `seen`; with `seen` unknown nothing is `*`.
/// From hooks alone, `!` needs a dialog after `seen`, because no jc hook says
/// a dialog was answered.
pub fn marker(
  cira: Option<(State, Option<SystemTime>)>,
  hook: Option<(HookState, SystemTime)>,
  seen: Option<SystemTime>,
) -> Option<Marker> {
  let after_seen = |at: SystemTime| seen.is_some_and(|seen| at > seen);
  let finished = |at: SystemTime| after_seen(at).then_some(Marker::Finished);
  let Some((state, since)) = cira else {
    return match hook? {
      (HookState::Dialog, at) => (seen.is_none() || after_seen(at)).then_some(Marker::Dialog),
      (HookState::Idle | HookState::Failed, at) => finished(at),
      (HookState::Working, _) => None,
    };
  };
  let missed_end =
    hook.filter(|&(h, at)| h.ends_turn() && since.is_none_or(|since| at > since)).map(|(_, at)| at);
  match (state, missed_end) {
    (State::Waiting, _) => Some(Marker::Waiting),
    (State::Gone, _) => Some(Marker::Gone),
    (_, Some(at)) => finished(at),
    (State::Dialog, None) => Some(Marker::Dialog),
    (State::Idle, None) => since.and_then(finished),
    (State::Working | State::Unknown, None) => None,
  }
}

// ---------------------------------------------------------------------------
// Reading Cira
// ---------------------------------------------------------------------------

/// The `cira` executable. Like `claude`, it is found on `PATH`, and it finds
/// its daemon through `CIRA_HOME` (else `~/.cira`) in jc's environment.
#[derive(Debug, Clone)]
pub struct Cira {
  program: PathBuf,
  timeout: Duration,
  /// How long a `cira session …` request may take ([`Cira::CALL`]).
  call: Duration,
}

impl Default for Cira {
  /// `cira` from `PATH`, given [`Cira::TIMEOUT`] to answer.
  fn default() -> Self {
    Self::new("cira", Self::TIMEOUT)
  }
}

impl Cira {
  /// How long a read may take before Cira counts as unreachable. `cira`
  /// itself waits 12 s for a daemon that does not accept, and the marker
  /// should fall back to jc's hooks sooner than that.
  pub const TIMEOUT: Duration = Duration::from_secs(3);

  pub fn new(program: impl Into<PathBuf>, timeout: Duration) -> Self {
    Self { program: program.into(), timeout, call: Self::CALL }
  }

  /// `cira status --json`, or why Cira was not reached: not installed, no
  /// daemon, too slow, any failure exit, or output that does not parse.
  /// Blocks for up to the timeout, so it is never called on the main thread.
  pub fn status(&self) -> Result<Status, Down> {
    let mut cmd = Command::new(&self.program);
    let (code, out, stderr) =
      run_bounded(cmd.args(["status", "--json"]), self.timeout).map_err(Down::not_started)?;
    match code {
      Some(0) => Status::parse(&out).map_err(|e| Down::Unreadable(e.to_string())),
      Some(exit::NOT_SENT) => Err(Down::NoDaemon(first_line(&stderr))),
      None => Err(Down::Silent(self.timeout)),
      code => Err(Down::Failed { code, message: first_line(&stderr) }),
    }
  }

  /// The same, with `call` for each `cira session …` request.
  pub fn with_call_timeout(self, call: Duration) -> Self {
    Self { call, ..self }
  }

  /// `cira session new`: Cira mints the session's id and starts its job.
  /// With `wait`, returns once the job has started or its start has failed.
  pub fn new_session(&self, req: &NewSession) -> Result<Created, CallError> {
    let timeout = if req.wait { Self::WAIT.max(self.call) } else { self.call };
    let out = self.call(&req.args(), timeout, None)?;
    serde_json::from_slice(&out).map_err(|e| CallError::Malformed(e.to_string()))
  }

  /// `cira project add NAME --path DIR`: register the jc project at `dir`, so
  /// jcc has a Cira project, and with it a todo.md, before its first session.
  /// The same name and path again changes nothing.
  pub fn add_project(&self, name: &str, dir: &Path) -> Result<(), CallError> {
    let path = format!("--path={}", dir.display());
    self.call(&strs(&["project", "add", "--json", &path, "--", name]), self.call, None).map(drop)
  }

  /// `cira session mode`: ask for `dangerous`, compare-and-set against the
  /// mode jc showed (`from`; exit 4 when it is no longer that).
  pub fn set_mode(&self, at: &At, id: &str, dangerous: bool, from: bool) -> Result<(), CallError> {
    self.call_at(at, mode_args(id, dangerous, from))
  }

  /// `cira session stop`.
  pub fn stop(&self, at: &At, id: &str) -> Result<(), CallError> {
    self.call_at(at, jays_args("stop", id))
  }

  /// `cira session rm`: terminal.
  pub fn remove(&self, at: &At, id: &str) -> Result<(), CallError> {
    self.call_at(at, jays_args("rm", id))
  }

  /// `cira session start` (Cira PLAN TMUX-16): run the session's job again
  /// in the mode it has. Cira lists Claude's jobs and starts a new one,
  /// continuing its conversation, if Claude no longer has it; being Jay's, it
  /// clears a stop of his.
  pub fn start(&self, at: &At, id: &str) -> Result<Started, CallError> {
    let out = self.call_at_out(at, jays_args("start", id))?;
    serde_json::from_slice(&out).map_err(|e| CallError::Malformed(e.to_string()))
  }

  /// `cira session label`: the label is Cira's (Q-159).
  pub fn rename(&self, at: &At, id: &str, label: &str) -> Result<(), CallError> {
    self.call_at(at, strs(&["session", "label", "--json", "--", id, label]))
  }

  /// `cira session shell`: the command that attaches to the session's shell
  /// in Cira's own tmux server, which Cira makes first if it has none (Cira
  /// DESIGN 12). Asked at each attach, so a shell Jay ended is made again.
  pub fn session_shell(&self, at: &At, id: &str) -> Result<SessionShell, CallError> {
    let out = self.call_at_out(at, strs(&["session", "shell", "--json", "--", id]))?;
    serde_json::from_slice(&out).map_err(|e| CallError::Malformed(e.to_string()))
  }

  /// A request about a session of `at`'s project: Cira finds a session's
  /// project by name, else by the caller's directory.
  fn call_at(&self, at: &At, args: Vec<String>) -> Result<(), CallError> {
    self.call_at_out(at, args).map(drop)
  }

  /// The same, with what it printed.
  fn call_at_out(&self, at: &At, mut args: Vec<String>) -> Result<Vec<u8>, CallError> {
    if let Some(project) = &at.project {
      args.insert(2, format!("--project={project}"));
    }
    self.call(&args, self.call, Some(&at.dir))
  }

  /// How long a request may take: `cira` waits up to 12 s for a daemon to
  /// accept and 30 s for its answer (exit 124 after that), and a margin.
  pub const CALL: Duration = Duration::from_secs(12 + 30 + 18);
  /// `session new --wait`: Cira waits two starts and a margin (its `WAIT`).
  const WAIT: Duration = Duration::from_secs(2 * 180 + 60);

  /// Run `cira <args>`, its stdout on success.
  fn call(
    &self,
    args: &[String],
    timeout: Duration,
    dir: Option<&Path>,
  ) -> Result<Vec<u8>, CallError> {
    let mut cmd = Command::new(&self.program);
    cmd.args(args);
    if let Some(dir) = dir {
      cmd.current_dir(dir);
    }
    let run = run_bounded(&mut cmd, timeout);
    let (code, stdout, stderr) = run.map_err(|e| CallError::Unreachable(Down::not_started(e)))?;
    match code {
      Some(0) => Ok(stdout),
      Some(exit::NOT_SENT) => Err(CallError::Unreachable(Down::NoDaemon(first_line(&stderr)))),
      Some(exit::NO_ANSWER) | None => Err(CallError::NoAnswer),
      Some(code) => Err(CallError::Refused { code, message: stderr, stdout }),
    }
  }
}

/// Where a request about a session is made from: the Cira project that lists
/// it, when jc knows it, and the jc project's directory, which Cira reads the
/// project from otherwise (a request names a session within a project).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct At {
  pub project: Option<String>,
  pub dir: PathBuf,
}

/// Why a `cira session …` request did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
  /// Cira was not reached, so nothing was sent: `cira` is not installed or
  /// could not be started, or no daemon accepted (exit 125).
  Unreachable(Down),
  /// Sent, with no answer (exit 124, or jc stopped waiting): it may have been
  /// recorded.
  NoAnswer,
  /// Cira answered with exit `code` (`cira_proto::exit`: 4 conflict, 5
  /// invalid, 6 not found, …); `message` is its stderr. `stdout` is what it
  /// printed anyway (`new --wait --json` prints the session when its job
  /// failed to start, exit 1).
  Refused { code: i32, message: String, stdout: Vec<u8> },
  /// It answered with output this build cannot read.
  Malformed(String),
}

impl CallError {
  /// Whether asking again is how this is answered: Cira was not reached, did
  /// not answer, or recorded nothing for a reason that passes (its own error,
  /// `not_recorded`, `version`, `read_only`). A refusal of the request itself
  /// (conflict, invalid, not found) is an answer.
  pub fn retry(&self) -> bool {
    match self {
      CallError::Unreachable(_) | CallError::NoAnswer => true,
      CallError::Refused { code, .. } => {
        matches!(*code, exit::ERROR | exit::NOT_RECORDED | exit::VERSION | exit::READ_ONLY)
      }
      CallError::Malformed(_) => false,
    }
  }
}

impl std::fmt::Display for CallError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      CallError::Unreachable(down) => down.fmt(f),
      CallError::NoAnswer => write!(f, "Cira did not answer; it may have recorded the request"),
      CallError::Refused { code, message, .. } => write!(f, "{} (exit {code})", message.trim()),
      CallError::Malformed(e) => unreadable(f, e),
    }
  }
}

/// Why a `cira` command did not reach Cira, said as what is wrong and what to
/// do: jcc shows it in one banner, and on stderr (Cira PLAN SP-03, Q-165).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Down {
  /// `cira` is not on `PATH`.
  NotInstalled,
  /// `cira` ran and said no daemon accepted (exit 125): its line, which names
  /// the socket it tried.
  NoDaemon(String),
  /// No answer within jc's timeout, or killed by a signal (`run_bounded`
  /// cannot tell them apart): no daemon (`cira` waits longer than jc does for
  /// one to accept), or a busy one.
  Silent(Duration),
  /// Any other failure: the exit code (`None`: not started) and the first
  /// line `cira` printed on stderr.
  Failed { code: Option<i32>, message: String },
  /// It answered with output this build cannot read.
  Unreadable(String),
}

impl Down {
  /// Whether `other` is this cause again. `cira`'s exit-125 line carries the
  /// milliseconds it waited, which differ between reads of one outage.
  pub fn same_cause(&self, other: &Down) -> bool {
    match (self, other) {
      (Down::NoDaemon(_), Down::NoDaemon(_)) => true,
      _ => self == other,
    }
  }

  fn not_started(e: std::io::Error) -> Self {
    match e.kind() {
      std::io::ErrorKind::NotFound => Down::NotInstalled,
      _ => Down::Failed { code: None, message: format!("`cira` could not be started: {e}") },
    }
  }
}

/// How Jay installs Cira, which also starts its daemon (launchd agent
/// `local.cira`): Cira's README.
const INSTALL: &str = "`./make.sh install` in ~/Dev/cira installs it and starts its daemon";

impl std::fmt::Display for Down {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Down::NotInstalled => {
        write!(f, "Cira isn't installed: `cira` was not found on PATH. {INSTALL}")
      }
      Down::NoDaemon(line) => write!(f, "Cira isn't running: {line}"),
      Down::Silent(t) => write!(
        f,
        "Cira isn't answering: `cira` gave no answer within {} s or was killed; its daemon may \
         not be running, or be busy. {INSTALL}",
        t.as_secs_f32()
      ),
      Down::Failed { code: Some(code), message } => {
        write!(f, "Cira failed: `cira` gave exit {code}: {message}")
      }
      Down::Failed { code: None, message } => write!(f, "Cira failed: {message}"),
      Down::Unreadable(e) => unreadable(f, e),
    }
  }
}

fn unreadable(f: &mut std::fmt::Formatter<'_>, e: &str) -> std::fmt::Result {
  write!(f, "Cira's answer did not parse: {e}")
}

fn first_line(text: &str) -> String {
  text.trim().lines().next().unwrap_or_default().to_string()
}

/// Whether jcc's last `cira status` reached Cira, and if not, why: what its
/// one Cira banner says.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum Reach {
  /// No read has finished yet.
  #[default]
  Unknown,
  Up,
  Down(Down),
}

impl Reach {
  /// Take a read's outcome. Returns whether what jcc shows changed: a poll
  /// that fails for the same cause again changes nothing, so it raises
  /// nothing new.
  pub fn read(&mut self, read: &Result<Status, Down>) -> bool {
    let same = match (&*self, read) {
      (Reach::Up, Ok(_)) => true,
      (Reach::Down(was), Err(now)) => was.same_cause(now),
      _ => false,
    };
    if !same {
      *self = match read {
        Ok(_) => Reach::Up,
        Err(down) => Reach::Down(down.clone()),
      };
    }
    !same
  }

  pub fn is_up(&self) -> bool {
    *self == Reach::Up
  }

  pub fn down(&self) -> Option<&Down> {
    match self {
      Reach::Down(down) => Some(down),
      _ => None,
    }
  }
}

/// Exit codes of Cira's contract jc acts on (Cira FOR-AGENTS "Exit codes").
pub mod exit {
  /// `session new --wait`: the job did not start (stdout still has the session).
  pub const ERROR: i32 = 1;
  /// `--from` is no longer the session's mode.
  pub const CONFLICT: i32 = 4;
  /// Refused, e.g. `--resume` of a conversation with no transcript, or a
  /// removed session asked to stop.
  pub const INVALID: i32 = 5;
  pub const NOT_FOUND: i32 = 6;
  /// The store failed and nothing was written.
  pub const NOT_RECORDED: i32 = 7;
  /// The daemon does not speak this request (another build).
  pub const VERSION: i32 = 8;
  /// The project refuses writes until a remedy is done.
  pub const READ_ONLY: i32 = 9;
  /// Sent, no answer: may have been recorded.
  pub const NO_ANSWER: i32 = 124;
  /// No daemon accepted: nothing was sent.
  pub const NOT_SENT: i32 = 125;
}

/// A `cira session new` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSession {
  pub cwd: PathBuf,
  pub label: String,
  /// Continue this Claude conversation (migrating a `> uuid=` heading).
  pub resume: Option<String>,
  pub dangerous: bool,
  pub wait: bool,
}

impl NewSession {
  /// A session Jay starts from jcc (New session, a transcript or an unbound
  /// heading adopted), in `cwd`, continuing `resume` if given (then waiting
  /// for its job): dangerous, Jay's default (Q-168: "I actually prefer
  /// everything to be dangerous unless specifically choosing auto").
  pub fn for_jay(cwd: PathBuf, label: String, resume: Option<String>) -> Self {
    let wait = resume.is_some();
    Self { cwd, label, resume, dangerous: true, wait }
  }

  /// The arguments after `cira`. Values go as `--opt=value`, so a label that
  /// starts with `-` is never read as a flag. The mode is always named: Cira
  /// starts a session dangerous unless asked for auto, and refuses a
  /// `--resume` that names none (Cira PLAN SES-04). Starting a dangerous
  /// session is Jay's (Q-171), and jc acts for Jay, so it says `--from-jay`.
  pub fn args(&self) -> Vec<String> {
    let mut args = strs(&["session", "new", "--json"]);
    args.push(format!("--cwd={}", self.cwd.display()));
    args.push(format!("--label={}", self.label));
    if let Some(uuid) = &self.resume {
      args.push(format!("--resume={uuid}"));
    }
    if self.dangerous {
      args.extend(strs(&["--dangerous", "--from-jay"]));
    } else {
      args.push("--auto".into());
    }
    if self.wait {
      args.push("--wait".into());
    }
    args
  }
}

/// `cira session new --json`'s answer, as much as jc uses.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Created {
  pub session: String,
  /// Under `--wait`: the session as `cira status --json` lists it.
  #[serde(default)]
  pub status: Option<Session>,
}

/// `cira session start --json`'s answer, as much as jc uses.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Started {
  pub next: NextJob,
  /// The job bound when Cira answered.
  #[serde(default)]
  pub job: Option<String>,
}

/// What becomes of a session's job (Cira's `cira_proto::session::NextJob`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NextJob {
  /// No job is bound, or its start failed: one starts.
  Starts,
  /// Cira lists Claude's jobs, and starts a new one if the bound one is gone.
  StartsIfGone,
  /// Jay stopped it and the caller is not Jay (never jc, which says
  /// `--from-jay`): nothing starts.
  Stopped,
  /// Another session holds its job or continues it: nothing starts.
  Held,
  /// Cira does not run its job: nothing starts.
  Nothing,
  #[serde(other)]
  Unknown,
}

impl Started {
  /// What Cira does, for the pane.
  pub fn said(&self) -> String {
    let job = self.job.as_deref().unwrap_or("(none)");
    match self.next {
      NextJob::Starts => "Cira starts its job".into(),
      NextJob::StartsIfGone => format!(
        "Cira looks at Claude's jobs and starts a new job, continuing its conversation, if job \
         {job} is gone"
      ),
      NextJob::Stopped => "Cira keeps it stopped, so nothing starts".into(),
      NextJob::Held => {
        "another session holds its job or continues it (Cira's page names it), so nothing starts"
          .into()
      }
      NextJob::Nothing => "Cira does not run its job, so nothing starts".into(),
      NextJob::Unknown => "Cira did not say what becomes of its job".into(),
    }
  }
}

/// `cira session shell --json`'s answer, as much as jc uses.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SessionShell {
  /// What attaches to it, run as an argument list and never through a shell:
  /// its `-t =s-<id>` would be a zsh `=` expansion.
  pub argv: Vec<String>,
}

fn strs(v: &[&str]) -> Vec<String> {
  v.iter().map(|s| s.to_string()).collect()
}

/// `session mode|stop|rm` change a session's job, which is Jay's (Q-157): jc
/// is Jay's client, so it says `--from-jay`.
fn jays_args(verb: &str, id: &str) -> Vec<String> {
  strs(&["session", verb, "--json", "--from-jay", "--", id])
}

fn mode_args(id: &str, dangerous: bool, from: bool) -> Vec<String> {
  let mode = |d: bool| if d { "dangerous" } else { "auto" };
  let mut args = strs(&["session", "mode", "--json", "--from-jay"]);
  args.push(format!("--from={}", mode(from)));
  args.extend(strs(&["--", id, mode(dangerous)]));
  args
}

/// Run `cmd` with stdin closed, killing it after `timeout`: its exit code
/// (`None` when killed or signalled), stdout and stderr; an error when it
/// could not be started.
pub(crate) fn run_bounded(
  cmd: &mut Command,
  timeout: Duration,
) -> std::io::Result<(Option<i32>, Vec<u8>, String)> {
  let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
  // Read on threads of their own, so a large answer cannot fill a pipe and
  // stall the child while this one waits for it to exit.
  let drain = |pipe: Option<Box<dyn Read + Send>>| {
    std::thread::spawn(move || {
      let mut out = Vec::new();
      if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut out);
      }
      out
    })
  };
  let stdout = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
  let stderr = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
  let deadline = Instant::now() + timeout;
  let exit = loop {
    match child.try_wait() {
      Ok(Some(exit)) => break Some(exit),
      Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
      _ => {
        let _ = child.kill();
        let _ = child.wait();
        break None;
      }
    }
  };
  let stdout = stdout.join().unwrap_or_default();
  let stderr = String::from_utf8_lossy(&stderr.join().unwrap_or_default()).into_owned();
  Ok((exit.and_then(|e| e.code()), stdout, stderr))
}

/// Call `read` every `every`, on a thread of its own, and send each result.
/// The caller's thread never runs `read`. Reads never overlap: the next waits
/// for the last and then `every`, or less once something is sent on the
/// returned kick (a read wanted now: a Claude pane's attach client exited).
/// The thread ends once the receiver is dropped.
pub fn poll<T: Send + 'static>(
  every: Duration,
  mut read: impl FnMut() -> T + Send + 'static,
) -> (flume::Receiver<T>, flume::Sender<()>) {
  let (tx, rx) = flume::bounded(1);
  let (kick, kicked) = flume::unbounded();
  std::thread::Builder::new()
    .name("jc-cira-poll".into())
    .spawn(move || {
      while tx.send(read()).is_ok() {
        match kicked.recv_timeout(every) {
          Ok(()) => while kicked.try_recv().is_ok() {},
          Err(flume::RecvTimeoutError::Timeout) => {}
          Err(flume::RecvTimeoutError::Disconnected) => std::thread::sleep(every),
        }
      }
    })
    .expect("spawning the Cira poll thread");
  (rx, kick)
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::Value;

  /// Captured from a real daemon by `tests/fixtures/cira/make-status.sh`.
  const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/cira/status.json");

  fn fixture() -> (Status, Value) {
    (Status::parse(FIXTURE).unwrap(), serde_json::from_slice(FIXTURE).unwrap())
  }

  #[test]
  fn jccs_todo_is_in_the_store_cira_names() {
    let (status, raw) = fixture();
    let home = PathBuf::from(raw["home"].as_str().unwrap());
    let project = &raw["projects"][0];
    let dir = PathBuf::from(project["path"].as_str().unwrap());
    let name = project["name"].as_str().unwrap();
    assert_eq!(status.todo_for(&dir), Some(home.join("projects").join(name).join("todo.md")));
    assert_eq!(status.todo_for(Path::new("/nowhere")), None);
  }

  #[test]
  fn parses_real_status_json() {
    let (status, raw) = fixture();
    let raw_projects = raw["projects"].as_array().unwrap();
    assert_eq!(status.projects.len(), raw_projects.len());
    for (p, rp) in status.projects.iter().zip(raw_projects) {
      assert_eq!(p.path.as_deref().and_then(Path::to_str), rp["path"].as_str());
      for (field, n) in [
        ("blocks", p.open.blocks),
        ("questions", p.open.questions),
        ("notifications", p.open.notifications),
      ] {
        assert_eq!(Some(n), rp["open"][field].as_u64(), "{}: open.{field}", p.name);
      }
      let raw_sessions = rp["sessions"].as_array().unwrap();
      assert_eq!(p.sessions.len(), raw_sessions.len());
      for (s, rs) in p.sessions.iter().zip(raw_sessions) {
        assert_eq!(s.short.as_deref(), rs["short"].as_str());
        assert_eq!(s.uuid.as_deref(), rs["uuid"].as_str());
        assert!(s.since().is_some(), "{}: state_since {:?}", s.id, s.state_since);
        // The generator names each hook-driven session after the state it
        // drove it to; the `owned` project's are Cira's own.
        let Some(driven) = s.id.strip_prefix("fx-") else { continue };
        let state: State = serde_json::from_value(Value::String(driven.to_string())).unwrap();
        assert_eq!(s.state, state, "{}", s.id);
        assert_ne!(s.state, State::Unknown, "{}", s.id);
      }
    }
    // Every state Cira has is in the fixture, so each one's parse is checked.
    let mut states: Vec<_> = status
      .projects
      .iter()
      .flat_map(|p| &p.sessions)
      .filter(|s| s.id.starts_with("fx-"))
      .map(|s| s.state)
      .collect();
    states.sort_by_key(|s| *s as u8);
    states.dedup();
    assert_eq!(states.len(), 5, "{states:?}");
    assert!(status.projects.iter().any(|p| p.open.blocks > 0));
  }

  /// The fixture's session labelled `label` (the `owned` project's), and its
  /// raw JSON.
  fn owned<'a>(status: &'a Status, raw: &'a Value, label: &str) -> (&'a Session, &'a Value) {
    let s =
      status.projects.iter().flat_map(|p| &p.sessions).find(|s| s.label.as_deref() == Some(label));
    let s = s.unwrap_or_else(|| panic!("no session {label} in the fixture"));
    let r =
      raw["projects"].as_array().unwrap().iter().flat_map(|p| p["sessions"].as_array().unwrap());
    (s, r.into_iter().find(|r| r["id"] == s.id.as_str()).unwrap())
  }

  #[test]
  fn reads_the_lifecycle_of_sessions_cira_runs() {
    let (status, raw) = fixture();
    let job_state =
      |r: &Value| -> JobState { serde_json::from_value(r["job"]["state"].clone()).unwrap() };
    for label in ["owned-running", "owned-dangerous", "owned-stopped", "owned-removed"] {
      let (s, r) = owned(&status, &raw, label);
      let job = s.job.as_ref().expect("a job is bound");
      assert_eq!(job.short, r["job"]["short"].as_str().unwrap(), "{label}");
      assert_eq!(job.dangerous, r["job"]["dangerous"].as_bool().unwrap(), "{label}");
      assert_eq!(job.state, job_state(r), "{label}");
      assert_ne!(job.state, JobState::Unknown, "{label}");
      assert_eq!(s.dangerous, r["dangerous"].as_bool().unwrap(), "{label}");
      assert_eq!(s.stopped_at.as_deref(), r["stopped_at"].as_str(), "{label}");
      assert_eq!(s.removed_at.as_deref(), r["removed_at"].as_str(), "{label}");
      assert_eq!(s.op_running(), r["op"]["running"].as_bool().unwrap_or(false), "{label}");
      assert_eq!(status.session(&s.id).map(|x| &x.id), Some(&s.id));
      assert_eq!(status.project_of(&s.id), Some("owned"));
    }
    // What the generator did to each, as Cira reports it.
    assert!(owned(&status, &raw, "owned-dangerous").0.dangerous);
    assert!(owned(&status, &raw, "owned-stopped").0.stopped().is_some());
    assert!(owned(&status, &raw, "owned-removed").0.removed_at.is_some());
    assert_eq!(
      owned(&status, &raw, "owned-running").0.job.as_ref().unwrap().state,
      JobState::Running
    );
  }

  #[test]
  fn an_unknown_state_and_unknown_fields_still_parse() {
    let (_, mut raw) = fixture();
    raw["new_field"] = Value::Bool(true);
    raw["projects"][0]["sessions"][0]["state"] = Value::String("hibernating".into());
    let status = Status::parse(raw.to_string().as_bytes()).unwrap();
    assert_eq!(status.projects[0].sessions[0].state, State::Unknown);
  }

  #[test]
  fn a_jc_project_belongs_to_the_deepest_cira_project_containing_it() {
    let (status, _) = fixture();
    let mut dirs: Vec<&Path> = status
      .projects
      .iter()
      .filter(|p| p.name != "owned")
      .filter_map(|p| p.path.as_deref())
      .collect();
    dirs.sort_by_key(|d| d.components().count());
    let [outer, inner] = dirs[..] else { panic!("fixture has two nested projects: {dirs:?}") };
    assert!(inner.starts_with(outer));
    let name = |p: &Path| status.project_for(p).map(|p| p.path.as_deref().unwrap());
    assert_eq!(name(outer), Some(outer));
    assert_eq!(name(&outer.join("elsewhere")), Some(outer));
    assert_eq!(name(inner), Some(inner));
    assert_eq!(name(&inner.join("sub/dir")), Some(inner));
    assert_eq!(name(outer.parent().unwrap()), None);
    // A sibling whose name merely starts with the project's is not inside it.
    let sibling = PathBuf::from(format!("{}-other", outer.display()));
    assert_eq!(name(&sibling), None);
  }

  #[test]
  fn a_legacy_session_is_found_by_short_then_uuid() {
    let (status, _) = fixture();
    let p = status.projects.iter().find(|p| p.sessions.len() > 1).unwrap();
    let [a, b, ..] = &p.sessions[..] else { unreachable!() };
    let id = |s: Option<&Session>| s.map(|s| s.id.clone());
    let uuid_a = a.uuid.as_deref().unwrap();
    let (short_b, uuid_b) = (b.short.as_deref().unwrap(), b.uuid.as_deref().unwrap());
    assert_eq!(id(p.session(short_b, uuid_a)), Some(b.id.clone()), "short beats uuid");
    assert_eq!(id(p.session("", uuid_b)), Some(b.id.clone()));
    assert_eq!(id(p.session("", "")), None);
    // One with a Cira id is found by that alone, in whichever project.
    assert_eq!(status.session(&a.id).map(|s| &s.id), Some(&a.id));
    assert_eq!(status.session("not-listed"), None);
  }

  #[test]
  fn header_and_picker_counts() {
    let (status, _) = fixture();
    let open = status.projects.iter().map(|p| p.open).find(|o| o.blocks > 1).unwrap();
    assert_eq!(blocks_suffix(Some(open)), format!(" · {} blocks", open.blocks));
    assert_eq!(blocks_label(Some(open)), format!("{} blocks", open.blocks));
    assert_eq!(blocks_suffix(Some(OpenCounts { blocks: 1, ..open })), " · 1 block");
    assert_eq!(blocks_suffix(Some(OpenCounts::default())), "");
    assert_eq!(blocks_suffix(None), "");
    assert_eq!(blocks_label(None), "");
  }

  fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
  }

  #[test]
  fn cira_state_sets_the_marker() {
    let seen = Some(at(100));
    let m = |state, since| marker(Some((state, Some(at(since)))), None, seen);
    assert_eq!(m(State::Dialog, 50), Some(Marker::Dialog), "a dialog shows, seen or not");
    assert_eq!(m(State::Idle, 150), Some(Marker::Finished), "idle since after last seen");
    assert_eq!(m(State::Idle, 50), None, "idle since before it was last on screen");
    assert_eq!(m(State::Waiting, 50), Some(Marker::Waiting));
    assert_eq!(m(State::Gone, 50), Some(Marker::Gone));
    assert_eq!(m(State::Working, 150), None, "working is not a mark");
    assert_eq!(m(State::Unknown, 150), None);
    assert_eq!(marker(Some((State::Idle, Some(at(150)))), None, None), None, "never seen");
  }

  #[test]
  fn cira_wins_over_hooks_but_not_a_turn_end_it_missed() {
    let seen = Some(at(100));
    let working = Some((State::Working, Some(at(200))));
    // Hooks older than Cira's state, or not a turn end, change nothing.
    assert_eq!(marker(working, Some((HookState::Idle, at(150))), seen), None);
    assert_eq!(marker(working, Some((HookState::Failed, at(150))), seen), None);
    assert_eq!(marker(working, Some((HookState::Dialog, at(300))), seen), None);
    // An API error after Cira's last state: Cira has no state for it.
    assert_eq!(marker(working, Some((HookState::Failed, at(300))), seen), Some(Marker::Finished));
    // A Stop Cira missed (its daemon restarting): Cira still says working.
    assert_eq!(marker(working, Some((HookState::Idle, at(300))), seen), Some(Marker::Finished));
    let dialog = Some((State::Dialog, Some(at(200))));
    assert_eq!(marker(dialog, Some((HookState::Idle, at(300))), seen), Some(Marker::Finished));
    // A turn that ended while Cira was down, after it last said idle.
    let stale_idle = Some((State::Idle, Some(at(50))));
    assert_eq!(marker(stale_idle, Some((HookState::Idle, at(150))), seen), Some(Marker::Finished));
    assert_eq!(marker(stale_idle, Some((HookState::Idle, at(60))), seen), None);
    // The same Stop reached Cira, which alone knows of agents and process ends.
    let waiting = Some((State::Waiting, Some(at(200))));
    assert_eq!(marker(waiting, Some((HookState::Idle, at(201))), seen), Some(Marker::Waiting));
    let gone = Some((State::Gone, Some(at(200))));
    assert_eq!(marker(gone, Some((HookState::Idle, at(201))), seen), Some(Marker::Gone));
  }

  #[test]
  fn hooks_set_the_marker_while_cira_does_not_know_the_session() {
    let seen = Some(at(100));
    assert_eq!(marker(None, Some((HookState::Dialog, at(150))), seen), Some(Marker::Dialog));
    // No jc hook says a dialog was answered: one from before the last look
    // may be long gone.
    assert_eq!(marker(None, Some((HookState::Dialog, at(50))), seen), None);
    assert_eq!(marker(None, Some((HookState::Dialog, at(50))), None), Some(Marker::Dialog));
    assert_eq!(marker(None, Some((HookState::Idle, at(150))), seen), Some(Marker::Finished));
    assert_eq!(marker(None, Some((HookState::Failed, at(150))), seen), Some(Marker::Finished));
    assert_eq!(marker(None, Some((HookState::Idle, at(50))), seen), None);
    assert_eq!(marker(None, Some((HookState::Working, at(150))), seen), None);
    assert_eq!(marker(None, None, seen), None);
  }

  #[test]
  fn every_turn_end_is_stamped_and_other_states_keep_their_start() {
    let idle = note_hook(None, HookState::Idle, at(10));
    assert_eq!(idle, (HookState::Idle, at(10)));
    // A second Stop with no prompt between (an automatic turn) is a new end.
    assert_eq!(note_hook(Some(idle), HookState::Idle, at(30)), (HookState::Idle, at(30)));
    let failed = (HookState::Failed, at(10));
    assert_eq!(note_hook(Some(failed), HookState::Failed, at(30)), (HookState::Failed, at(30)));
    // A repeated dialog or prompt keeps the time it began.
    let dialog = (HookState::Dialog, at(10));
    assert_eq!(note_hook(Some(dialog), HookState::Dialog, at(30)), dialog);
    let working = (HookState::Working, at(10));
    assert_eq!(note_hook(Some(working), HookState::Working, at(30)), working);
    assert_eq!(note_hook(Some(working), HookState::Idle, at(30)), (HookState::Idle, at(30)));
  }

  #[test]
  fn only_turn_changes_set_the_hook_state() {
    let state = |k| HookState::from_hook(&k);
    assert_eq!(state(HookEventKind::PromptSubmit), Some(HookState::Working));
    assert_eq!(state(HookEventKind::Stop), Some(HookState::Idle));
    assert_eq!(state(HookEventKind::PermissionPrompt), Some(HookState::Dialog));
    assert_eq!(state(HookEventKind::StopFailure), Some(HookState::Failed));
    // An attach resuming the session, or the reminder a minute after a stop,
    // is not a turn that finished.
    assert_eq!(state(HookEventKind::SessionStart), None);
    assert_eq!(state(HookEventKind::IdlePrompt), None);
  }

  #[test]
  fn an_unreachable_cira_says_why() {
    let missing = Cira::new("/nonexistent/jc-test/cira", Duration::from_secs(1));
    assert_eq!(missing.status(), Err(Down::NotInstalled));
    // A `cira` that exits 125, as one with no daemon does, after printing JSON.
    let failing =
      script("failing", "echo '{\"projects\":[]}'; echo 'no daemon on /h/cira.sock' >&2; exit 125");
    assert_eq!(
      Cira::new(failing, Duration::from_secs(5)).status(),
      Err(Down::NoDaemon("no daemon on /h/cira.sock".into()))
    );
    let broken = script("broken", "echo 'bad home' >&2; echo more >&2; exit 3");
    assert_eq!(
      Cira::new(broken, Duration::from_secs(5)).status(),
      Err(Down::Failed { code: Some(3), message: "bad home".into() })
    );
    let hanging = script("hanging", "exec sleep 30");
    let started = Instant::now();
    let t = Duration::from_millis(300);
    assert_eq!(Cira::new(hanging, t).status(), Err(Down::Silent(t)));
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    let garbled = script("garbled", "echo nope");
    assert!(matches!(
      Cira::new(garbled, Duration::from_secs(5)).status(),
      Err(Down::Unreadable(_))
    ));
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/cira/status.json");
    let answering = script("answering", &format!("cat '{path}'"));
    assert_eq!(Cira::new(answering, Duration::from_secs(5)).status(), Ok(fixture().0));
  }

  /// Q-165: Jay saw "Cira is unreachable" with `cira` not installed, and
  /// could not tell what was wrong or what to do.
  /// The review's P2: `cira`'s exit-125 line names the milliseconds it
  /// waited, so two reads of one outage differ only in that number.
  #[test]
  fn reads_that_differ_only_in_the_wait_are_one_cause() {
    let line = |ms: u32| format!("no daemon accepted a connection on /h/cira.sock within {ms} ms");
    let mut reach = Reach::default();
    assert!(reach.read(&Err(Down::NoDaemon(line(1500)))));
    assert!(!reach.read(&Err(Down::NoDaemon(line(1503)))), "the same cause again");
    assert!(reach.read(&Err(Down::NotInstalled)), "another cause is a change");
  }

  #[test]
  fn the_cira_down_message_names_the_cause_and_the_remedy() {
    let missing = Cira::new("/nonexistent/jc-test/cira", Duration::from_secs(1));
    let why = missing.status().unwrap_err().to_string();
    assert!(why.contains("not found on PATH"), "{why}");
    assert!(why.contains("./make.sh install"), "{why}");
    let socket = "no daemon accepted a connection on /h/cira.sock within 12000 ms";
    let why = Down::NoDaemon(socket.into()).to_string();
    assert!(why.contains(socket), "cira's own line names the socket: {why}");
    let why = Down::Silent(Cira::TIMEOUT).to_string();
    assert!(why.contains("no answer within 3 s") && why.contains("daemon"), "{why}");
    let why = Down::Failed { code: Some(3), message: "bad home".into() }.to_string();
    assert!(why.contains("exit 3") && why.contains("bad home"), "{why}");
    // A call says the same as the status read.
    let at = At { project: None, dir: std::env::temp_dir() };
    let call = missing.stop(&at, "c1").unwrap_err().to_string();
    assert_eq!(call, Down::NotInstalled.to_string());
  }

  #[test]
  fn the_poll_reads_off_the_calling_thread() {
    let caller = std::thread::current().id();
    let started = Instant::now();
    let (rx, _kick) = poll(Duration::from_millis(10), move || {
      std::thread::sleep(Duration::from_millis(200));
      std::thread::current().id()
    });
    // `poll` returns before the first (slow) read completes...
    assert!(started.elapsed() < Duration::from_millis(150), "{:?}", started.elapsed());
    // ...and the reads run elsewhere.
    for _ in 0..2 {
      let reader = rx.recv_timeout(Duration::from_secs(5)).unwrap();
      assert_ne!(reader, caller);
    }
  }

  // ---- jc as Cira's client (PLAN TMUX-01/02) ----

  fn script(name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("jc-cira-call-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
  }

  #[test]
  fn new_session_passes_values_that_cannot_be_read_as_flags() {
    let req = NewSession {
      cwd: "/p/q r".into(),
      label: "-n surprise".into(),
      resume: None,
      dangerous: false,
      wait: false,
    };
    assert_eq!(
      req.args(),
      ["session", "new", "--json", "--cwd=/p/q r", "--label=-n surprise", "--auto"],
      "an auto start names its mode (Cira SES-04: dangerous is the default) and is anyone's"
    );
    // Cira SES-04: `--resume` without a mode is refused (exit 2), so a
    // migrated heading without `> dangerous` says auto.
    let resumed = NewSession { resume: Some("u".into()), ..req.clone() };
    assert!(resumed.args().contains(&"--auto".to_string()), "{:?}", resumed.args());
    let req = NewSession {
      resume: Some("335b461a-eba5-485f-898c-f757fdb3f2bf".into()),
      dangerous: true,
      wait: true,
      ..req
    };
    let args = req.args();
    assert!(args.contains(&"--resume=335b461a-eba5-485f-898c-f757fdb3f2bf".to_string()));
    assert!(args.contains(&"--dangerous".to_string()));
    assert!(args.contains(&"--from-jay".to_string()), "a dangerous start is Jay's (Q-171)");
    assert!(args.contains(&"--wait".to_string()));
  }

  #[test]
  fn a_session_jay_starts_is_dangerous_and_his() {
    let fresh = NewSession::for_jay("/p".into(), "New Session".into(), None);
    assert_eq!(
      fresh.args(),
      ["session", "new", "--json", "--cwd=/p", "--label=New Session", "--dangerous", "--from-jay"],
      "Jay's new session started auto (Q-168: dangerous unless he chooses auto)"
    );
    let adopted = NewSession::for_jay("/p".into(), "T".into(), Some("u".into()));
    assert!(adopted.dangerous && adopted.wait, "{adopted:?}");
  }

  #[test]
  fn mode_stop_and_rm_are_jays_and_name_the_session_after_a_double_dash() {
    assert_eq!(
      mode_args("-c1", true, false),
      ["session", "mode", "--json", "--from-jay", "--from=auto", "--", "-c1", "dangerous"]
    );
    assert_eq!(mode_args("c1", false, true)[4..], ["--from=dangerous", "--", "c1", "auto"]);
    assert_eq!(jays_args("stop", "c1"), ["session", "stop", "--json", "--from-jay", "--", "c1"]);
    assert_eq!(jays_args("rm", "c1")[1], "rm");
  }

  #[test]
  fn a_call_tells_unreachable_from_refused() {
    let t = Duration::from_secs(10);
    let at = At { project: None, dir: std::env::temp_dir() };
    let gone = Cira::new("/nonexistent/jc-test/cira", t);
    assert_eq!(
      gone.stop(&at, "c1"),
      Err(CallError::Unreachable(Down::NotInstalled)),
      "not installed"
    );
    // 125: no daemon accepted; nothing was sent.
    let no_daemon = Cira::new(script("no-daemon", "echo 'no daemon' >&2; exit 125"), t);
    assert_eq!(
      no_daemon.stop(&at, "c1"),
      Err(CallError::Unreachable(Down::NoDaemon("no daemon".into())))
    );
    let conflict = Cira::new(script("conflict", "echo 'mode is dangerous' >&2; exit 4"), t);
    match conflict.set_mode(&at, "c1", true, false) {
      Err(CallError::Refused { code: exit::CONFLICT, message, .. }) => {
        assert_eq!(message.trim(), "mode is dangerous")
      }
      other => panic!("expected a conflict, got {other:?}"),
    }
    let slow =
      Cira::new(script("slow", "exec sleep 30"), t).with_call_timeout(Duration::from_millis(200));
    assert_eq!(slow.rename(&at, "c1", "x"), Err(CallError::NoAnswer), "no answer in time");
    let sent = Cira::new(script("sent", "exit 124"), t);
    assert_eq!(sent.stop(&at, "c1"), Err(CallError::NoAnswer), "124: may have been recorded");
  }

  #[test]
  fn refusals_that_record_nothing_are_asked_again() {
    let refused = |code| CallError::Refused { code, message: String::new(), stdout: vec![] };
    // Found in review: a `[D]` met by a store that could not write (7) was
    // recorded as done, and the stop never sent again.
    for code in [exit::ERROR, exit::NOT_RECORDED, exit::VERSION, exit::READ_ONLY] {
      assert!(refused(code).retry(), "exit {code}");
    }
    assert!(CallError::Unreachable(Down::NotInstalled).retry());
    assert!(CallError::NoAnswer.retry());
    for code in [exit::CONFLICT, exit::INVALID, exit::NOT_FOUND, 2] {
      assert!(!refused(code).retry(), "exit {code} answers the request");
    }
    assert!(!CallError::Malformed(String::new()).retry());
  }

  #[test]
  fn a_session_request_names_its_project_and_runs_in_the_projects_directory() {
    // Found 2026-11-12 against a dev daemon: `cira session mode|stop|rm|label`
    // resolve the session within the caller's project, and jc's own working
    // directory is in none ("inside no registered project", exit 6).
    let dir = std::env::temp_dir().join(format!("jc-cira-at-{}", std::process::id()));
    let proj = dir.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let log = dir.join("log");
    let echo = Cira::new(
      script(
        "echo-at",
        &format!("pwd -P > '{}'; printf '%s\\n' \"$@\" >> '{}'", log.display(), log.display()),
      ),
      Duration::from_secs(10),
    );
    let at = At { project: Some("jcrework".into()), dir: proj.clone() };
    echo.stop(&at, "c1").unwrap();
    let logged = std::fs::read_to_string(&log).unwrap();
    let mut lines = logged.lines();
    assert_eq!(
      lines.next(),
      Some(proj.canonicalize().unwrap().to_str().unwrap()),
      "runs in the project"
    );
    let args: Vec<&str> = lines.collect();
    assert_eq!(args, ["session", "stop", "--project=jcrework", "--json", "--from-jay", "--", "c1"]);
    let _ = std::fs::remove_dir_all(&dir);
  }

  /// Cira PLAN TMUX-16: "Start this session again" is `cira session start`,
  /// Jay's, in the project, by the session's id; its answer says what Cira
  /// does, never that a job starts when none will.
  #[test]
  fn start_asks_by_the_sessions_id_and_says_what_cira_does() {
    let dir = std::env::temp_dir().join(format!("jc-cira-start-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("log");
    let answer = r#"{"project":"p","session":"c1","dangerous":false,"recorded":true,"next":"starts_if_gone","job":"eb3c2f7e"}"#;
    let cira = Cira::new(
      script("start", &format!("printf '%s\\n' \"$@\" > '{}'; echo '{answer}'", log.display())),
      Duration::from_secs(10),
    );
    let at = At { project: Some("p".into()), dir: dir.clone() };
    let started = cira.start(&at, "c1").unwrap();
    let args = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
      args.lines().collect::<Vec<_>>(),
      ["session", "start", "--project=p", "--json", "--from-jay", "--", "c1"]
    );
    assert_eq!(started, Started { next: NextJob::StartsIfGone, job: Some("eb3c2f7e".into()) });
    assert!(
      started.said().contains("new job") && started.said().contains("eb3c2f7e"),
      "{}",
      started.said()
    );
    for (next, says) in [
      (NextJob::Starts, "starts"),
      (NextJob::Held, "nothing starts"),
      (NextJob::Nothing, "nothing starts"),
      (NextJob::Stopped, "nothing starts"),
    ] {
      let said = Started { next, job: None }.said();
      assert!(said.contains(says), "{next:?}: {said}");
    }
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn new_session_reads_the_minted_id_and_the_status_a_failed_wait_prints() {
    let t = Duration::from_secs(10);
    let made = Cira::new(
      script(
        "made",
        r#"echo '{"project":"p","session":"s-1","recorded":true,"project_created":false}'"#,
      ),
      t,
    );
    let req = NewSession {
      cwd: "/p".into(),
      label: "L".into(),
      resume: None,
      dangerous: false,
      wait: false,
    };
    assert_eq!(made.new_session(&req).unwrap().session, "s-1");
    // `--wait` whose job did not start: exit 1, the session still printed.
    let (_, raw) = fixture();
    let listed = raw["projects"][0]["sessions"][0].clone();
    let body = serde_json::json!({ "project": "p", "session": "s-2", "status": listed });
    let failed = Cira::new(script("failed", &format!("echo '{body}'; exit 1")), t);
    match failed.new_session(&NewSession { wait: true, ..req }) {
      Err(CallError::Refused { code: exit::ERROR, stdout, .. }) => {
        let created: Created = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(created.session, "s-2");
        assert!(created.status.is_some());
      }
      other => panic!("expected exit 1 with the session, got {other:?}"),
    }
  }

  #[test]
  fn session_shell_names_its_project_and_reads_the_argv_cira_answers() {
    let dir = std::env::temp_dir().join(format!("jc-cira-shell-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("log");
    // A real answer (`tests/fixtures/cira/make-shell.sh`).
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/cira/session-shell.json");
    let raw: Value = serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
    let cira = Cira::new(
      script("shell", &format!("printf '%s\\n' \"$@\" > '{}'; cat '{fixture}'", log.display())),
      Duration::from_secs(10),
    );
    let at = At { project: Some("p".into()), dir: dir.clone() };
    let shell = cira.session_shell(&at, "-c1").unwrap();
    let logged = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
      logged.lines().collect::<Vec<_>>(),
      ["session", "shell", "--project=p", "--json", "--", "-c1"],
      "the session after a double dash, in its project"
    );
    let want: Vec<&str> =
      raw["argv"].as_array().unwrap().iter().map(|w| w.as_str().unwrap()).collect();
    assert_eq!(shell.argv, want, "argv kept word for word");
    let removed = Cira::new(
      script("shell-removed", "echo 'session c-1 is removed' >&2; exit 5"),
      Duration::from_secs(10),
    );
    assert!(matches!(
      removed.session_shell(&at, "c-1"),
      Err(CallError::Refused { code: exit::INVALID, .. })
    ));
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn a_kick_starts_the_next_read_at_once() {
    let started = Instant::now();
    let (rx, kick) = poll(Duration::from_secs(60), Instant::now);
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    kick.send(()).unwrap();
    rx.recv_timeout(Duration::from_secs(5)).expect("a second read well before 60 s");
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
  }
}
