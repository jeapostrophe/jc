//! What jc reads from Cira (the mailbox between Jay and his sessions): each
//! project's pending counts and each session's state, from `cira status
//! --json`, polled on a thread of its own ([`poll`]).
//!
//! Cira's JSON is a contract whose fields are added, never renamed or dropped
//! (Cira's `FOR-AGENTS.md`), so unknown fields are ignored and a state this
//! build does not know reads as [`State::Unknown`].
//!
//! The session marker ([`marker`]) comes from Cira's state when Cira knows the
//! session, and from jc's own hooks ([`HookState`]) when it does not: while
//! Cira is unreachable, for a session started without a `CIRA_SESSION`, and
//! for an API-error stop, which Cira has no state for.

use crate::hooks::HookEventKind;
use serde::Deserialize;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// `cira status --json`, as much of it as jc uses.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Status {
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
  /// Its Cira id: the `CIRA_SESSION` jc passed (`> cira=`), else a uuid.
  pub id: String,
  pub state: State,
  /// When it entered `state`, RFC 3339.
  pub state_since: String,
  /// Its background short id (`> bg=`), once reported.
  #[serde(default)]
  pub short: Option<String>,
  #[serde(default)]
  pub uuid: Option<String>,
}

/// A session's state as Cira records it (Cira DESIGN 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
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

  /// The Cira project a jc project at `path` belongs to: the deepest one whose
  /// directory contains it — Cira's own rule for the project of a working
  /// directory, so a jc project inside a registered repository (a worktree,
  /// a subdirectory) counts that repository's blocks.
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
  /// The session a jc session is: by its stable id (`> cira=`), else its short
  /// id, else its current uuid. Empty values match nothing.
  pub fn session(&self, cira: Option<&str>, short: &str, uuid: &str) -> Option<&Session> {
    let some = |s: &str| (!s.is_empty()).then_some(s.to_string());
    let (short, uuid) = (some(short), some(uuid));
    cira
      .filter(|c| !c.is_empty())
      .and_then(|c| self.sessions.iter().find(|s| s.id == c))
      .or_else(|| short.and_then(|v| self.sessions.iter().find(|s| s.short.as_ref() == Some(&v))))
      .or_else(|| uuid.and_then(|v| self.sessions.iter().find(|s| s.uuid.as_ref() == Some(&v))))
  }
}

impl Session {
  /// `state_since` as a time, `None` when it does not parse.
  pub fn since(&self) -> Option<SystemTime> {
    chrono::DateTime::parse_from_rfc3339(&self.state_since).ok().map(SystemTime::from)
  }
}

/// `TODO · 4 blocks`: the TODO pane header's suffix for a project's open
/// blocks, empty when there are none or Cira is unreachable.
pub fn blocks_suffix(open: Option<OpenCounts>) -> String {
  match open.map_or(0, |o| o.blocks) {
    0 => String::new(),
    1 => " · 1 block".to_string(),
    n => format!(" · {n} blocks"),
  }
}

/// The picker's per-project count: `4 blocks`, empty when none.
pub fn blocks_label(open: Option<OpenCounts>) -> String {
  blocks_suffix(open).trim_start_matches(" · ").to_string()
}

// ---------------------------------------------------------------------------
// The session marker
// ---------------------------------------------------------------------------

/// The Cmd-P picker's mark for a running session (Cira PLAN TMUX-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Marker {
  /// `!`: a permission dialog is waiting, seen or not. Sorts first.
  Dialog,
  /// `*`: it finished a turn since it was last on screen.
  Finished,
  /// `~`: its turn is over and background agents are still running.
  Waiting,
  /// `x`: the session process has ended.
  Gone,
}

