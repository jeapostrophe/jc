//! Which app this process is (Cira PLAN TMUX-07): `jc`, which runs each Claude
//! session in a terminal of its own, or `jcc`, Cira's client, where Cira owns
//! each session and jc attaches to it. One binary and one codebase; the flavor
//! is chosen once at start from the program's name ([`Flavor::from_program`])
//! and decides everything the two must not share, so both can run side by side
//! on the same projects without acting on each other's sessions or state:
//!
//! - the config directory (`state.toml`, `sessions.toml`, the single-instance
//!   `jc.sock`): [`Flavor::config_subdir`];
//! - the hook URLs each installs into a project's `.claude/settings.local.json`,
//!   which are also how each finds and removes only its own: [`Flavor::hook_prefix`];
//! - the TODO.md section holding its session headings, and the id line those
//!   headings carry: [`Flavor::section`], [`Flavor::id_kind`];
//! - who runs a session: jc starts `claude` itself ([`Flavor::claude_command`]),
//!   jcc asks Cira ([`Flavor::cira`]).

use crate::claude::Launch;
use crate::todo::IdKind;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
  /// `jc`: each Claude pane runs `claude --session-id|--resume <uuid>` in jc's
  /// own terminal; its headings carry `> uuid=` under `# Claude`.
  #[default]
  Jc,
  /// `jcc`: Cira's client. Each Claude pane attaches to the job Cira names;
  /// its headings carry `> cira=` under `# Cira`.
  Jcc,
}

static CURRENT: OnceLock<Flavor> = OnceLock::new();

impl Flavor {
  /// The flavor a program named `argv0` runs as: `jcc` when its file name is
  /// exactly `jcc` (the `jcc.app` bundle's executable, or a link of that
  /// name), else `jc`.
  pub fn from_program(argv0: Option<&OsStr>) -> Self {
    let name = argv0.and_then(|a| Path::new(a).file_name());
    if name == Some(OsStr::new("jcc")) { Self::Jcc } else { Self::Jc }
  }

  /// Make `self` this process's flavor ([`Flavor::current`]); the first call
  /// wins. Returns the flavor in effect.
  pub fn init(self) -> Self {
    *CURRENT.get_or_init(|| self)
  }

  /// This process's flavor: what [`Flavor::init`] set, else `jc`.
  pub fn current() -> Self {
    CURRENT.get().copied().unwrap_or_default()
  }

  /// The app's name, as its window, bundle and command line show it.
  pub fn name(self) -> &'static str {
    match self {
      Self::Jc => "jc",
      Self::Jcc => "jcc",
    }
  }

  /// The config directory under `$HOME` when `JC_CONFIG_DIR` is unset.
  pub fn config_subdir(self) -> &'static str {
    match self {
      Self::Jc => ".config/jc",
      Self::Jcc => ".config/jcc",
    }
  }

  /// The path prefix of the hook URLs this app installs. Each app recognises
  /// (and removes) only entries carrying its own prefix, and neither prefix
  /// contains the other, so a running `jc` (any version) never removes `jcc`'s
  /// hooks, nor `jcc` jc's.
  pub fn hook_prefix(self) -> &'static str {
    match self {
      Self::Jc => "/jc-hook/",
      Self::Jcc => "/jcc-hook/",
    }
  }

  /// The top-level TODO.md section (`# <section>`) whose `##` headings are
  /// this app's sessions. Everything else in the file is text to it, so jc
  /// and jcc never act on the same heading, and a `jc` that predates jcc
  /// (it reads only `# Claude`) never sees jcc's.
  pub fn section(self) -> &'static str {
    match self {
      Self::Jc => "Claude",
      Self::Jcc => "Cira",
    }
  }

  /// The id line this app writes into the headings it makes.
  pub fn id_kind(self) -> IdKind {
    match self {
      Self::Jc => IdKind::Uuid,
      Self::Jcc => IdKind::Cira,
    }
  }

  /// Does a heading in this app's section with an id line of `kind` name one
  /// of its sessions? jc never acts on a `> cira=` heading, wherever it is;
  /// jcc's own are `> cira=`, and a `> uuid=` heading moved into its section
  /// is a conversation it migrates to Cira (Cira PLAN TMUX-01).
  pub fn owns(self, kind: IdKind) -> bool {
    match self {
      Self::Jc => kind == IdKind::Uuid,
      Self::Jcc => true,
    }
  }

  /// The `cira` client, for jcc only: jc never asks Cira for anything.
  pub fn cira(self) -> Option<crate::cira::Cira> {
    match self {
      Self::Jc => None,
      Self::Jcc => Some(crate::cira::Cira::default()),
    }
  }

  /// What a Claude pane runs for the heading bound to `uuid`, for jc only
  /// (`claude --session-id|--resume <uuid>`, see [`Launch`]). `None` for
  /// jcc, whose panes run only the `claude attach` of the job Cira names.
  pub fn claude_command(self, uuid: &str, launch: Launch, dangerous: bool) -> Option<Vec<String>> {
    match self {
      Self::Jc => Some(crate::claude::claude_argv(uuid, launch, dangerous)),
      Self::Jcc => None,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn the_program_name_chooses_the_flavor() {
    let of = |s: &str| Flavor::from_program(Some(OsStr::new(s)));
    assert_eq!(of("/Applications/jcc.app/Contents/MacOS/jcc"), Flavor::Jcc);
    assert_eq!(of("jcc"), Flavor::Jcc);
    assert_eq!(of("/x/target/release/jc.app/Contents/MacOS/jc-app"), Flavor::Jc);
    assert_eq!(of("jc-app"), Flavor::Jc);
    assert_eq!(of("/x/jcc-old/jc-app"), Flavor::Jc, "only the file name counts");
    assert_eq!(of("jccx"), Flavor::Jc);
    assert_eq!(Flavor::from_program(None), Flavor::Jc);
  }

  #[test]
  fn jc_and_jcc_share_no_config_dir_hook_or_section() {
    assert_ne!(Flavor::Jc.config_subdir(), Flavor::Jcc.config_subdir());
    assert_ne!(Flavor::Jc.section(), Flavor::Jcc.section());
    let (jc, jcc) = (Flavor::Jc.hook_prefix(), Flavor::Jcc.hook_prefix());
    // Each app finds its own hook entries by substring: neither may match the other's.
    assert!(!jc.contains(jcc) && !jcc.contains(jc), "{jc} vs {jcc}");
  }

  #[test]
  fn jc_runs_claude_itself_and_never_asks_cira() {
    let jc = Flavor::Jc;
    assert!(jc.cira().is_none(), "jc has no cira client");
    assert_eq!(
      jc.claude_command("u-1", Launch::Resume, true).unwrap(),
      ["claude", "--resume", "u-1", "--dangerously-skip-permissions"]
    );
    assert_eq!(
      jc.claude_command("u-1", Launch::New, false).unwrap(),
      ["claude", "--session-id", "u-1"]
    );
    assert!(!jc.owns(IdKind::Cira), "jc never acts on a > cira= heading");
    assert_eq!(jc.id_kind(), IdKind::Uuid);
  }

  #[test]
  fn jcc_asks_cira_and_never_runs_claude_itself() {
    let jcc = Flavor::Jcc;
    assert!(jcc.cira().is_some());
    assert_eq!(jcc.claude_command("u-1", Launch::Resume, true), None);
    assert_eq!(jcc.claude_command("u-1", Launch::New, false), None);
    assert_eq!(jcc.id_kind(), IdKind::Cira);
  }
}
