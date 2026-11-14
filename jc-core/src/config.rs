use crate::flavor::Flavor;
use crate::model::{Project, WindowLayout};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// User-editable configuration (`config.toml` in [`config_dir`]).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppConfig {
  pub editor: String,
  pub window: WindowLayout,
}

/// App-managed state (`state.toml` in [`config_dir`]).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppState {
  pub projects: Vec<Project>,
}

impl AppState {
  /// The path of the first registered project, falling back to the current directory.
  pub fn project_path(&self) -> PathBuf {
    self
      .projects
      .first()
      .map(|p| p.path.clone())
      .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
  }

  /// Find or create a project for the given path. Returns a mutable
  /// reference to the existing project if already registered.
  pub fn register_project(&mut self, path: &Path) -> &mut Project {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let pos = self.projects.iter().position(|p| p.path == canonical);
    match pos {
      Some(i) => &mut self.projects[i],
      None => {
        let project = Project::from(canonical);
        self.projects.push(project);
        self.projects.last_mut().unwrap()
      }
    }
  }
}

/// The app's config and state directory: `$JC_CONFIG_DIR` when set and
/// non-empty, else the flavor's own under `$HOME` (`~/.config/jc`, or
/// `~/.config/jcc`: [`Flavor::config_subdir`]). So `jc` and `jcc` never share a
/// `state.toml` (their project lists), a `sessions.toml` or the single-instance
/// socket (`jc.sock`), and the override lets a development build run beside both.
pub fn config_dir() -> PathBuf {
  config_dir_from(Flavor::current(), std::env::var_os("JC_CONFIG_DIR"), dirs::home_dir())
}

fn config_dir_from(
  flavor: Flavor,
  over: Option<std::ffi::OsString>,
  home: Option<PathBuf>,
) -> PathBuf {
  match over.filter(|d| !d.is_empty()) {
    Some(dir) => PathBuf::from(dir),
    None => home.expect("could not determine home directory").join(flavor.config_subdir()),
  }
}

fn ensure_config_dir() -> Result<PathBuf> {
  let dir = config_dir();
  fs::create_dir_all(&dir)
    .with_context(|| format!("failed to create config directory: {}", dir.display()))?;
  Ok(dir)
}

pub fn config_path() -> PathBuf {
  config_dir().join("config.toml")
}

pub fn state_path() -> PathBuf {
  config_dir().join("state.toml")
}

/// What jc remembers of each Cira session (`crate::lifecycle::Records`).
pub fn sessions_path() -> PathBuf {
  config_dir().join("sessions.toml")
}

pub fn theme_path() -> PathBuf {
  config_dir().join("theme.toml")
}

/// The file's bytes, `None` when there is none.
fn read_config(path: &Path) -> Result<Option<Vec<u8>>> {
  match fs::read(path) {
    Ok(bytes) => Ok(Some(bytes)),
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
    Err(e) => Err(anyhow::anyhow!("failed to read {}: {e}", path.display())),
  }
}

/// TOML that is not UTF-8 does not parse either.
fn parse_toml<T: serde::de::DeserializeOwned>(bytes: Vec<u8>) -> Result<T> {
  Ok(toml::from_str(&String::from_utf8(bytes)?)?)
}

fn load_toml<T: Default + serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
  match read_config(path)? {
    Some(bytes) => parse_toml(bytes).with_context(|| format!("failed to parse {}", path.display())),
    None => Ok(T::default()),
  }
}

pub fn load_theme() -> Result<crate::theme::ThemeConfig> {
  load_toml(&theme_path())
}

pub fn load_config() -> Result<AppConfig> {
  load_toml(&config_path())
}

fn save_toml<T: Serialize>(filename: &str, value: &T) -> Result<()> {
  let dir = ensure_config_dir()?;
  let path = dir.join(filename);
  let contents =
    toml::to_string_pretty(value).with_context(|| format!("failed to serialize {filename}"))?;
  crate::replace::write_replacing(&path, contents.as_bytes())
    .with_context(|| format!("failed to write {}", path.display()))
}

pub fn save_config(config: &AppConfig) -> Result<()> {
  save_toml("config.toml", config)
}

/// What a changed `state.toml` asks of the projects an app has open (Cira
/// PLAN TMUX-10): those it lists that are not open, and those open that it no
/// longer lists, each in order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StateChange {
  pub added: Vec<PathBuf>,
  pub removed: Vec<PathBuf>,
}

/// What an app does about an open project `state.toml` no longer lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leaving {
  /// Keep it: it is the only project open.
  Stay,
  /// Ask Jay first.
  Ask,
  /// Ask, once the dialog already shown closes (it is re-evaluated then).
  Wait,
  Close,
}

/// For a project `state.toml` dropped: with `open` projects open, `sessions`
/// in it (jc's end when it closes, busy or idle), its buffers that could not
/// be saved (`unsaved`), and whether a dialog is up already. jcc's sessions
/// keep running in Cira, so it asks only about unsaved buffers.
pub fn leaving(
  flavor: Flavor,
  open: usize,
  sessions: usize,
  unsaved: bool,
  dialog_open: bool,
) -> Leaving {
  if open <= 1 {
    return Leaving::Stay;
  }
  let ask = unsaved || (flavor == Flavor::Jc && sessions > 0);
  match (ask, dialog_open) {
    (false, _) => Leaving::Close,
    (true, true) => Leaving::Wait,
    (true, false) => Leaving::Ask,
  }
}

