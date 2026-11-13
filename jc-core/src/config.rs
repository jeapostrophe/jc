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

/// JC's config and state directory: `$JC_CONFIG_DIR` when set and non-empty, else
/// `~/.config/jc`. The override lets a second JC build run beside the installed one
/// without sharing its `state.toml` or its single-instance socket (`jc.sock`).
pub fn config_dir() -> PathBuf {
  config_dir_from(std::env::var_os("JC_CONFIG_DIR"), dirs::home_dir())
}

fn config_dir_from(over: Option<std::ffi::OsString>, home: Option<PathBuf>) -> PathBuf {
  match over.filter(|d| !d.is_empty()) {
    Some(dir) => PathBuf::from(dir),
    None => home.expect("could not determine home directory").join(".config/jc"),
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
  fs::write(&path, contents).with_context(|| format!("failed to write {}", path.display()))?;
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
  fn jc_config_dir_overrides_home_config() {
    let home = Path::new("/home/someone");
    assert_eq!(
      config_dir_from(Some("/tmp/jc-scratch".into()), Some(home.to_path_buf())),
      PathBuf::from("/tmp/jc-scratch")
    );
    assert_eq!(config_dir_from(Some("".into()), Some(home.to_path_buf())), home.join(".config/jc"));
    assert_eq!(config_dir_from(None, Some(home.to_path_buf())), home.join(".config/jc"));
  }
}
