//! Which app this process is (Cira PLAN TMUX-07): `jc`, which runs each Claude
//! session in a terminal of its own, or `jcc`, Cira's client, where Cira owns
//! each session and jc attaches to it. One binary and one codebase; the flavor
//! is chosen once at start ([`Flavor::chosen`]) and names the app. What each
//! app does differently is in ARCH.md ("Two Apps: jc and jcc"); what they must
//! not share is decided here, so both run side by side on the same projects:
//! the config directory ([`Flavor::config_subdir`]), the hook URLs
//! ([`Flavor::hook_prefix`]), which headings are a session
//! ([`Flavor::owns`], [`Flavor::id_kind`]) and the TODO file
//! ([`Flavor::todo_path`]).

use crate::cira::Status;
use crate::todo::IdKind;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
  /// `jc`: each Claude pane runs `claude --session-id|--resume <uuid>` in jc's
  /// own terminal; its headings carry `> uuid=` in the project's TODO.md.
  #[default]
  Jc,
  /// `jcc`: Cira's client. Each Claude pane attaches to the job Cira names;
  /// its headings carry `> cira=` in its own todo.md in Cira's store.
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

  /// The flavor this process runs as: `JC_FLAVOR` (`jc` or `jcc`) when set,
  /// for a development build (`cargo run` names its program `jc-app`), else
  /// the program's name ([`Flavor::from_program`]). `Err` names a
  /// `JC_FLAVOR` that is neither, along with the program's flavor, used instead.
  pub fn chosen(argv0: Option<&OsStr>, env: Option<&OsStr>) -> Result<Self, (Self, String)> {
    let program = Self::from_program(argv0);
    match env.map(|v| v.to_string_lossy()) {
      None => Ok(program),
      Some(v) if v == "jc" => Ok(Self::Jc),
      Some(v) if v == "jcc" => Ok(Self::Jcc),
      Some(v) => Err((program, format!("JC_FLAVOR={v} is neither jc nor jcc"))),
    }
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

  /// The TODO file this app keeps for the project at `project`, the only one
  /// it reads or writes: jc's is the project's own `TODO.md`; jcc's is
  /// `todo.md` beside the scratch pad of the Cira project holding `project`
  /// (`$CIRA_HOME/projects/<name>/todo.md`, [`Status::todo_for`]), so the two
  /// apps never write one file. `None` for jcc until a status read names that
  /// project.
  pub fn todo_path(self, project: &Path, status: Option<&Status>) -> Option<PathBuf> {
    match self {
      Self::Jc => Some(project.join("TODO.md")),
      Self::Jcc => status?.todo_for(project),
    }
  }

  /// The id line this app writes into the headings it makes.
  pub fn id_kind(self) -> IdKind {
    match self {
      Self::Jc => IdKind::Uuid,
      Self::Jcc => IdKind::Cira,
    }
  }

  /// The top-level headings whose `##` headings are sessions; the first is
  /// the one this app writes. jc's is `# Claude`. jcc's todo.md is its own
  /// (Q-199), so it writes `# Cira`, and still reads `# Claude`: files it made
  /// before, and blocks pasted from jc's TODO.md.
  pub fn sections(self) -> &'static [&'static str] {
    match self {
      Self::Jc => &["# Claude"],
      Self::Jcc => &["# Cira", "# Claude"],
    }
  }

  /// Does a session heading with an id line of `kind` name one of its
  /// sessions? jc never acts on a `> cira=` heading; jcc's own are `> cira=`,
  /// and a `> uuid=` heading pasted into its file is a conversation it
  /// migrates to Cira (Cira PLAN TMUX-01).
  pub fn owns(self, kind: IdKind) -> bool {
    match self {
      Self::Jc => kind == IdKind::Uuid,
      Self::Jcc => true,
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
  fn jc_flavor_overrides_the_program_name() {
    let dev = Some(OsStr::new("target/debug/jc-app"));
    let jcc = Some(OsStr::new("/x/jcc.app/Contents/MacOS/jcc"));
    assert_eq!(Flavor::chosen(dev, None), Ok(Flavor::Jc));
    assert_eq!(Flavor::chosen(dev, Some(OsStr::new("jcc"))), Ok(Flavor::Jcc));
    assert_eq!(Flavor::chosen(jcc, Some(OsStr::new("jc"))), Ok(Flavor::Jc));
    let (fallback, why) = Flavor::chosen(jcc, Some(OsStr::new("JCC"))).unwrap_err();
    assert_eq!(fallback, Flavor::Jcc, "a bad value falls back to the program's name");
    assert!(why.contains("JC_FLAVOR=JCC"), "{why}");
  }

  #[test]
  fn jc_and_jcc_share_no_config_dir_or_hook() {
    assert_ne!(Flavor::Jc.config_subdir(), Flavor::Jcc.config_subdir());
    let (jc, jcc) = (Flavor::Jc.hook_prefix(), Flavor::Jcc.hook_prefix());
    // Each app finds its own hook entries by substring: neither may match the other's.
    assert!(!jc.contains(jcc) && !jcc.contains(jc), "{jc} vs {jcc}");
  }

  /// A status read naming the Cira project `cira-proj` at `/w/proj`, in a
  /// store at `/store`.
  fn status() -> Status {
    Status::parse(br#"{"home":"/store","projects":[{"name":"cira-proj","path":"/w/proj"}]}"#)
      .unwrap()
  }

  #[test]
  fn jc_keeps_the_projects_todo_and_never_jccs() {
    let project = Path::new("/w/proj");
    for status in [None, Some(status())] {
      let jc = Flavor::Jc.todo_path(project, status.as_ref()).unwrap();
      assert_eq!(jc, Path::new("/w/proj/TODO.md"));
      assert!(!jc.starts_with("/store"), "jc never reads or writes Cira's store");
    }
  }

  #[test]
  fn jccs_todo_is_beside_the_scratch_pad_and_never_the_projects() {
    let status = status();
    let at = |p: &str| Flavor::Jcc.todo_path(Path::new(p), Some(&status));
    assert_eq!(at("/w/proj").unwrap(), Path::new("/store/projects/cira-proj/todo.md"));
    assert_eq!(at("/w/proj/sub").unwrap(), Path::new("/store/projects/cira-proj/todo.md"));
    assert_eq!(at("/w/other"), None, "no Cira project holds it yet");
    assert_eq!(Flavor::Jcc.todo_path(Path::new("/w/proj"), None), None, "no read yet");
    // Whatever the read, jcc's file is never one under the project.
    for p in ["/w/proj", "/w/proj/sub", "/w/other"] {
      assert!(at(p).is_none_or(|todo| !todo.starts_with(p)), "{p}");
    }
  }
}
