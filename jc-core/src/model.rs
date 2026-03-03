use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskStatus {
  #[default]
  Active,
  Paused,
  Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
  pub id: Uuid,
  pub name: String,
  #[serde(default)]
  pub status: TaskStatus,
  /// The Claude Code session slug binding this task to a session group.
  /// Stable across session forks (plan->execute, /clear+resume).
  #[serde(default)]
  pub session_slug: Option<String>,
  pub created_at: DateTime<Utc>,
}

impl Task {
  pub fn with_name(name: impl Into<String>) -> Self {
    Self {
      id: Uuid::new_v4(),
      name: name.into(),
      status: TaskStatus::default(),
      session_slug: None,
      created_at: Utc::now(),
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
  pub id: Uuid,
  pub name: String,
  pub path: PathBuf,
  #[serde(default)]
  pub tasks: Vec<Task>,
  pub added_at: DateTime<Utc>,
}

impl Project {
  pub fn from_path(path: &Path) -> Self {
    let name = path
      .file_name()
      .map(|n| n.to_string_lossy().into_owned())
      .unwrap_or_else(|| path.display().to_string());
    Self {
      id: Uuid::new_v4(),
      name,
      path: path.to_path_buf(),
      tasks: Vec::default(),
      added_at: Utc::now(),
    }
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
