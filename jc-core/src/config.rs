use crate::model::{Project, WindowLayout};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

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

fn load_toml<T: Default + serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
  match fs::read_to_string(path) {
    Ok(contents) => {
      toml::from_str(&contents).with_context(|| format!("failed to parse {}", path.display()))
    }
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
    Err(e) => Err(anyhow::anyhow!("failed to read {}: {e}", path.display())),
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

pub fn load_sessions() -> Result<crate::lifecycle::Records> {
  load_toml(&sessions_path())
}

pub fn save_sessions(records: &crate::lifecycle::Records) -> Result<()> {
  save_toml("sessions.toml", records)
}

#[cfg(test)]
mod tests {
  use super::*;

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
