use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
  pub path: PathBuf,
}

impl Project {
  pub fn name(&self) -> String {
    self
      .path
      .file_name()
      .map(|n| n.to_string_lossy().into_owned())
      .unwrap_or_else(|| self.path.display().to_string())
  }
}

impl From<PathBuf> for Project {
  fn from(path: PathBuf) -> Self {
    Self { path }
  }
}

impl From<&Path> for Project {
  fn from(path: &Path) -> Self {
    Self { path: path.to_path_buf() }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WindowLayout {
  #[serde(default)]
  pub width: Option<u32>,
  #[serde(default)]
  pub height: Option<u32>,
  #[serde(default)]
  pub x: Option<i32>,
  #[serde(default)]
  pub y: Option<i32>,
}
