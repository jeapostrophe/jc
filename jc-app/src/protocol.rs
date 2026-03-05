use serde::{Deserialize, Serialize};

/// Messages sent from server to client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
  AuthChallenge { token: String },
  AuthResult { success: bool },
  StateSnapshot(MobileStateSnapshot),
}

/// Messages sent from client to server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
  Auth { token: String },
}

/// Full state snapshot pushed to connected mobile clients.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MobileStateSnapshot {
  pub projects: Vec<MobileProject>,
  pub active_project_index: usize,
  pub usage: Option<MobileUsage>,
}

/// Lightweight project representation for the mobile client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileProject {
  pub name: String,
  pub sessions: Vec<MobileSession>,
  pub active_session_index: Option<usize>,
  pub problems: Vec<MobileProblem>,
}

/// Lightweight session representation for the mobile client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileSession {
  pub slug: String,
  pub label: String,
  pub problems: Vec<MobileProblem>,
}

/// A single problem visible on mobile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileProblem {
  pub rank: i8,
  pub description: String,
}

/// Usage stats visible on mobile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileUsage {
  pub par: f64,
  pub par_status: String,
  pub limit_pct: f64,
  pub working_pct: f64,
  pub five_hour_pct: f64,
  pub pace: Option<f64>,
  pub remaining_hours: Option<f64>,
}
