//! Locating Claude Code session transcripts on disk.
//!
//! Claude Code stores each session as `~/.claude/projects/<bucket>/<uuid>.jsonl`,
//! where `<bucket>` is the launch working directory with every character that is
//! not ASCII alphanumeric or `-` replaced by `-`. A git worktree therefore lands
//! in its *own* sibling bucket: the harness creates worktrees at
//! `<root>/.claude/worktrees/<branch>`, which encodes to
//! `<encoded_root>--claude-worktrees-<branch>`. A project's live sessions can thus
//! be spread across the root bucket and any number of worktree buckets, so a
//! discovery pass that looks only at the root bucket misses every session that
//! was launched inside a worktree. These helpers enumerate all of a project's
//! buckets.

use std::path::{Path, PathBuf};

/// How jc (`Flavor::Jc`) starts the Claude CLI on a session uuid.
///
/// The two are NOT interchangeable, and picking wrong leaves a dead pane.
/// Measured 2026-08-27: `claude --resume <uuid>` with no `<uuid>.jsonl` on disk
/// prints "No conversation found with session ID: <uuid>" and exits — that is
/// true both for a uuid Claude has garbage-collected and for one jc minted but
/// never sent a prompt to. `claude --session-id <uuid>` on that same uuid starts
/// a fresh conversation under it. So the choice is made by whether the
/// transcript exists ([`transcript_in`]), and a session whose transcript
/// vanished is revived rather than retired — which is why jc has no "expired"
/// state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
  /// `--session-id <uuid>`: claim a uuid with no conversation behind it.
  New,
  /// `--resume <uuid>`: continue an existing conversation. Requires the
  /// transcript to be present.
  Resume,
}

/// jc's own `claude` command line for `uuid` (see [`Launch`]).
pub fn claude_argv(uuid: &str, launch: Launch, dangerous: bool) -> Vec<String> {
  let flag = match launch {
    Launch::New => "--session-id",
    Launch::Resume => "--resume",
  };
  let mut argv = vec!["claude".to_string(), flag.to_string(), uuid.to_string()];
  if dangerous {
    argv.push("--dangerously-skip-permissions".to_string());
  }
  argv
}

/// Encode a project path the way Claude Code names its `~/.claude/projects/<dir>`
/// bucket: every character that is not ASCII alphanumeric or `-` becomes `-`.
pub fn encode_project_path(project_path: &Path) -> String {
  project_path
    .to_string_lossy()
    .chars()
    .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
    .collect()
}

/// The `--claude-worktrees-` fragment is the encoding of `/.claude/worktrees/`,
/// the path segment the harness inserts for a worktree.
const WORKTREE_INFIX: &str = "--claude-worktrees-";

/// Is `name` (a directory under `~/.claude/projects/`) a git-worktree bucket
/// belonging to the project whose root encodes to `encoded_root`?
///
/// Matching on the full [`WORKTREE_INFIX`] separator keeps a sibling project like
/// `<encoded_root>-2` (`pgm` vs `pgm-2`) from being swept in, since its worktree
/// buckets read `<encoded_root>-2--claude-worktrees-…`, not `<encoded_root>--…`.
pub fn is_worktree_bucket(encoded_root: &str, name: &str) -> bool {
  name.strip_prefix(encoded_root).is_some_and(|rest| rest.starts_with(WORKTREE_INFIX))
}

/// Is `path` Claude Code's executable: a file named `claude` (a link the
/// installer made, or Node's `process.title`), or the native installer's
/// versioned binary (`…/claude/versions/2.1.323`), which the link resolves to?
fn is_claude_program(path: &str) -> bool {
  let path = Path::new(path);
  let named = |p: Option<&Path>, name: &str| p.and_then(Path::file_name).is_some_and(|n| n == name);
  named(Some(path), "claude")
    || (named(path.parent(), "versions") && named(path.parent().and_then(Path::parent), "claude"))
}

