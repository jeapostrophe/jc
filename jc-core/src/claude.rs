//! Locating Claude Code session transcripts on disk.
//!
//! Claude Code stores each session as `~/.claude/projects/<bucket>/<uuid>.jsonl`,
//! where `<bucket>` is the launch working directory with every character that is
//! not ASCII alphanumeric or `-` replaced by `-`. A git worktree therefore lands
//! in its *own* sibling bucket: the harness creates worktrees at
//! `<root>/.claude/worktrees/<branch>`, which encodes to
//! `<encoded_root>--claude-worktrees-<branch>`. A project's live sessions can thus
//! be spread across the root bucket and any number of worktree buckets, so a
//! liveness/discovery check that looks only at the root bucket wrongly concludes a
//! worktree session's transcript is gone. These helpers enumerate all of a
//! project's buckets.

use std::path::{Path, PathBuf};

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
pub fn transcript_in(dirs: &[PathBuf], uuid: &str) -> bool {
  let file = format!("{uuid}.jsonl");
  dirs.iter().any(|dir| dir.join(&file).exists())
}

#[cfg(test)]
mod tests {
  use super::*;

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
  fn session_dirs_in_returns_root_when_projects_root_missing() {
    let projects_root = std::env::temp_dir().join(format!("jc-missing-{}", std::process::id()));
    std::fs::remove_dir_all(&projects_root).ok();
    let dirs = session_dirs_in(&projects_root, Path::new("/Users/jay/Dev/pgm"));
    assert_eq!(dirs, vec![projects_root.join("-Users-jay-Dev-pgm")]);
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
}
