use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WindowLayout {
  pub width: Option<u32>,
  pub height: Option<u32>,
  pub x: Option<i32>,
  pub y: Option<i32>,
}