impl Marker {
  pub fn glyph(self) -> &'static str {
    match self {
      Marker::Dialog => "!",
      Marker::Finished => "*",
      Marker::Waiting => "~",
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

/// The marker for a session, from Cira's view of it (`cira`: its state and
/// since when), jc's hooks (`hook`: the last state they reported and when),
/// and when it was last on screen (`seen`).
///
/// Cira wins, except for an API-error stop newer than Cira's state: Cira has
/// no state for one and still says `working`. An idle session is `*` only if
/// it went idle after `seen`; with `seen` unknown it is not marked. When both
/// say idle, the later time counts: a turn that ended while Cira was down is
/// one only jc's hooks saw.
pub fn marker(
  cira: Option<(State, Option<SystemTime>)>,
  hook: Option<(HookState, SystemTime)>,
  seen: Option<SystemTime>,
) -> Option<Marker> {
  let finished = |at: Option<SystemTime>| match (at, seen) {
    (Some(at), Some(seen)) if at > seen => Some(Marker::Finished),
    _ => None,
  };
  let from_hook = |(state, at): (HookState, SystemTime)| match state {
    HookState::Dialog => Some(Marker::Dialog),
    HookState::Idle | HookState::Failed => finished(Some(at)),
    HookState::Working => None,
  };
  match (cira, hook) {
    (Some((_, since)), Some(h @ (HookState::Failed, at)))
      if since.is_none_or(|since| at > since) =>
    {
      from_hook(h)
    }
    (Some((state, since)), _) => match state {
      State::Dialog => Some(Marker::Dialog),
      State::Idle => {
        let hook_idle = hook.filter(|(h, _)| *h == HookState::Idle).map(|(_, at)| at);
        finished(since.max(hook_idle))
      }
      State::Waiting => Some(Marker::Waiting),
      State::Gone => Some(Marker::Gone),
      State::Working | State::Unknown => None,
    },
    (None, Some(h)) => from_hook(h),
    (None, None) => None,
  }
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
    Self { program: program.into(), timeout }
  }

  /// `cira status --json`, or `None` when Cira is unreachable: not installed,
  /// no daemon, too slow, any failure exit, or output that does not parse.
  /// Blocks for up to the timeout, so it is never called on the main thread.
  pub fn status(&self) -> Option<Status> {
    let mut child = Command::new(&self.program)
      .args(["status", "--json"])
      .stdin(Stdio::null())
      .stdout(Stdio::piped())
      .stderr(Stdio::null())
      .spawn()
      .ok()?;
    // Read on a thread of its own, so a large answer cannot fill the pipe and
    // stall the child while this one waits for it to exit.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
      let mut out = Vec::new();
      stdout.read_to_end(&mut out).map(|_| out)
    });
    let deadline = Instant::now() + self.timeout;
    let exit = loop {
      match child.try_wait() {
        Ok(Some(exit)) => break exit,
        Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
        _ => {
          let _ = child.kill();
          let _ = child.wait();
          return None;
        }
      }
    };
    let out = reader.join().ok()?.ok()?;
    if !exit.success() {
      return None;
    }
    Status::parse(&out).ok()
  }
}

