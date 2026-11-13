//! Claude Code background sessions as jc sees them: `claude attach`, which a
//! Claude pane runs, and `claude agents --json`, which says when an attached
//! session can take input. Starting, forking, stopping and removing jobs is
//! Cira's (Cira DESIGN 12); jc asks for those with `cira session …`
//! (`crate::cira`) and never runs them itself.
//!
//! Every command is an argument vector, never a shell string, and every call
//! here blocks — run them off the main thread. Facts about the CLI below were
//! measured on Claude Code 2.1.322 (Cira SP-03/SP-06 findings).

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// One entry of `claude agents --json --all`. Background entries carry `id`
/// (the short id); live entries carry `pid` and `status`; stopped and failed
/// background entries carry only `state`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
  pub id: Option<String>,
  pub kind: Option<String>,
  /// The conversation uuid. Current for a live session; may be the creation
  /// uuid of a stopped one after a `/clear`.
  pub session_id: Option<String>,
  /// `blocked`, `working`, `stopped`, `failed`, … (the agent-view summary).
  pub state: Option<String>,
  /// `idle`, `busy` or `waiting` — present only while the process runs.
  pub status: Option<String>,
}

impl Agent {
  /// The process is running.
  pub fn is_live(&self) -> bool {
    self.status.is_some()
  }
}

/// The entry for background session `short`.
pub fn find<'a>(agents: &'a [Agent], short: &str) -> Option<&'a Agent> {
  agents.iter().find(|a| a.id.as_deref() == Some(short))
}

/// The short id a background session's uuid starts with (its first segment).
pub fn short_of(uuid: &str) -> Option<&str> {
  uuid.split_once('-').map(|(short, _)| short)
}

/// Does a hook whose `session_id` is `hook_session` belong to the background
/// session `short`, last known on conversation `uuid`?
///
/// At creation a background session's uuid starts with its short id; a
/// `/clear` gives it a new uuid with the same short id, which jc learns from
/// the `/clear` hook pair or from `claude agents --json`.
pub fn hook_matches(short: &str, uuid: &str, hook_session: &str) -> bool {
  if hook_session.is_empty() {
    return false;
  }
  hook_session == uuid || (!short.is_empty() && short_of(hook_session) == Some(short))
}

/// What became of a job [`Claude::wait_settled`] watched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
  /// Its process runs.
  Live(Agent),
  /// It settled `failed`.
  Failed,
  /// Claude Code does not list it at all: removed.
  Gone,
  /// Still neither live nor failed when the poll gave up.
  TimedOut,
}

/// How often and how long to poll `claude agents --json`.
#[derive(Debug, Clone, Copy)]
pub struct Poll {
  pub interval: Duration,
  pub timeout: Duration,
}

impl Poll {
  /// A cold start or a resume can take about two minutes to reach idle under
  /// load, and an `agents --json` call costs about a second at load ~140.
  pub const START: Poll =
    Poll { interval: Duration::from_millis(1500), timeout: Duration::from_secs(180) };
}

/// The `claude` executable and the calls jc makes to it.
#[derive(Debug, Clone)]
pub struct Claude {
  program: PathBuf,
}

impl Default for Claude {
  /// `claude` from `PATH`.
  fn default() -> Self {
    Self { program: PathBuf::from("claude") }
  }
}

impl Claude {
  pub fn new(program: impl Into<PathBuf>) -> Self {
    Self { program: program.into() }
  }

  /// `claude attach <short>`, as the argument vector the Claude pane runs.
  pub fn attach_argv(&self, short: &str) -> Vec<String> {
    vec![self.program.to_string_lossy().into_owned(), "attach".into(), short.into()]
  }

  /// How long one `claude` call may take before jc gives up on it.
  const TIMEOUT: Duration = Duration::from_secs(30);

  /// `claude <args>`'s stdout, or why it failed (including no answer within
  /// [`Self::TIMEOUT`]).
  fn run_ok(&self, args: &[&str]) -> Result<Vec<u8>> {
    let mut cmd = Command::new(&self.program);
    let what = || format!("claude {}", args.join(" "));
    let (code, stdout, stderr) = crate::cira::run_bounded(cmd.args(args), Self::TIMEOUT)
      .with_context(|| format!("running {}", self.program.display()))?;
    match code {
      Some(0) => Ok(stdout),
      Some(code) => bail!("{} failed (exit {code}): {}", what(), stderr.trim()),
      None => bail!("{} gave no answer in {:?}", what(), Self::TIMEOUT),
    }
  }

  /// `claude agents --json --all`.
  pub fn agents(&self) -> Result<Vec<Agent>> {
    let out = self.run_ok(&["agents", "--json", "--all"])?;
    serde_json::from_slice(&out).context("parsing claude agents --json")
  }

