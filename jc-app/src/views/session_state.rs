use crate::views::reply_view::ReplyView;
use gpui::*;
use jc_core::problem::{AppTodoProblem, ClaudeProblem, SessionProblem, TerminalProblem};
use jc_core::session::discover_session_group;
use jc_core::todo::TodoProblem;
use jc_terminal::{Palette, TerminalConfig, TerminalView};
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PendingEvent {
  ClaudeStop,
  ClaudePermission,
  ClaudeIdle,
  TerminalBell,
}

pub struct SessionState {
  pub slug: String,
  pub label: String,
  pub claude_terminal: Entity<TerminalView>,
  pub general_terminal: Entity<TerminalView>,
  pub reply_view: Entity<ReplyView>,
  pub pending_events: HashSet<PendingEvent>,
  pub problems: Vec<SessionProblem>,
}

impl SessionState {
  pub fn create(
    slug: String,
    label: String,
    project_path: &Path,
    palette: &Palette,
    window: &mut Window,
    cx: &mut App,
  ) -> Self {
    // Find the most recent JSONL session UUID for this slug so we can
    // resume the Claude session. Falls back to plain `claude` if none found.
    let command = discover_session_group(project_path, &slug)
      .and_then(|g| g.latest_session_id())
      .map(|uuid| format!("claude --resume {uuid}"))
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

    let slug_for_reply = slug.clone();
    let reply_project = project_path.to_path_buf();
    let reply_view = cx.new(|cx| {
      let mut rv = ReplyView::new(reply_project, window, cx);
      rv.set_session_slug(Some(slug_for_reply), window, cx);
      rv
    });

    Self {
      slug,
      label,
      claude_terminal,
      general_terminal,
      reply_view,
      pending_events: HashSet::default(),
      problems: Vec::new(),
    }
  }

  /// Rebuild `self.problems` from pending events and todo problems.
  /// Returns `true` if the problem list changed.
  pub fn refresh_problems(&mut self, todo_problems: &[TodoProblem]) -> bool {
    let mut problems = Vec::<SessionProblem>::new();

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
        TodoProblem::InvalidSessionSlug { slug, line, .. } if slug == &self.slug => {
          problems.push(SessionProblem::Todo(AppTodoProblem::InvalidSlug {
            slug: slug.clone(),
            line: *line,
          }));
        }
        TodoProblem::UnsentWait { slug } if slug == &self.slug => {
          problems.push(SessionProblem::Todo(AppTodoProblem::UnsentWait { slug: slug.clone() }));
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
