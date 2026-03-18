use gpui::*;
use jc_core::problem::{AppTodoProblem, ClaudeProblem, SessionProblem, TerminalProblem};
use jc_core::todo::TodoProblem;
use jc_terminal::{Palette, TerminalConfig, TerminalView};
use std::collections::HashSet;
use std::path::Path;

pub type SessionId = usize;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PendingEvent {
  ClaudeStop,
  ClaudePermission,
  ClaudeIdle,
  TerminalBell,
}

pub struct SessionState {
  pub id: SessionId,
  pub uuid: Option<String>,
  pub label: String,
  pub claude_terminal: Entity<TerminalView>,
  pub general_terminal: Entity<TerminalView>,
  pub pending_events: HashSet<PendingEvent>,
  pub problems: Vec<SessionProblem>,
}

impl SessionState {
  pub fn create(
    id: SessionId,
    uuid: Option<String>,
    label: String,
    project_path: &Path,
    palette: &Palette,
    window: &mut Window,
    cx: &mut App,
  ) -> Self {
    // If we have a UUID, resume that session. Otherwise launch plain `claude`.
    let command = uuid
      .as_ref()
      .filter(|u| !u.is_empty())
      .map(|u| format!("claude --resume {u}"))
      .unwrap_or_else(|| "claude".to_string());

    let claude_config = TerminalConfig {
      command: Some(command),
      palette: Some(palette.clone()),
      ..Default::default()
    };
    let general_config = TerminalConfig { palette: Some(palette.clone()), ..Default::default() };

    let project = project_path.to_path_buf();
    let claude_terminal = cx.new(|cx| TerminalView::new(claude_config, Some(&project), window, cx));
    let general_terminal =
      cx.new(|cx| TerminalView::new(general_config, Some(&project), window, cx));

    Self {
      id,
      uuid,
      label,
      claude_terminal,
      general_terminal,
      pending_events: HashSet::default(),
      problems: Vec::new(),
    }
  }

  /// Rebuild `self.problems` from pending events and todo problems.
  /// Returns `true` if the problem list changed.
  pub fn refresh_problems(&mut self, todo_problems: &[TodoProblem]) -> bool {
    let mut problems = Vec::new();

    for event in &self.pending_events {
      let sp = match event {
        PendingEvent::ClaudeStop => SessionProblem::Claude(ClaudeProblem::Stop),
        PendingEvent::ClaudePermission => SessionProblem::Claude(ClaudeProblem::Permission),
        PendingEvent::ClaudeIdle => SessionProblem::Claude(ClaudeProblem::Idle),
        PendingEvent::TerminalBell => SessionProblem::Terminal(TerminalProblem::Bell),
      };
      problems.push(sp);
    }

    for tp in todo_problems {
      match tp {
        TodoProblem::UnsentWait { label } if label == &self.label => {
          problems.push(SessionProblem::Todo(AppTodoProblem::UnsentWait { label: label.clone() }));
        }
        _ => {}
      }
    }

    problems.sort_by_key(|p| p.rank());
    let changed = self.problems != problems;
    self.problems = problems;
    changed
  }

  /// Clear all pending events (called when the user interacts with the session).
  pub fn acknowledge(&mut self) {
    self.pending_events.clear();
  }
}
