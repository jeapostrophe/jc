//! Claude Code background sessions: the `claude` subcommands jc runs to start,
//! find, stop and remove them, and what it reads back.
//!
//! Every command is an argument vector, never a shell string, and every call
//! here blocks — run them off the main thread. Facts about the CLI below were
//! measured on Claude Code 2.1.322 (Cira SP-03/SP-06 findings).

use crate::todo::IdKind;
use anyhow::{Context as _, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// How a background session's permissions are set when it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permissions {
  /// Pass nothing: Claude Code's default, or what a resumed job saved.
  Unset,
  /// `--dangerously-skip-permissions` (`> dangerous`).
  Dangerous,
  /// `--permission-mode default`. Needed to turn bypass OFF on a resumed
  /// conversation, because passing no flag keeps the job's saved mode.
  ForceDefault,
}

/// One `claude --bg` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartSpec {
  /// `-n <label>`: the display name. Fixed at creation; never a key.
  pub label: String,
  /// The stable id, passed as `--settings '{"env":{"CIRA_SESSION":…}}'`. The
  /// environment of `claude --bg` itself never reaches the session.
  pub cira: Option<String>,
  pub permissions: Permissions,
  /// `--resume <uuid>`: continue that conversation in the background. Claude
  /// Code continues the same uuid only when no process holds it; otherwise,
  /// or when flags differ from the job's saved ones, it starts a copy.
  pub resume: Option<String>,
}

impl StartSpec {
  /// The arguments after `claude`.
  pub fn args(&self) -> Vec<String> {
    let mut args = vec!["--bg".to_string(), "-n".into(), self.label.clone()];
    if let Some(cira) = &self.cira {
      let settings = serde_json::json!({ "env": { "CIRA_SESSION": cira } });
      args.extend(["--settings".into(), settings.to_string()]);
    }
    if let Some(uuid) = &self.resume {
      args.extend(["--resume".into(), uuid.clone()]);
    }
    // Never both: Claude Code drops `--dangerously-skip-permissions` when a
    // later `--permission-mode` follows it.
    match self.permissions {
      Permissions::Unset => {}
      Permissions::Dangerous => args.push("--dangerously-skip-permissions".into()),
      Permissions::ForceDefault => args.extend(["--permission-mode".into(), "default".into()]),
    }
    args
  }
}

/// The short id in `claude --bg`'s stdout: `backgrounded · <short> · <name> …`.
/// A forked copy prints the same line with the copy's id (the note about the
/// copy goes to stderr).
pub fn parse_backgrounded(stdout: &str) -> Option<String> {
  stdout.lines().find_map(|line| {
    let rest = line.trim_start().strip_prefix("backgrounded · ")?;
    let short = rest.split_whitespace().next()?;
    (!short.is_empty()).then(|| short.to_string())
  })
}

/// One entry of `claude agents --json --all`. Background entries carry `id`
/// (the short id); live entries carry `pid` and `status`; stopped and failed
/// background entries carry only `state`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
  pub id: Option<String>,
  pub cwd: Option<String>,
  pub kind: Option<String>,
  /// The conversation uuid. Current for a live session; may be the creation
  /// uuid of a stopped one after a `/clear`.
  pub session_id: Option<String>,
  pub name: Option<String>,
  /// `blocked`, `working`, `stopped`, `failed`, … (the agent-view summary).
  pub state: Option<String>,
  /// `idle`, `busy` or `waiting` — present only while the process runs.
  pub status: Option<String>,
  pub pid: Option<u32>,
}

impl Agent {
  /// The process is running.
  pub fn is_live(&self) -> bool {
    self.status.is_some()
  }

  fn is_background(&self) -> bool {
    self.kind.as_deref() == Some("background")
  }
}

/// The entry for background session `short`.
pub fn find<'a>(agents: &'a [Agent], short: &str) -> Option<&'a Agent> {
  agents.iter().find(|a| a.id.as_deref() == Some(short))
}

/// What starting a heading's session takes, given what Claude Code is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchPlan {
  /// `claude attach <short>`. Attach also resumes a stopped or retired job.
  Attach(String),
  /// `claude --bg`, continuing `resume` when given, then attach.
  Start { resume: Option<String> },
  /// A legacy uuid held by a running interactive `claude`: `--bg --resume`
  /// would fork a copy, so nothing is started.
  Blocked,
}

