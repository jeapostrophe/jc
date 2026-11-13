use crate::flavor::Flavor;
use crate::model::{Project, WindowLayout};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
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
  write_replacing(&path, contents.as_bytes(), |file, bytes| file.write_all(bytes))
    .with_context(|| format!("failed to write {}", path.display()))
}

/// Replace `path` with `bytes`, written by `write` (a seam for tests) to a
/// temporary file beside it, synced, then renamed over it: a save cut off
/// part way (jc killed, the disk full) leaves the old file whole, never a
/// truncated one that would not parse. The temporary name is per process, so
/// each file must have one writer thread.
fn write_replacing(
  path: &Path,
  bytes: &[u8],
  write: impl FnOnce(&mut fs::File, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
  let dir = path.parent().unwrap_or(Path::new("."));
  let mut name = std::ffi::OsString::from(".");
  name.push(path.file_name().unwrap_or_default());
  name.push(format!(".tmp-{}", std::process::id()));
  let temp = dir.join(name);
  let written = (|| {
    let mut file = fs::File::create(&temp)?;
    write(&mut file, bytes)?;
    file.sync_all()?;
    fs::rename(&temp, path)
  })();
  if written.is_err() {
    let _ = fs::remove_file(&temp);
  }
  written?;
  // The rename is durable once the directory is synced; the file is replaced
  // either way, so a filesystem that cannot sync a directory fails no save.
  let _ = fs::File::open(dir).and_then(|d| d.sync_all());
  Ok(())
}

pub fn save_config(config: &AppConfig) -> Result<()> {
  save_toml("config.toml", config)
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
  fn a_save_cut_off_part_way_leaves_the_old_file_whole() {
    let dir = std::env::temp_dir().join(format!("jc-cut-save-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sessions.toml");
    fs::write(&path, "old").unwrap();

    let cut = write_replacing(&path, b"new contents", |file, bytes| {
      file.write_all(&bytes[..3])?;
      Err(std::io::Error::other("killed mid-write"))
    });
    assert!(cut.is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "old", "a cut-off save leaves the old file");
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "and no temporary file behind");

    write_replacing(&path, b"new contents", |file, bytes| file.write_all(bytes)).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "new contents");
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
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