/// Claude Code's own arguments, when command line `args` (program first) runs
/// it: by its executable, or as an interpreter's script — an npm install runs
/// `node …/@anthropic-ai/claude-code/cli.js`, or the `claude` link to it, and
/// any script named `claude` (a wrapper) counts too.
fn claude_args<'a>(args: &'a [&'a str]) -> Option<&'a [&'a str]> {
  let program = args.first()?;
  if is_claude_program(program) {
    return Some(&args[1..]);
  }
  let script = args.get(1)?;
  let by_node = Path::new(program).file_name()?.to_string_lossy().starts_with("node");
  (is_claude_program(script) || (by_node && script.contains("/@anthropic-ai/claude-code/")))
    .then(|| &args[2..])
}

/// Is command line `args` (program first) a foreground `claude` — not a
/// background job (`--bg`, Cira's) nor an attach client?
fn is_foreground_claude(args: &[&str]) -> bool {
  claude_args(args).is_some_and(|a| !a.contains(&"--bg") && a.first() != Some(&"attach"))
}

/// Does command line `args` run `claude` itself on conversation `uuid` in the
/// foreground — `claude --resume <uuid>` or `--session-id <uuid>`, as jc's own
/// panes start it?
pub fn runs_conversation(args: &[&str], uuid: &str) -> bool {
  if uuid.is_empty() || !is_foreground_claude(args) {
    return false;
  }
  let args = claude_args(args).unwrap_or_default();
  args.windows(2).any(|w| matches!(w[0], "--resume" | "-r" | "--session-id") && w[1] == uuid)
    || args.iter().any(|a| {
      a.strip_prefix("--resume=").or_else(|| a.strip_prefix("--session-id=")) == Some(uuid)
    })
}

/// Given the running processes (`pid`, command line) and Claude Code's
/// registry of interactive sessions (`pid`, current `sessionId`, from
/// `~/.claude/sessions/<pid>.json`), is a foreground `claude` on conversation
/// `uuid`? By its command line, or — after a `/clear` moved it to a uuid its
/// command line does not show — by the registry, for a pid that is still a
/// foreground `claude` (not a reused pid).
pub fn conversation_running_in(
  procs: &[(u32, Vec<&str>)],
  registry: &[(u32, String)],
  uuid: &str,
) -> bool {
  procs.iter().any(|(_, args)| runs_conversation(args, uuid))
    || registry.iter().any(|(pid, session)| {
      session == uuid && procs.iter().any(|(p, args)| p == pid && is_foreground_claude(args))
    })
}

/// Is any process on this machine running `claude` on `uuid` in the
/// foreground ([`conversation_running_in`])? jcc asks before migrating a
/// heading moved out of jc's section, so a conversation jc still runs is never
/// run twice (Cira PLAN TMUX-07). `false` when `ps` cannot run.
pub fn conversation_running(uuid: &str) -> bool {
  let Ok(out) =
    std::process::Command::new("/bin/ps").args(["-axww", "-o", "pid=,command="]).output()
  else {
    return false;
  };
  let text = String::from_utf8_lossy(&out.stdout);
  // One per line, so the two iterate together.
  let exes: Vec<Option<String>> =
    text.lines().map(|line| executable(line.split_whitespace().next()?.parse().ok()?)).collect();
  let procs: Vec<(u32, Vec<&str>)> = text
    .lines()
    .zip(&exes)
    .filter_map(|(line, exe)| {
      let mut words = line.split_whitespace();
      let pid = words.next()?.parse().ok()?;
      let mut args: Vec<&str> = words.collect();
      // The command line shows how it was started (`claude`, a link, any
      // path); the executable the kernel ran says what it is.
      if let (Some(exe), Some(first)) = (exe.as_deref(), args.first_mut())
        && is_claude_program(exe)
      {
        *first = exe;
      }
      Some((pid, args))
    })
    .collect();
  let registry = std::env::var_os("HOME")
    .and_then(|home| std::fs::read_dir(Path::new(&home).join(".claude/sessions")).ok())
    .into_iter()
    .flatten()
    .flatten()
    .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
    .filter_map(|e| {
      let v: serde_json::Value = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
      Some((
        u32::try_from(v.get("pid")?.as_u64()?).ok()?,
        v.get("sessionId")?.as_str()?.to_string(),
      ))
    })
    .collect::<Vec<_>>();
  conversation_running_in(&procs, &registry, uuid)
}

/// The file process `pid` executes, links resolved; `None` when it has gone
/// or cannot be asked.
fn executable(pid: u32) -> Option<String> {
  #[cfg(target_os = "macos")]
  {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let len =
      unsafe { libc::proc_pidpath(pid as libc::c_int, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (len > 0).then(|| String::from_utf8_lossy(&buf[..len as usize]).into_owned())
  }
  #[cfg(not(target_os = "macos"))]
  {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok().map(|p| p.to_string_lossy().into_owned())
  }
}

/// The conversations Claude Code's background jobs run, from
/// `<jobs>/<short>/state.json` (`sessionId`, and `resumeSessionId`, the one a
/// job was resumed from; measured 2026-11-12 on Claude Code 2.1.322). jc's
/// transcript picker leaves them out: they are Cira's (jcc's), and resuming one
/// in jc would run it twice. Unreadable entries are skipped.
pub fn background_conversations(jobs: &Path) -> std::collections::HashSet<String> {
  let Ok(dir) = std::fs::read_dir(jobs) else { return Default::default() };
  dir
    .flatten()
    .filter_map(|e| std::fs::read(e.path().join("state.json")).ok())
    .filter_map(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    .flat_map(|v| {
      ["sessionId", "resumeSessionId"]
        .map(|k| v.get(k).and_then(|id| id.as_str()).map(str::to_string))
    })
    .flatten()
    .collect()
}

/// `~/.claude/projects` under `home`.
fn projects_root(home: &Path) -> PathBuf {
  home.join(".claude/projects")
}

/// All transcript buckets for `project_path`: its root bucket followed by any
/// git-worktree sibling buckets (see [`is_worktree_bucket`]). The root bucket is
/// always first and always present in the result even if absent on disk.
pub fn session_dirs(home: &Path, project_path: &Path) -> Vec<PathBuf> {
  session_dirs_in(&projects_root(home), project_path)
}

/// [`session_dirs`] against an explicit `~/.claude/projects` directory (testable).
fn session_dirs_in(projects_root: &Path, project_path: &Path) -> Vec<PathBuf> {
  let encoded = encode_project_path(project_path);
  let mut dirs = vec![projects_root.join(&encoded)];
  if let Ok(read_dir) = std::fs::read_dir(projects_root) {
    for entry in read_dir.flatten() {
      // `file_type()` is served from the readdir entry (no extra stat on the
      // platforms we target), so filter on it before touching the filesystem.
      let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
      if is_dir && entry.file_name().to_str().is_some_and(|n| is_worktree_bucket(&encoded, n)) {
        dirs.push(entry.path());
      }
    }
  }
  dirs
}

/// Does a `<uuid>.jsonl` transcript exist in any of `dirs`?
///
/// This is the question that decides `--resume` vs `--session-id`: measured
/// 2026-08-27, `claude --resume <uuid>` on a UUID with no transcript prints
/// "No conversation found with session ID: <uuid>" and exits, whether the
/// transcript was garbage-collected or never written. `--session-id <uuid>` on
/// that same UUID starts a fresh conversation under it, so a session whose
/// transcript is gone is revived rather than retired.
pub fn transcript_in(dirs: &[PathBuf], uuid: &str) -> bool {
  let file = format!("{uuid}.jsonl");
  dirs.iter().any(|dir| dir.join(&file).exists())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_foreground_claude_on_the_uuid_is_running_it() {
    let u = "335b461a-eba5-485f-898c-f757fdb3f2bf";
    let runs = |line: &str| runs_conversation(&line.split_whitespace().collect::<Vec<_>>(), u);
    assert!(runs(&format!("claude --resume {u}")));
    assert!(runs(&format!(
      "/Users/x/.local/bin/claude --session-id {u} --dangerously-skip-permissions"
    )));
    assert!(runs(&format!("claude --resume={u}")));
    assert!(!runs(&format!("claude --bg -n x --resume {u}")), "Cira's job, not jc's");
    assert!(!runs(&format!("claude --resume {u}0")));
    assert!(!runs(&format!("vim {u}.jsonl")));
    assert!(!runs(&format!("grep --resume {u}")));
    assert!(!runs_conversation(&["claude", "--resume", ""], ""));
  }

  #[test]
  fn background_jobs_name_their_conversations() {
    let jobs = std::env::temp_dir().join(format!("jc-bg-jobs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&jobs);
    std::fs::create_dir_all(jobs.join("2c49e0b3")).unwrap();
    std::fs::create_dir_all(jobs.join("broken")).unwrap();
    std::fs::write(
      jobs.join("2c49e0b3/state.json"),
      r#"{"state":"working","sessionId":"new-uuid","resumeSessionId":"moved-uuid","daemonShort":"2c49e0b3"}"#,
    )
    .unwrap();
    std::fs::write(jobs.join("broken/state.json"), "{").unwrap();
    let found = background_conversations(&jobs);
    assert_eq!(found, ["new-uuid", "moved-uuid"].map(String::from).into_iter().collect());
    assert!(background_conversations(&jobs.join("missing")).is_empty());
    std::fs::remove_dir_all(&jobs).unwrap();
  }

  #[test]
  fn claude_is_found_however_it_was_installed() {
    let procs = vec![
      // npm: node runs the package's cli.js (or the `claude` link to it).
      (
        1,
        vec![
          "node",
          "/usr/local/lib/node_modules/@anthropic-ai/claude-code/cli.js",
          "--resume",
          "u-npm",
        ],
      ),
      (2, vec!["/opt/homebrew/bin/node", "/opt/homebrew/bin/claude", "--session-id", "u-link"]),
      // The native installer's versioned binary, run by its own path.
      (3, vec!["/Users/x/.local/share/claude/versions/2.1.323", "--resume", "u-native"]),
      // A wrapper script named claude, run by its interpreter.
      (6, vec!["/bin/sh", "/x/bin/claude", "--resume", "u-wrapped"]),
      // Not claude: another node script, and an attach client by any path.
      (4, vec!["node", "/x/tsserver.js", "--resume", "u-other"]),
      (5, vec!["/Users/x/.local/share/claude/versions/2.1.323", "attach", "c0ffee00"]),
    ];
    let registry = vec![];
    for uuid in ["u-npm", "u-link", "u-native", "u-wrapped"] {
      assert!(conversation_running_in(&procs, &registry, uuid), "{uuid} runs, and was not found");
    }
    assert!(!conversation_running_in(&procs, &registry, "u-other"), "a script that is not claude");
  }

  #[test]
  fn a_cleared_conversation_is_found_through_claudes_registry() {
    // jc started `claude --resume A`; a `/clear` moved it to B, which jc wrote
    // into the heading. Measured 2026-11-12: `~/.claude/sessions/<pid>.json`
    // follows the clear while the command line keeps A.
    let procs = vec![
      (21768, vec!["/Users/x/.local/bin/claude", "--resume", "a-uuid"]),
      (300, vec!["claude", "attach", "c0ffee00"]),
      (400, vec!["vim"]),
    ];
    let registry =
      vec![(21768, "b-uuid".to_string()), (300, "c-uuid".to_string()), (400, "d-uuid".to_string())];
    assert!(conversation_running_in(&procs, &registry, "b-uuid"), "found by the registry");
    assert!(conversation_running_in(&procs, &registry, "a-uuid"), "and by the command line");
    assert!(!conversation_running_in(&procs, &registry, "c-uuid"), "an attach client is Cira's");
    assert!(!conversation_running_in(&procs, &registry, "d-uuid"), "a reused pid is not claude");
    assert!(!conversation_running_in(&procs, &[(999, "b-uuid".into())], "b-uuid"), "a dead pid");
  }

  #[test]
  fn encode_maps_slash_and_dot_to_dash() {
    assert_eq!(encode_project_path(Path::new("/Users/jay/Dev/pgm")), "-Users-jay-Dev-pgm");
    // A worktree at <root>/.claude/worktrees/<b> collapses the `/.` into `--`.
    assert_eq!(
      encode_project_path(Path::new("/Users/jay/Dev/pgm/.claude/worktrees/plan-3d")),
      "-Users-jay-Dev-pgm--claude-worktrees-plan-3d"
    );
  }

  #[test]
  fn worktree_bucket_predicate() {
    let root = "-Users-jay-Dev-pgm";
    assert!(is_worktree_bucket(root, "-Users-jay-Dev-pgm--claude-worktrees-plan-3d"));
    // The root bucket itself is not a worktree bucket.
    assert!(!is_worktree_bucket(root, root));
    // A sibling project (pgm-2) and its worktrees must NOT match.
    assert!(!is_worktree_bucket(root, "-Users-jay-Dev-pgm-2"));
    assert!(!is_worktree_bucket(root, "-Users-jay-Dev-pgm-2--claude-worktrees-x"));
    // Unrelated project.
    assert!(!is_worktree_bucket(root, "-Users-jay-Dev-other"));
  }

  #[test]
  fn session_dirs_in_collects_root_plus_worktrees() {
    let base = std::env::temp_dir().join(format!("jc-sdtest-{}", std::process::id()));
    std::fs::remove_dir_all(&base).ok();
    let projects_root = base.join(".claude/projects");
    let names = [
      "-Users-jay-Dev-pgm",                           // root bucket
      "-Users-jay-Dev-pgm--claude-worktrees-plan-3d", // this project's worktree
      "-Users-jay-Dev-pgm--claude-worktrees-smt-c9c", // another worktree
      "-Users-jay-Dev-pgm-2",                         // sibling project (decoy)
      "-Users-jay-Dev-pgm-2--claude-worktrees-x",     // sibling's worktree (decoy)
      "-Users-jay-Dev-other",                         // unrelated (decoy)
    ];
    for n in names {
      std::fs::create_dir_all(projects_root.join(n)).unwrap();
    }

    let mut got: Vec<String> = session_dirs_in(&projects_root, Path::new("/Users/jay/Dev/pgm"))
      .into_iter()
      .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
      .collect();
    got.sort();
    assert_eq!(
      got,
      vec![
        "-Users-jay-Dev-pgm".to_string(),
        "-Users-jay-Dev-pgm--claude-worktrees-plan-3d".to_string(),
        "-Users-jay-Dev-pgm--claude-worktrees-smt-c9c".to_string(),
      ]
    );

    std::fs::remove_dir_all(&base).ok();
  }

  #[test]
  fn transcript_in_finds_uuid_in_worktree_bucket() {
    let home = std::env::temp_dir().join(format!("jc-txtest-{}", std::process::id()));
    std::fs::remove_dir_all(&home).ok();
    let wt = home.join(".claude/projects/-Users-jay-Dev-pgm--claude-worktrees-plan-3d");
    std::fs::create_dir_all(&wt).unwrap();
    let uuid = "d98aae01-acc0-44db-acfd-b0e68c2d902b";
    std::fs::write(wt.join(format!("{uuid}.jsonl")), b"{}").unwrap();

    let dirs = session_dirs(&home, Path::new("/Users/jay/Dev/pgm"));
    assert!(transcript_in(&dirs, uuid));
    assert!(!transcript_in(&dirs, "00000000-0000-0000-0000-000000000000"));

    std::fs::remove_dir_all(&home).ok();
  }

  #[test]
  fn session_dirs_in_returns_root_when_projects_root_missing() {
    let projects_root = std::env::temp_dir().join(format!("jc-missing-{}", std::process::id()));
    std::fs::remove_dir_all(&projects_root).ok();
    let dirs = session_dirs_in(&projects_root, Path::new("/Users/jay/Dev/pgm"));
    assert_eq!(dirs, vec![projects_root.join("-Users-jay-Dev-pgm")]);
  }
}