/// Which `state.toml` read is the newest, so an older one that finishes
/// later is not applied (review P5).
#[derive(Debug, Default)]
pub struct Reads {
  issued: u64,
}

impl Reads {
  /// Number a new read.
  pub fn issue(&mut self) -> u64 {
    self.issued += 1;
    self.issued
  }

  /// Whether read `n` is the newest issued.
  pub fn is_latest(&self, n: u64) -> bool {
    n == self.issued
  }
}

/// A `state.toml`'s projects as the app opens them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Resolved {
  /// Canonical paths, in order.
  pub listed: Vec<PathBuf>,
  /// Listed paths that do not exist, as written.
  pub missing: Vec<PathBuf>,
}

impl Resolved {
  /// Compare with the projects open now (canonical paths). A missing path
  /// is not opened, and an open project listed only under it is not closed.
  pub fn change_from(&self, open: &[PathBuf]) -> StateChange {
    let listed = AppState { projects: self.listed.iter().cloned().map(Project::from).collect() };
    let mut change = listed.change_from(open);
    change.removed.retain(|p| !self.missing.contains(p));
    change
  }
}

impl AppState {
  /// Canonicalize each listed path; does file-system calls.
  pub fn resolve(&self) -> Resolved {
    let mut resolved = Resolved::default();
    for path in self.projects.iter().map(|p| &p.path) {
      match fs::canonicalize(path) {
        Ok(path) if !resolved.listed.contains(&path) => resolved.listed.push(path),
        Ok(_) => {}
        Err(_) => resolved.missing.push(path.clone()),
      }
    }
    resolved
  }

  /// Compare with the projects open now.
  pub fn change_from(&self, open: &[PathBuf]) -> StateChange {
    let listed: Vec<&PathBuf> = self.projects.iter().map(|p| &p.path).collect();
    StateChange {
      added: listed.iter().filter(|p| !open.contains(p)).map(|p| (*p).clone()).collect(),
      removed: open.iter().filter(|p| !listed.contains(p)).cloned().collect(),
    }
  }
}

pub fn load_state() -> Result<AppState> {
  load_toml(&state_path())
}

pub fn save_state(state: &AppState) -> Result<()> {
  save_toml("state.toml", state)
}

/// The records, and where a `sessions.toml` that did not parse was kept.
pub fn load_sessions() -> Result<(crate::lifecycle::Records, Option<PathBuf>)> {
  load_toml_keeping_bad(&sessions_path(), SystemTime::now())
}

/// [`load_toml`] for a file jc rewrites from what it loaded: one that does not
/// parse is renamed `<name>.bad-<unix secs>`, and the defaults come back with
/// that path, so saving them loses nothing. An error means there is nothing
/// safe to save over: the file could not be read, or could not be moved.
fn load_toml_keeping_bad<T: Default + serde::de::DeserializeOwned>(
  path: &Path,
  now: SystemTime,
) -> Result<(T, Option<PathBuf>)> {
  let Some(bytes) = read_config(path)? else { return Ok((T::default(), None)) };
  match parse_toml(bytes) {
    Ok(value) => Ok((value, None)),
    Err(e) => {
      let secs = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
      let mut aside = path.as_os_str().to_owned();
      aside.push(format!(".bad-{secs}"));
      let aside = PathBuf::from(aside);
      fs::rename(path, &aside).map_err(|io| {
        anyhow::anyhow!("failed to parse {} ({e}), and to move it aside: {io}", path.display())
      })?;
      Ok((T::default(), Some(aside)))
    }
  }
}

pub fn save_sessions(records: &crate::lifecycle::Records) -> Result<()> {
  save_toml("sessions.toml", records)
}

#[cfg(test)]
mod tests {
  use super::*;

  /// TMUX-10 (Jay: "live-react to the state.toml file changing"). An app's
  /// own write lists exactly what it has open, so it asks for nothing: no
  /// echo.
  #[test]
  fn a_dropped_project_closes_unless_that_loses_something() {
    use Leaving::*;
    assert_eq!(leaving(Flavor::Jcc, 2, 3, false, false), Close, "jcc's sessions run on in Cira");
    assert_eq!(leaving(Flavor::Jcc, 2, 0, false, true), Close, "not held up by another dialog");
    assert_eq!(leaving(Flavor::Jcc, 2, 0, true, false), Ask, "an unsaved buffer");
    assert_eq!(leaving(Flavor::Jc, 2, 1, false, false), Ask, "jc's sessions would end");
    assert_eq!(leaving(Flavor::Jc, 2, 0, false, false), Close);
    assert_eq!(leaving(Flavor::Jc, 2, 1, false, true), Wait, "asked once the dialog closes");
    assert_eq!(leaving(Flavor::Jc, 1, 0, false, false), Stay, "the last project");
  }

