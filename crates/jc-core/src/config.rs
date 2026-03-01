use crate::model::{Project, WindowLayout};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// User-editable configuration (~/.config/jc/config.toml).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppConfig {
  pub editor: String,
  pub window: WindowLayout,
}

/// App-managed state (~/.config/jc/state.toml).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppState {
  pub projects: Vec<Project>,
}

impl AppState {
  /// Find or create a project for the given path. Returns a mutable
  /// reference to the existing project if already registered.
  pub fn register_project(&mut self, path: &Path) -> &mut Project {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let pos = self.projects.iter().position(|p| p.path == canonical);
    match pos {
      Some(i) => &mut self.projects[i],
      None => {
        let project = Project::from_path(&canonical);
        self.projects.push(project);
        self.projects.last_mut().unwrap()
      }
    }
  }
}

fn config_dir() -> PathBuf {
  dirs::home_dir().expect("could not determine home directory").join(".config/jc")
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

pub fn load_config() -> Result<AppConfig> {
  let path = config_path();
  if !path.exists() {
    return Ok(AppConfig::default());
  }
  let contents =
    fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
  let config: AppConfig =
    toml::from_str(&contents).with_context(|| format!("failed to parse {}", path.display()))?;
  Ok(config)
}

pub fn save_config(config: &AppConfig) -> Result<()> {
  let dir = ensure_config_dir()?;
  let path = dir.join("config.toml");
  let contents = toml::to_string_pretty(config).context("failed to serialize config")?;
  fs::write(&path, contents).with_context(|| format!("failed to write {}", path.display()))?;
  Ok(())
}

pub fn load_state() -> Result<AppState> {
  let path = state_path();
  if !path.exists() {
    return Ok(AppState::default());
  }
  let contents =
    fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
  let state: AppState =
    toml::from_str(&contents).with_context(|| format!("failed to parse {}", path.display()))?;
  Ok(state)
}

pub fn save_state(state: &AppState) -> Result<()> {
  let dir = ensure_config_dir()?;
  let path = dir.join("state.toml");
  let contents = toml::to_string_pretty(state).context("failed to serialize state")?;
  fs::write(&path, contents).with_context(|| format!("failed to write {}", path.display()))?;
  Ok(())
}