/// Call `read` every `every`, on a thread of its own, and send each result.
/// The caller's thread never runs `read`. Reads never overlap: the next waits
/// for the last and then `every`. The thread ends once the receiver is
/// dropped.
pub fn poll<T: Send + 'static>(
  every: Duration,
  mut read: impl FnMut() -> T + Send + 'static,
) -> flume::Receiver<T> {
  let (tx, rx) = flume::bounded(1);
  std::thread::Builder::new()
    .name("jc-cira-poll".into())
    .spawn(move || {
      while tx.send(read()).is_ok() {
        std::thread::sleep(every);
      }
    })
    .expect("spawning the Cira poll thread");
  rx
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
        // The generator names each session after the state it drove it to.
        let driven = s.id.strip_prefix("fx-").unwrap();
        let state: State = serde_json::from_value(Value::String(driven.to_string())).unwrap();
        assert_eq!(s.state, state, "{}", s.id);
        assert_ne!(s.state, State::Unknown, "{}", s.id);
      }
    }
    // Every state Cira has is in the fixture, so each one's parse is checked.
    let states: std::collections::HashSet<_> =
      status.projects.iter().flat_map(|p| &p.sessions).map(|s| s.state).collect();
    assert_eq!(states.len(), 5, "{states:?}");
    assert!(status.projects.iter().any(|p| p.open.blocks > 0));
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
    let mut dirs: Vec<&Path> = status.projects.iter().filter_map(|p| p.path.as_deref()).collect();
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
  fn a_jc_session_is_found_by_cira_id_then_short_then_uuid() {
    let (status, _) = fixture();
    let p = status.projects.iter().find(|p| p.sessions.len() > 1).unwrap();
    let [a, b, ..] = &p.sessions[..] else { unreachable!() };
    let short_b = b.short.as_deref().unwrap();
    let uuid_b = b.uuid.as_deref().unwrap();
    assert_eq!(p.session(Some(&a.id), short_b, uuid_b).map(|s| &s.id), Some(&a.id));
    assert_eq!(p.session(Some("not-cira"), short_b, "").map(|s| &s.id), Some(&b.id));
    assert_eq!(p.session(None, "", uuid_b).map(|s| &s.id), Some(&b.id));
    assert_eq!(p.session(Some(""), "", ""), None);
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
  fn cira_wins_over_hooks_except_a_newer_api_error() {
    let seen = Some(at(100));
    let working = Some((State::Working, Some(at(200))));
    // A stale hook says idle; Cira says working since later.
    assert_eq!(marker(working, Some((HookState::Idle, at(150))), seen), None);
    assert_eq!(marker(working, Some((HookState::Dialog, at(300))), seen), None);
    // An API error after Cira's last state: Cira still says working.
    assert_eq!(marker(working, Some((HookState::Failed, at(300))), seen), Some(Marker::Finished));
    // One before it was followed by a new turn Cira saw.
    assert_eq!(marker(working, Some((HookState::Failed, at(150))), seen), None);
    // Both idle: a turn that ended while Cira was down (Cira's idle is older
    // than the last look, the hook's newer) still marks the session.
    let stale_idle = Some((State::Idle, Some(at(50))));
    assert_eq!(marker(stale_idle, Some((HookState::Idle, at(150))), seen), Some(Marker::Finished));
    assert_eq!(marker(stale_idle, Some((HookState::Idle, at(60))), seen), None);
  }

  #[test]
  fn hooks_set_the_marker_while_cira_does_not_know_the_session() {
    let seen = Some(at(100));
    assert_eq!(marker(None, Some((HookState::Dialog, at(50))), seen), Some(Marker::Dialog));
    assert_eq!(marker(None, Some((HookState::Idle, at(150))), seen), Some(Marker::Finished));
    assert_eq!(marker(None, Some((HookState::Failed, at(150))), seen), Some(Marker::Finished));
    assert_eq!(marker(None, Some((HookState::Idle, at(50))), seen), None);
    assert_eq!(marker(None, Some((HookState::Working, at(150))), seen), None);
    assert_eq!(marker(None, None, seen), None);
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
  fn dialog_sorts_before_finished_before_the_rest() {
    let mut marks = [Marker::Gone, Marker::Waiting, Marker::Finished, Marker::Dialog];
    marks.sort();
    assert_eq!(marks[..2], [Marker::Dialog, Marker::Finished]);
  }

  #[test]
  fn an_unreachable_cira_reads_as_none() {
    let missing = Cira::new("/nonexistent/jc-test/cira", Duration::from_secs(1));
    assert_eq!(missing.status(), None);
    // A `cira` that exits 125, as one with no daemon does, after printing JSON.
    let dir = std::env::temp_dir().join(format!("jc-cira-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = |name: &str, body: &str| {
      let path = dir.join(name);
      std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
      use std::os::unix::fs::PermissionsExt;
      std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
      path
    };
    let failing = script("failing", "echo '{\"projects\":[]}'; exit 125");
    assert_eq!(Cira::new(failing, Duration::from_secs(5)).status(), None);
    let hanging = script("hanging", "exec sleep 30");
    let started = Instant::now();
    assert_eq!(Cira::new(hanging, Duration::from_millis(300)).status(), None);
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    let fixture = dir.join("status.json");
    std::fs::write(&fixture, FIXTURE).unwrap();
    let answering = script("answering", &format!("cat '{}'", fixture.display()));
    assert_eq!(Cira::new(answering, Duration::from_secs(5)).status(), Some(fixture_status()));
    let _ = std::fs::remove_dir_all(&dir);
  }

  fn fixture_status() -> Status {
    fixture().0
  }

  #[test]
  fn the_poll_reads_off_the_calling_thread() {
    let caller = std::thread::current().id();
    let started = Instant::now();
    let rx = poll(Duration::from_millis(10), move || {
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
}