  /// Poll `claude agents --json --all` until `short` is live, has settled
  /// `failed`, is not listed at all, or `poll.timeout` passes.
  pub fn wait_settled(&self, short: &str, poll: Poll) -> Result<Settled> {
    let deadline = Instant::now() + poll.timeout;
    loop {
      let agents = self.agents()?;
      match find(&agents, short) {
        None => return Ok(Settled::Gone),
        Some(a) if a.is_live() => return Ok(Settled::Live(a.clone())),
        Some(a) if a.state.as_deref() == Some("failed") => return Ok(Settled::Failed),
        Some(_) => {}
      }
      if Instant::now() >= deadline {
        return Ok(Settled::TimedOut);
      }
      std::thread::sleep(poll.interval);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::atomic::{AtomicUsize, Ordering};

  /// A fake `claude` that serves `agents.<n>.json` (or the last one) for
  /// `agents` and refuses anything else.
  struct Fake {
    dir: PathBuf,
  }

  static NEXT: AtomicUsize = AtomicUsize::new(0);

  impl Fake {
    fn new(agents: &[&str]) -> Self {
      let dir = std::env::temp_dir().join(format!(
        "jc-fake-claude-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
      ));
      let _ = std::fs::remove_dir_all(&dir);
      std::fs::create_dir_all(&dir).unwrap();
      for (i, json) in agents.iter().enumerate() {
        std::fs::write(dir.join(format!("agents.{i}.json")), json).unwrap();
      }
      let d = dir.display();
      let script = format!(
        r#"#!/bin/sh
case "$1" in
  agents)
    n=$(cat "{d}/agents.count" 2>/dev/null || echo 0); echo $((n+1)) > "{d}/agents.count"
    f="{d}/agents.$n.json"; [ -f "$f" ] || f=$(ls "{d}"/agents.*.json | sort -t. -k2 -n | tail -n 1)
    cat "$f" ;;
  *) echo "unexpected $1" >&2; exit 2 ;;
esac
"#
      );
      let path = dir.join("claude");
      std::fs::write(&path, script).unwrap();
      use std::os::unix::fs::PermissionsExt;
      std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
      Self { dir }
    }

    fn claude(&self) -> Claude {
      Claude::new(self.dir.join("claude"))
    }
  }

  impl Drop for Fake {
    fn drop(&mut self) {
      let _ = std::fs::remove_dir_all(&self.dir);
    }
  }

  const FAST: Poll = Poll { interval: Duration::from_millis(1), timeout: Duration::from_secs(5) };
  const U: &str = "335b461a-eba5-485f-898c-f757fdb3f2bf";

  #[test]
  fn parses_agents_json() {
    // Shapes copied from `claude agents --json --all` on 2.1.322.
    let json = r#"[
      {"pid": 72526, "id": "335b461a", "cwd": "/p", "kind": "background", "startedAt": 1, "sessionId": "335b461a-eba5-485f-898c-f757fdb3f2bf", "name": "n", "status": "idle", "state": "blocked"},
      {"id": "427e24ee", "cwd": "/p", "kind": "background", "startedAt": 2, "sessionId": "427e24ee-fc2b-4397-95e9-e007a5ecfb0a", "name": "n", "state": "stopped"},
      {"pid": 2866, "cwd": "/q", "kind": "interactive", "startedAt": 3, "sessionId": "6d0d98a7-a86c-4551-a045-4ee62e10a6e9", "name": "boku", "status": "idle"}
    ]"#;
    let agents: Vec<Agent> = serde_json::from_str(json).unwrap();
    assert!(find(&agents, "335b461a").unwrap().is_live());
    assert!(!find(&agents, "427e24ee").unwrap().is_live());
    assert!(agents[2].is_live() && agents[2].id.is_none());
  }

  #[test]
  fn hooks_match_by_short_id_prefix_and_by_the_learned_uuid() {
    // A fresh background session's first hook: jc knows only the short id.
    assert!(hook_matches("35a13d68", "", "35a13d68-0f6e-4b1e-9d36-1f2a3b4c5d6e"));
    // After `/clear` the uuid no longer starts with the short id; the learned
    // uuid matches it.
    assert!(hook_matches(
      "35a13d68",
      "6955c2a6-aaaa-bbbb-cccc-dddddddddddd",
      "6955c2a6-aaaa-bbbb-cccc-dddddddddddd"
    ));
    // Someone else's session.
    assert!(!hook_matches("35a13d68", "", "a5185f98-0000-0000-0000-000000000000"));
    // A short id is a whole uuid segment, not any prefix.
    assert!(!hook_matches("35a13d6", "", "35a13d68-0f6e-4b1e-9d36-1f2a3b4c5d6e"));
    assert!(!hook_matches("35a13d68", "", ""));
    assert!(!hook_matches("", "", "35a13d68-0f6e-4b1e-9d36-1f2a3b4c5d6e"));
  }

  #[test]
  fn waiting_for_a_job_tells_live_failed_gone_and_timed_out_apart() {
    let live = r#"[{"id":"aaaa1111","kind":"background","status":"idle","sessionId":"u"}]"#;
    let fake = Fake::new(&[r#"[{"id":"aaaa1111","kind":"background","state":"working"}]"#, live]);
    assert!(matches!(fake.claude().wait_settled("aaaa1111", FAST).unwrap(), Settled::Live(_)));
    // A pane whose job was removed elsewhere must stop polling.
    let fake = Fake::new(&[r#"[{"id":"other111","kind":"background","status":"idle"}]"#]);
    assert_eq!(fake.claude().wait_settled("aaaa1111", FAST).unwrap(), Settled::Gone);
    let fake = Fake::new(&[r#"[{"id":"aaaa1111","kind":"background","state":"failed"}]"#]);
    assert_eq!(fake.claude().wait_settled("aaaa1111", FAST).unwrap(), Settled::Failed);
    let fake = Fake::new(&[r#"[{"id":"aaaa1111","kind":"background","state":"working"}]"#]);
    let slow = Poll { interval: Duration::from_millis(1), timeout: Duration::from_millis(20) };
    assert_eq!(fake.claude().wait_settled("aaaa1111", slow).unwrap(), Settled::TimedOut);
  }

  #[test]
  fn attach_is_one_argument_vector() {
    assert_eq!(
      Claude::new("/x/claude").attach_argv("335b461a"),
      ["/x/claude", "attach", "335b461a"]
    );
  }

  #[test]
  fn short_of_is_the_first_uuid_segment() {
    assert_eq!(short_of(U), Some("335b461a"));
    assert_eq!(short_of("nodash"), None);
  }
}