/// Decide how to start the heading whose id line is `id` (never empty).
///
/// - `> bg=S` known to Claude Code: attach to it. Unknown (removed): start a
///   fresh session, keeping the heading — the background analogue of reviving
///   a garbage-collected transcript.
/// - Legacy `> uuid=U`: attach to a background job already on that
///   conversation; refuse when an interactive process holds it; otherwise
///   continue it with `--bg --resume U` (whose short id is U's prefix) when its
///   transcript is on disk, or start fresh when it is not.
pub fn plan_launch(
  id: &str,
  kind: IdKind,
  agents: &[Agent],
  transcript_exists: bool,
) -> LaunchPlan {
  match kind {
    IdKind::Bg => match find(agents, id) {
      Some(_) => LaunchPlan::Attach(id.to_string()),
      None => LaunchPlan::Start { resume: None },
    },
    IdKind::Uuid => {
      let on_it = || agents.iter().filter(|a| a.session_id.as_deref() == Some(id));
      // A live background job first, then any (attach resumes a stopped one).
      let job = on_it()
        .filter(|a| a.is_background())
        .max_by_key(|a| a.is_live())
        .and_then(|a| a.id.clone());
      if let Some(short) = job {
        LaunchPlan::Attach(short)
      } else if on_it().any(Agent::is_live) {
        LaunchPlan::Blocked
      } else if transcript_exists {
        LaunchPlan::Start { resume: Some(id.to_string()) }
      } else {
        LaunchPlan::Start { resume: None }
      }
    }
  }
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
  hook_session == uuid
    || (!short.is_empty()
      && hook_session.strip_prefix(short).is_some_and(|rest| rest.starts_with('-')))
}

/// How often and how long to poll `claude agents --json`.
#[derive(Debug, Clone, Copy)]
pub struct Poll {
  pub interval: Duration,
  pub timeout: Duration,
}

impl Poll {
  /// A cold start can take about two minutes to reach idle under load, and an
  /// `agents --json` call costs about a second at load ~140.
  pub const START: Poll =
    Poll { interval: Duration::from_millis(1500), timeout: Duration::from_secs(180) };
}

/// The `claude` executable and the calls jc makes to it.
#[derive(Debug, Clone)]
pub struct Claude {
  program: PathBuf,
}

impl Default for Claude {
  /// `claude` from `PATH`, as the Claude pane always ran it.
  fn default() -> Self {
    Self { program: PathBuf::from("claude") }
  }
}

impl Claude {
  pub fn new(program: impl Into<PathBuf>) -> Self {
    Self { program: program.into() }
  }

  pub fn program(&self) -> &Path {
    &self.program
  }

  /// `claude attach <short>`, as the argument vector the Claude pane runs.
  pub fn attach_argv(&self, short: &str) -> Vec<String> {
    vec![self.program.to_string_lossy().into_owned(), "attach".into(), short.into()]
  }

  fn run(&self, args: &[String], cwd: Option<&Path>) -> Result<Output> {
    let mut cmd = Command::new(&self.program);
    cmd.args(args).stdin(Stdio::null());
    if let Some(cwd) = cwd {
      cmd.current_dir(cwd);
    }
    cmd.output().with_context(|| format!("running {} {}", self.program.display(), args.join(" ")))
  }

  fn run_ok(&self, args: &[String]) -> Result<Output> {
    let out = self.run(args, None)?;
    if !out.status.success() {
      bail!(
        "claude {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim().to_string()
          + String::from_utf8_lossy(&out.stdout).trim()
      );
    }
    Ok(out)
  }

  /// One `claude --bg` in `cwd`; returns the short id it printed.
  pub fn start_once(&self, spec: &StartSpec, cwd: &Path) -> Result<String> {
    let args = spec.args();
    let out = self.run(&args, Some(cwd))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_backgrounded(&stdout).with_context(|| {
      format!(
        "claude --bg printed no short id (exit {}): {} {}",
        out.status,
        stdout.trim(),
        String::from_utf8_lossy(&out.stderr).trim()
      )
    })
  }

  /// `claude agents --json --all`.
  pub fn agents(&self) -> Result<Vec<Agent>> {
    let out = self.run_ok(&["agents".into(), "--json".into(), "--all".into()])?;
    serde_json::from_slice(&out.stdout).context("parsing claude agents --json")
  }

  pub fn stop(&self, short: &str) -> Result<()> {
    self.run_ok(&["stop".into(), short.into()]).map(drop)
  }

  pub fn rm(&self, short: &str) -> Result<()> {
    self.run_ok(&["rm".into(), short.into()]).map(drop)
  }

