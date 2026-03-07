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

pub fn theme_path() -> PathBuf {
  config_dir().join("theme.toml")
}

fn load_toml<T: Default + for<'de> serde::de::Deserialize<'de>>(
  path: &std::path::Path,
) -> Result<T> {
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