  #[test]
  fn only_the_newest_read_is_applied() {
    let mut reads = Reads::default();
    let first = reads.issue();
    let second = reads.issue();
    assert!(!reads.is_latest(first), "an older read that finishes later");
    assert!(reads.is_latest(second));
  }

  /// Review: paths are compared as the app opens them (canonical), and one
  /// that does not exist is neither opened nor a reason to close.
  #[test]
  fn listed_paths_are_canonical_and_missing_ones_are_set_aside() {
    let dir = std::env::temp_dir().join(format!("jc-state-{}", std::process::id()));
    let real = dir.join("real");
    std::fs::create_dir_all(&real).unwrap();
    let real = fs::canonicalize(&real).unwrap();
    let state = AppState {
      projects: [dir.join("real/../real"), dir.join("gone")]
        .into_iter()
        .map(Project::from)
        .collect(),
    };
    let resolved = state.resolve();
    assert_eq!(resolved.listed, std::slice::from_ref(&real));
    assert_eq!(resolved.missing, [dir.join("gone")]);
    let change = resolved.change_from(&[real.clone(), dir.join("gone")]);
    assert_eq!(change, StateChange::default(), "an open project whose directory went stays");
    let _ = fs::remove_dir_all(&dir);
  }

  #[test]
  fn a_changed_state_file_names_what_to_open_and_close() {
    let state = |paths: &[&str]| AppState {
      projects: paths.iter().map(|p| Project::from(PathBuf::from(p))).collect(),
    };
    let open = [PathBuf::from("/w/a"), PathBuf::from("/w/b")];
    assert_eq!(
      state(&["/w/a", "/w/b"]).change_from(&open),
      StateChange::default(),
      "its own write"
    );
    assert_eq!(state(&["/w/b", "/w/a"]).change_from(&open), StateChange::default(), "order alone");
    let change = state(&["/w/b", "/w/c"]).change_from(&open);
    assert_eq!(change.added, [PathBuf::from("/w/c")]);
    assert_eq!(change.removed, [PathBuf::from("/w/a")]);
  }

  #[test]
  fn a_records_file_that_does_not_parse_is_kept_aside() {
    let dir = std::env::temp_dir().join(format!("jc-bad-records-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sessions.toml");
    let bad = "[sessions.c-1\nmark = \"Disabled\"\n";
    fs::write(&path, bad).unwrap();
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1234);

    let (records, kept) = load_toml_keeping_bad::<crate::lifecycle::Records>(&path, now).unwrap();
    let aside = dir.join("sessions.toml.bad-1234");
    assert_eq!(records, Default::default());
    assert_eq!(kept.as_deref(), Some(aside.as_path()));
    assert_eq!(
      fs::read_to_string(&aside).ok().as_deref(),
      Some(bad),
      "the bad file is kept aside before jc saves its defaults over it"
    );
    assert!(!path.exists(), "the bad file is moved, not copied");

    // Bytes that are not UTF-8 (a write cut off inside a character) did not
    // parse either.
    fs::write(&path, b"[sessions.c-1]\nlabel = \"caf\xc3").unwrap();
    let (_, kept) = load_toml_keeping_bad::<crate::lifecycle::Records>(&path, now).unwrap();
    assert_eq!(kept.as_deref(), Some(aside.as_path()), "a non-UTF-8 file is kept aside");

    // A file that parses, or none at all, is not moved.
    fs::write(&path, "").unwrap();
    let (_, kept) = load_toml_keeping_bad::<crate::lifecycle::Records>(&path, now).unwrap();
    assert!(path.exists() && kept.is_none());
    fs::remove_file(&path).unwrap();
    let (_, kept) = load_toml_keeping_bad::<crate::lifecycle::Records>(&path, now).unwrap();
    assert!(kept.is_none());
    fs::remove_dir_all(&dir).unwrap();
  }

  #[test]
  fn jc_config_dir_overrides_home_config() {
    let home = Path::new("/home/someone");
    for flavor in [Flavor::Jc, Flavor::Jcc] {
      assert_eq!(
        config_dir_from(flavor, Some("/tmp/jc-scratch".into()), Some(home.to_path_buf())),
        PathBuf::from("/tmp/jc-scratch")
      );
    }
    assert_eq!(
      config_dir_from(Flavor::Jc, Some("".into()), Some(home.to_path_buf())),
      home.join(".config/jc")
    );
    assert_eq!(
      config_dir_from(Flavor::Jc, None, Some(home.to_path_buf())),
      home.join(".config/jc")
    );
  }

  #[test]
  fn jcc_keeps_its_own_config_dir_beside_jc() {
    let home = Some(PathBuf::from("/home/someone"));
    let jc = config_dir_from(Flavor::Jc, None, home.clone());
    let jcc = config_dir_from(Flavor::Jcc, None, home);
    assert_eq!(jcc, Path::new("/home/someone/.config/jcc"));
    assert!(
      !jcc.starts_with(&jc) && !jc.starts_with(&jcc),
      "{} vs {}",
      jc.display(),
      jcc.display()
    );
  }
}