  /// Poll until `short` is live (`Ok(Some(entry))`), has settled `failed`
  /// (`Ok(None)`), or `poll.timeout` passes (`Ok(Some(default))` — the caller
  /// attaches anyway, and attach shows whatever there is).
  pub fn wait_settled(&self, short: &str, poll: Poll) -> Result<Option<Agent>> {
    let deadline = Instant::now() + poll.timeout;
    loop {
      let agents = self.agents()?;
      match find(&agents, short) {
        Some(a) if a.is_live() => return Ok(Some(a.clone())),
        Some(a) if a.state.as_deref() == Some("failed") => return Ok(None),
        _ => {}
      }
      if Instant::now() >= deadline {
        return Ok(Some(Agent::default()));
      }
      std::thread::sleep(poll.interval);
    }
  }

  /// Start a background session and wait for it to settle, retrying once when
  /// the first start settles `failed` (seen at high load: SIGKILL before init).
  /// The failed job is removed before the retry.
  pub fn start(&self, spec: &StartSpec, cwd: &Path, poll: Poll) -> Result<String> {
    let first = self.start_once(spec, cwd)?;
    if self.wait_settled(&first, poll)?.is_some() {
      return Ok(first);
    }
    eprintln!("claude --bg: {first} settled failed; removing it and retrying once");
    if let Err(e) = self.rm(&first) {
      eprintln!("claude rm {first}: {e:#}");
    }
    let second = self.start_once(spec, cwd)?;
    match self.wait_settled(&second, poll)? {
      Some(_) => Ok(second),
      None => bail!("claude --bg failed twice ({first}, then {second})"),
    }
  }

  /// Change a session's `> dangerous` mode: Claude Code keeps a job's saved
  /// options, so the only route is to stop `short`, resume its conversation
  /// `uuid` with the new mode — which starts a copy under a new short id and a
  /// new uuid, carrying the conversation — and remove the old job. Returns the
  /// new short id.
  pub fn toggle_dangerous(
    &self,
    short: &str,
    uuid: &str,
    spec: &StartSpec,
    cwd: &Path,
    poll: Poll,
  ) -> Result<String> {
    debug_assert_eq!(spec.resume.as_deref(), Some(uuid), "the toggle resumes the conversation");
    self.stop(short)?;
    let new_short = self.start(spec, cwd, poll)?;
    if let Err(e) = self.rm(short) {
      eprintln!("claude rm {short} after toggling dangerous: {e:#}");
    }
    Ok(new_short)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::atomic::{AtomicUsize, Ordering};

  /// A fake `claude`: logs each invocation's argv (one argument per line, then
  /// `---`), prints `backgrounded · <next short>` for `--bg`, serves
  /// `agents.<n>.json` (or the last one) for `agents`, and echoes `stop`/`rm`.
  struct Fake {
    dir: PathBuf,
  }

  static NEXT: AtomicUsize = AtomicUsize::new(0);

  impl Fake {
    fn new(shorts: &[&str], agents: &[&str]) -> Self {
      let dir = std::env::temp_dir().join(format!(
        "jc-fake-claude-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
      ));
      let _ = std::fs::remove_dir_all(&dir);
      std::fs::create_dir_all(&dir).unwrap();
      std::fs::write(dir.join("shorts"), shorts.join("\n") + "\n").unwrap();
      for (i, json) in agents.iter().enumerate() {
        std::fs::write(dir.join(format!("agents.{i}.json")), json).unwrap();
      }
      let d = dir.display();
      let script = format!(
        r#"#!/bin/sh
for a in "$@"; do printf '%s\n' "$a"; done >> "{d}/log"
echo --- >> "{d}/log"
case "$1" in
  --bg)
    short=$(head -n 1 "{d}/shorts"); tail -n +2 "{d}/shorts" > "{d}/shorts.t"; mv "{d}/shorts.t" "{d}/shorts"
    echo "Starting background service…" >&2
    printf 'backgrounded · %s · name (idle — send a prompt to start)\n  claude attach %s    open in this terminal\n' "$short" "$short" ;;
  agents)
    n=$(cat "{d}/agents.count" 2>/dev/null || echo 0); echo $((n+1)) > "{d}/agents.count"
    f="{d}/agents.$n.json"; [ -f "$f" ] || f=$(ls "{d}"/agents.*.json | sort -t. -k2 -n | tail -n 1)
    cat "$f" ;;
  stop) echo "stopped $2" ;;
  rm) echo "removed $2" ;;
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

