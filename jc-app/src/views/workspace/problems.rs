use crate::views::pane::PaneContentKind;
use gpui::*;
use jc_core::problem::ProblemTarget;

use super::{NextProblem, Workspace};

impl Workspace {
  pub(super) fn next_problem(
    &mut self,
    _: &NextProblem,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let project = &self.projects[self.active_project_index];

    // Collect all problem targets sorted by rank.
    let mut ranked: Vec<(i8, ProblemTarget)> = Vec::new();
    if let Some(session) = project.active_session() {
      ranked.extend(session.problems.iter().map(|p| (p.rank(), p.target())));
    }
    ranked.extend(project.problems.iter().map(|p| (p.rank(), p.target())));

    if ranked.is_empty() {
      // No problems — go straight to TODO/WAIT.
      self.last_jumped_target = None;
      self.set_active_pane_view(PaneContentKind::TodoEditor, window, cx);
      return;
    }

    ranked.sort_by_key(|(rank, _)| *rank);

    // Find position of last jumped target; advance to next (or start at 0).
    let next_idx = match &self.last_jumped_target {
      Some(prev) => ranked.iter().position(|(_, t)| t == prev).map(|i| i + 1).unwrap_or(0),
      None => 0,
    };

    if next_idx >= ranked.len() {
      // End-of-cycle: jump to TODO/WAIT, reset so next press restarts.
      self.last_jumped_target = None;
      self.set_active_pane_view(PaneContentKind::TodoEditor, window, cx);
    } else {
      let target = ranked.swap_remove(next_idx).1;
      self.jump_to_problem_target(target, window, cx);
    }
  }

  fn jump_to_problem_target(
    &mut self,
    target: ProblemTarget,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.last_jumped_target = Some(target.clone());

    let kind = match &target {
      ProblemTarget::ClaudeTerminal => PaneContentKind::ClaudeTerminal,
      ProblemTarget::GeneralTerminal => PaneContentKind::GeneralTerminal,
      ProblemTarget::TodoEditor => PaneContentKind::TodoEditor,
      ProblemTarget::DiffView { .. } => PaneContentKind::GitDiff,
      ProblemTarget::CodeView { .. } => PaneContentKind::CodeViewer,
    };

    // set_active_pane_view handles view resolution, refresh, focus, and TODO scroll.
    self.set_active_pane_view(kind, window, cx);

    // Post-navigation: open specific file or navigate to diff entry.
    match target {
      ProblemTarget::CodeView { file, line } => {
        let project_path = self.projects[self.active_project_index].path.clone();
        let full_path = project_path.join(&file);
        let code_view = self.projects[self.active_project_index].code_view.clone();
        code_view.update(cx, |v, cx| {
          v.open_file(full_path, window, cx);
          if let Some(line) = line {
            v.scroll_to_line(line as u32, window, cx);
          }
        });
      }
      ProblemTarget::DiffView { file } => {
        let diff_view = self.projects[self.active_project_index].diff_view.clone();
        let file_str = file.to_string_lossy();
        let idx = {
          let dv = diff_view.read(cx);
          dv.file_diffs().iter().position(|fd| fd.name == *file_str)
        };
        if let Some(idx) = idx {
          diff_view.update(cx, |v, cx| v.set_file_index(idx, window, cx));
        }
      }
      _ => {}
    }
  }

}
