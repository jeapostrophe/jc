//! What jc does about a session Cira owns (Cira DESIGN 12, PLAN TMUX-01): when
//! a Claude pane attaches, waits or holds ([`pane_action`]), what a changed
//! TODO.md marker asks of Cira ([`mark_request`]), and which side of a label
//! moved ([`label_sync`]). Pure: the workspace feeds them `cira status --json`
//! and what it remembers ([`Records`], `sessions.toml` in jc's config dir).

use crate::cira::{CallError, JobState, ModePending, Session, SessionShell, exit};
use crate::todo::Mark;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
    return wait("Cira is unreachable; this pane attaches once it answers");
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
    JobState::Absent => PaneAction::Hold(format!("its job {} is gone", job.short)),
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

/// The heading says `heading`, Cira says `cira`, and the two last agreed on
/// `last`. The label is Cira's (Q-159), so when both moved, or jc has no
/// record of them agreeing, Cira's wins.
pub fn label_sync(heading: &str, cira: Option<&str>, last: Option<&str>) -> LabelSync {
  match cira {
    Some(cira) if cira == heading => LabelSync::Agreed,
    None => LabelSync::Send,
    Some(cira) if last == Some(cira) => LabelSync::Send,
    Some(cira) => LabelSync::Write(cira.to_string()),
  }
}

/// What jc remembers per Cira session id across restarts, so a marker edit
/// or a rename made while it was closed is acted on at its next start.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Records {
  pub sessions: BTreeMap<String, Record>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
  /// The marker jc last acted on.
  pub mark: Option<Mark>,
  /// The label the heading and Cira last agreed on.
  pub label: Option<String>,
}

#[cfg(test)]
mod tests {
  use super::*;
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
    assert_eq!(pane_action(Some(&s), true, None, None), PaneAction::Hold("held".into()));
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
      shell_child(Err(CallError::Unreachable), false),
      ShellChild::Own("Cira is unreachable".into())
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
      pane.answered("c-1", Err(CallError::Unreachable), false, at(100)),
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
    assert_eq!(label_sync("A", Some("A"), Some("old")), LabelSync::Agreed);
    assert_eq!(label_sync("B", Some("A"), Some("A")), LabelSync::Send, "renamed in TODO.md");
    assert_eq!(label_sync("A", Some("B"), Some("A")), LabelSync::Write("B".into()), "on the page");
    assert_eq!(label_sync("C", Some("B"), Some("A")), LabelSync::Write("B".into()), "both");
    assert_eq!(label_sync("C", Some("B"), None), LabelSync::Write("B".into()), "no record");
    assert_eq!(label_sync("C", None, None), LabelSync::Send, "Cira has no label");
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
      Record { mark: Some(Mark::Disabled), label: Some("a \"quoted\" label".into()) },
    );
    records.sessions.insert("c-2".into(), Record::default());
    let text = toml::to_string_pretty(&records).unwrap();
    assert_eq!(toml::from_str::<Records>(&text).unwrap(), records, "{text}");
    assert_eq!(toml::from_str::<Records>("").unwrap(), Records::default());
  }
}