    /// Every invocation's argv, in order.
    fn calls(&self) -> Vec<Vec<String>> {
      let log = std::fs::read_to_string(self.dir.join("log")).unwrap_or_default();
      let mut calls = vec![];
      let mut cur = vec![];
      for line in log.lines() {
        if line == "---" {
          calls.push(std::mem::take(&mut cur));
        } else {
          cur.push(line.to_string());
        }
      }
      calls
    }
  }

  impl Drop for Fake {
    fn drop(&mut self) {
      let _ = std::fs::remove_dir_all(&self.dir);
    }
  }

  const FAST: Poll = Poll { interval: Duration::from_millis(1), timeout: Duration::from_secs(5) };

  fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
  }

  /// The `--settings` value the spec row prescribes: `{"env":{"CIRA_SESSION":…}}`.
  fn settings_env(args: &[String]) -> serde_json::Value {
    let i = args.iter().position(|a| a == "--settings").expect("--settings passed");
    let v: serde_json::Value = serde_json::from_str(&args[i + 1]).unwrap();
    v["env"]["CIRA_SESSION"].clone()
  }

  // ---- argument lists (PLAN TMUX-01 start line, TMUX-02) ----

  #[test]
  fn start_args_carry_label_and_cira_session_as_separate_arguments() {
    let spec = StartSpec {
      label: "Fix the build \"now\"".into(),
      cira: Some("c-1".into()),
      permissions: Permissions::Unset,
      resume: None,
    };
    let args = spec.args();
    assert_eq!(args[0], "--bg");
    let n = args.iter().position(|a| a == "-n").expect("-n passed");
    assert_eq!(args[n + 1], "Fix the build \"now\"", "a label with spaces stays one argument");
    assert_eq!(settings_env(&args), "c-1");
    assert!(!args.iter().any(|a| a.contains("dangerously") || a == "--permission-mode"));
    assert!(!args.contains(&"--resume".to_string()));
  }

  #[test]
  fn dangerous_is_the_bypass_flag_and_never_with_permission_mode() {
    let mut spec = StartSpec {
      label: "x".into(),
      cira: None,
      permissions: Permissions::Dangerous,
      resume: Some("u-1".into()),
    };
    let args = spec.args();
    assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
    assert!(!args.contains(&"--permission-mode".to_string()));
    let r = args.iter().position(|a| a == "--resume").expect("--resume passed");
    assert_eq!(args[r + 1], "u-1");
    assert!(!args.contains(&"--settings".to_string()), "no stable id, no --settings");

    spec.permissions = Permissions::ForceDefault;
    let args = spec.args();
    let p = args.iter().position(|a| a == "--permission-mode").expect("--permission-mode");
    assert_eq!(args[p + 1], "default");
    assert!(!args.contains(&"--dangerously-skip-permissions".to_string()));
  }

  #[test]
  fn parses_the_short_id_claude_bg_prints() {
    // Verbatim stdout of `claude --bg -n jctmux-probe1 …` on 2.1.322.
    let out = "backgrounded · 335b461a · jctmux-probe1 (idle — send a prompt to start)\n  claude agents             list sessions\n  claude attach 335b461a    open in this terminal\n";
    assert_eq!(parse_backgrounded(out).as_deref(), Some("335b461a"));
    assert_eq!(parse_backgrounded("Starting background service…\n"), None);
  }

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

  // ---- launch planning (TMUX-01: attach on start; migrate `> uuid=` once) ----

  fn bg(short: &str, uuid: &str, live: bool) -> Agent {
    Agent {
      id: Some(short.into()),
      kind: Some("background".into()),
      session_id: Some(uuid.into()),
      status: live.then(|| "idle".into()),
      state: Some(if live { "blocked" } else { "stopped" }.into()),
      ..Default::default()
    }
  }

  fn interactive(uuid: &str) -> Agent {
    Agent {
      kind: Some("interactive".into()),
      session_id: Some(uuid.into()),
      status: Some("idle".into()),
      pid: Some(1),
      ..Default::default()
    }
  }

  const U: &str = "335b461a-eba5-485f-898c-f757fdb3f2bf";

  #[test]
  fn a_bg_heading_attaches_running_or_stopped_and_restarts_when_removed() {
    let agents = [bg("335b461a", U, false)];
    assert_eq!(
      plan_launch("335b461a", IdKind::Bg, &agents, false),
      LaunchPlan::Attach("335b461a".into())
    );
    assert_eq!(
      plan_launch("deadbeef", IdKind::Bg, &agents, true),
      LaunchPlan::Start { resume: None },
      "a removed job comes back as a fresh session under the same heading"
    );
  }

  #[test]
  fn a_legacy_uuid_is_migrated_once_and_never_while_running() {
    // Not running anywhere, transcript on disk: continue it in the background.
    assert_eq!(
      plan_launch(U, IdKind::Uuid, &[], true),
      LaunchPlan::Start { resume: Some(U.into()) }
    );
    // Transcript gone: a fresh session (the heading survives).
    assert_eq!(plan_launch(U, IdKind::Uuid, &[], false), LaunchPlan::Start { resume: None });
    // Held by an interactive `claude` (an old jc): resuming would fork a copy.
    assert_eq!(plan_launch(U, IdKind::Uuid, &[interactive(U)], true), LaunchPlan::Blocked);
    // Already a background job on that conversation (migrated before, or by
    // hand): attach to it rather than forking.
    assert_eq!(
      plan_launch(U, IdKind::Uuid, &[bg("335b461a", U, true)], true),
      LaunchPlan::Attach("335b461a".into())
    );
  }

  // ---- hook matching by short id ----

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

  // ---- starting, with one retry ----

  #[test]
  fn start_runs_claude_bg_in_the_project_and_returns_the_short_id() {
    let fake =
      Fake::new(&["aaaa1111"], &[r#"[{"id":"aaaa1111","kind":"background","status":"idle"}]"#]);
    let spec = StartSpec {
      label: "L".into(),
      cira: Some("c".into()),
      permissions: Permissions::Unset,
      resume: None,
    };
    let short = fake.claude().start(&spec, &std::env::temp_dir(), FAST).unwrap();
    assert_eq!(short, "aaaa1111");
    let calls = fake.calls();
    assert_eq!(calls[0], spec.args());
  }

  #[test]
  fn a_start_that_settles_failed_is_removed_and_retried_once() {
    let failed = r#"[{"id":"aaaa1111","kind":"background","state":"failed"}]"#;
    let live = r#"[{"id":"bbbb2222","kind":"background","status":"idle","state":"blocked"}]"#;
    let fake = Fake::new(&["aaaa1111", "bbbb2222"], &[failed, live]);
    let spec =
      StartSpec { label: "L".into(), cira: None, permissions: Permissions::Unset, resume: None };
    let short = fake.claude().start(&spec, &std::env::temp_dir(), FAST).unwrap();
    assert_eq!(short, "bbbb2222");
    let verbs: Vec<String> = fake.calls().iter().map(|c| c[0].clone()).collect();
    assert_eq!(verbs, strs(&["--bg", "agents", "rm", "--bg", "agents"]));
    assert_eq!(fake.calls()[2], strs(&["rm", "aaaa1111"]));
  }

  #[test]
  fn a_second_failed_start_is_an_error() {
    let failed = r#"[{"id":"aaaa1111","kind":"background","state":"failed"},{"id":"bbbb2222","kind":"background","state":"failed"}]"#;
    let fake = Fake::new(&["aaaa1111", "bbbb2222"], &[failed]);
    let spec =
      StartSpec { label: "L".into(), cira: None, permissions: Permissions::Unset, resume: None };
    assert!(fake.claude().start(&spec, &std::env::temp_dir(), FAST).is_err());
    let bg_starts = fake.calls().iter().filter(|c| c[0] == "--bg").count();
    assert_eq!(bg_starts, 2, "exactly one retry");
  }

  // ---- the dangerous toggle (Q-52) ----

  #[test]
  fn toggling_dangerous_stops_resumes_with_the_flag_and_removes_the_old_job() {
    let live = r#"[{"id":"bbbb2222","kind":"background","status":"idle"}]"#;
    let fake = Fake::new(&["bbbb2222"], &[live]);
    let spec = StartSpec {
      label: "L".into(),
      cira: Some("c-1".into()),
      permissions: Permissions::Dangerous,
      resume: Some(U.into()),
    };
    let new_short =
      fake.claude().toggle_dangerous("335b461a", U, &spec, &std::env::temp_dir(), FAST).unwrap();
    assert_eq!(new_short, "bbbb2222");
    let calls = fake.calls();
    assert_eq!(calls[0], strs(&["stop", "335b461a"]));
    assert_eq!(calls[1], spec.args());
    assert_eq!(calls.last().unwrap(), &strs(&["rm", "335b461a"]));
  }
}
