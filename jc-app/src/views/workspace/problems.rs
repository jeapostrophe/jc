use crate::views::pane::PaneContentKind;
use crate::views::session_state::SessionId;
use gpui::*;
use jc_core::problem::{ProblemLayer, ProblemTarget};

use super::{NextProblem, Workspace};

// ---------------------------------------------------------------------------
// Cycle state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct ProblemCycleState {
  pub layer: ProblemLayer,
  pub index: usize,
}

struct CrossSessionProblem {
  project_index: usize,
  session_id: SessionId,
  target: ProblemTarget,
}

impl Workspace {
  // -------------------------------------------------------------------------
  // Main entry point: Cmd-;
  // -------------------------------------------------------------------------

  pub(super) fn next_problem(
    &mut self,
    _: &NextProblem,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    // Phase 1: Collect L0 problems across ALL sessions in ALL projects.
    let l0_problems = self.collect_cross_session_l0(cx);

    if !l0_problems.is_empty() {
      // Store home session on FIRST L0 jump only.
      if self.pre_layer0_home.is_none() {
        if let Some(active_sid) = self.projects[self.active_project_index].active_session {
          self.pre_layer0_home = Some((self.active_project_index, active_sid));
        }
      }

      // Advance cycle within L0 list.
      let idx = match &self.problem_cycle {
        Some(state) if state.layer == ProblemLayer::L0 => {
          let next = state.index + 1;
          if next < l0_problems.len() { next } else { 0 }
        }
        _ => 0,
      };

      self.problem_cycle = Some(ProblemCycleState { layer: ProblemLayer::L0, index: idx });

      let problem = &l0_problems[idx];
      let target_pi = problem.project_index;
      let target_sid = problem.session_id;
      let target = problem.target.clone();

      // Switch to the target session if needed (skip acknowledge to preserve the problem).
      let current_sid = self.projects[self.active_project_index].active_session;
      if target_pi != self.active_project_index || Some(target_sid) != current_sid {
        self.switch_to_session_inner(target_pi, Some(target_sid), true, window, cx);
      }

      self.jump_to_problem_target(target, window, cx);
      return;
    }

    // Phase 2: No L0 problems — return home if we were away.
    if let Some((home_pi, home_sid)) = self.pre_layer0_home.take() {
      // Validate home session still exists.
      let home_valid =
        self.projects.get(home_pi).map(|p| p.sessions.contains_key(&home_sid)).unwrap_or(false);
      if home_valid {
        self.switch_to_session(home_pi, Some(home_sid), window, cx);
      }
      self.problem_cycle = None;
      // Fall through to cycle L1+ in (now current) session.
    }

    // Phase 3: Cycle L1/L2/L3 in current session/project.
    let local_problems = self.collect_local_problems(cx);

    if local_problems.is_empty() {
      // No problems at all — go to TODO/WAIT.
      self.problem_cycle = None;
      self.show_in_pane(
        self.resolve_pane_for_kind(PaneContentKind::TodoEditor, cx),
        PaneContentKind::TodoEditor,
        window,
        cx,
      );
      return;
    }

    // Advance through layers.
    let idx = match &self.problem_cycle {
      Some(state) if state.layer >= ProblemLayer::L1 => {
        // Find problems in the same layer as current cycle.
        let same_layer: Vec<usize> = local_problems
          .iter()
          .enumerate()
          .filter(|(_, (l, _, _))| *l == state.layer)
          .map(|(i, _)| i)
          .collect();

        if same_layer.is_empty() {
          // Current layer exhausted, find first problem in a higher layer.
          local_problems.iter().position(|(l, _, _)| *l > state.layer)
        } else {
          // Try to advance within the layer.
          let current_pos = same_layer.iter().position(|&i| {
            let cycle_idx_in_layer = state.index;
            let pos_in_layer = same_layer.iter().position(|&j| j == i).unwrap_or(0);
            pos_in_layer == cycle_idx_in_layer
          });
          match current_pos {
            Some(pos) if pos + 1 < same_layer.len() => Some(same_layer[pos + 1]),
            _ => {
              // End of this layer, move to next layer.
              local_problems.iter().position(|(l, _, _)| *l > state.layer)
            }
          }
        }
      }
      _ => Some(0), // Start at first problem.
    };

    if let Some(idx) = idx {
      let (layer, _, target) = &local_problems[idx];
      let layer = *layer;
      let target = target.clone();

      // Compute index within the layer for cycle tracking.
      let layer_start = local_problems.iter().position(|(l, _, _)| *l == layer).unwrap_or(0);
      let index_in_layer = idx - layer_start;

      self.problem_cycle = Some(ProblemCycleState { layer, index: index_in_layer });
      self.jump_to_problem_target(target, window, cx);
    } else {
      // All layers exhausted — go to TODO, reset cycle.
      self.problem_cycle = None;
      self.show_in_pane(
        self.resolve_pane_for_kind(PaneContentKind::TodoEditor, cx),
        PaneContentKind::TodoEditor,
        window,
        cx,
      );
    }
  }

  // -------------------------------------------------------------------------
  // Problem collection helpers
  // -------------------------------------------------------------------------

  fn collect_cross_session_l0(&self, _cx: &App) -> Vec<CrossSessionProblem> {
    let mut result = Vec::new();
    for (pi, project) in self.projects.iter().enumerate() {
      let mut session_ids: Vec<SessionId> = project.sessions.keys().copied().collect();
      session_ids.sort();
      for sid in session_ids {
        let session = &project.sessions[&sid];
        for problem in &session.problems {
          if problem.layer() == ProblemLayer::L0 {
            result.push(CrossSessionProblem {
              project_index: pi,
              session_id: sid,
              target: problem.target(),
            });
          }
        }
      }
    }
    result
  }

  fn collect_local_problems(&self, _cx: &App) -> Vec<(ProblemLayer, i8, ProblemTarget)> {
    let project = &self.projects[self.active_project_index];
    let mut problems: Vec<(ProblemLayer, i8, ProblemTarget)> = Vec::new();

    // Session problems.
    if let Some(session) = project.active_session() {
      for sp in &session.problems {
        problems.push((sp.layer(), sp.rank(), sp.target()));
      }

      // Synthetic L3: session idle and has been busy before.
      if !session.busy && session.has_ever_been_busy {
        // Only add L3 if no other problems target Claude terminal already.
        let has_claude_problem =
          session.problems.iter().any(|p| matches!(p.target(), ProblemTarget::ClaudeTerminal));
        if !has_claude_problem {
          problems.push((ProblemLayer::L3, 0, ProblemTarget::ClaudeTerminal));
        }
      }
    }

    // Project problems (diffs, scripts → L1).
    for pp in &project.problems {
      problems.push((pp.layer(), pp.rank(), pp.target()));
    }

    // L2 suppression: if session is busy OR any L1 exists, remove L2 items.
    let session_busy = project.active_session().map(|s| s.busy).unwrap_or(false);
    let has_l1 = problems.iter().any(|(l, _, _)| *l == ProblemLayer::L1);
    if session_busy || has_l1 {
      problems.retain(|(l, _, _)| *l != ProblemLayer::L2);
    }

    // Sort by (layer, rank).
    problems.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    problems
  }

  // -------------------------------------------------------------------------
  // Layer-aware problem counts (used by render.rs)
  // -------------------------------------------------------------------------

  /// For each layer, collect session labels that have problems in that layer,
  /// excluding the active session in the active project.
  pub(super) fn layer_problem_sessions(&self, _cx: &App) -> [Vec<String>; 4] {
    let mut result: [Vec<String>; 4] = Default::default();
    let layers = [ProblemLayer::L0, ProblemLayer::L1, ProblemLayer::L2, ProblemLayer::L3];

    for (li, layer) in layers.iter().enumerate() {
      for (pi, project) in self.projects.iter().enumerate() {
        // Check session problems.
        for (&sid, session) in &project.sessions {
          let is_active = pi == self.active_project_index && project.active_session == Some(sid);
          if is_active {
            continue;
          }

          let mut has_layer = false;

          // Check explicit session problems.
          for sp in &session.problems {
            if sp.layer() == *layer {
              has_layer = true;
              break;
            }
          }

          // Check synthetic L3.
          if !has_layer && *layer == ProblemLayer::L3 && !session.busy && session.has_ever_been_busy
          {
            let has_claude_problem =
              session.problems.iter().any(|p| matches!(p.target(), ProblemTarget::ClaudeTerminal));
            if !has_claude_problem {
              has_layer = true;
            }
          }

          if has_layer {
            let label = format!("{} > {}", project.name, session.label);
            result[li].push(label);
          }
        }

        // Check project-level problems (L1: diffs, scripts) — attribute to project name.
        // Skip projects with no attached sessions (nobody is working on them).
        if *layer == ProblemLayer::L1
          && !project.problems.is_empty()
          && !project.sessions.is_empty()
        {
          let label = format!("{} (files)", project.name);
          // Avoid duplicating if a session already added this project.
          if !result[li].iter().any(|l| l.starts_with(&project.name)) {
            result[li].push(label);
          }
        }
      }
    }
    result
  }

  // -------------------------------------------------------------------------
  // Pane resolution
  // -------------------------------------------------------------------------

  fn resolve_pane_for_kind(&self, kind: PaneContentKind, cx: &App) -> usize {
    let visible = self.visible_pane_count();

    // 1-pane: always pane 0.
    if visible == 1 {
      return 0;
    }

    // If a visible pane already shows this content kind, use it.
    if let Some(idx) = (0..visible).find(|&i| self.panes[i].read(cx).content_kind() == Some(kind)) {
      return idx;
    }

    // 2-pane: pane 0 is Claude, pane 1 is everything else.
    if visible == 2 {
      return if kind == PaneContentKind::ClaudeTerminal { 0 } else { 1 };
    }

    // 3-pane: Claude=0, TODO=1, Other=2.
    match kind {
      PaneContentKind::ClaudeTerminal => 0,
      PaneContentKind::TodoEditor => 1,
      _ => 2,
    }
  }

  // -------------------------------------------------------------------------
  // Jump to a specific problem target
  // -------------------------------------------------------------------------

  fn jump_to_problem_target(
    &mut self,
    target: ProblemTarget,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let kind = match &target {
      ProblemTarget::ClaudeTerminal => PaneContentKind::ClaudeTerminal,
      ProblemTarget::GeneralTerminal => PaneContentKind::GeneralTerminal,
      ProblemTarget::TodoEditor => PaneContentKind::TodoEditor,
      ProblemTarget::DiffView { .. } => PaneContentKind::GitDiff,
      ProblemTarget::CodeView { file, .. }
        if file.file_name().and_then(|f| f.to_str()) == Some("TODO.md") =>
      {
        PaneContentKind::TodoEditor
      }
      ProblemTarget::CodeView { .. } => PaneContentKind::CodeViewer,
    };

    let pane_idx = self.resolve_pane_for_kind(kind, cx);
    self.show_in_pane(pane_idx, kind, window, cx);

    // Post-navigation: open specific file or navigate to diff entry.
    // Skip CodeView actions when the target was remapped to TodoEditor.
    match target {
      ProblemTarget::CodeView { file, line } if kind == PaneContentKind::CodeViewer => {
        let project_path = self.projects[self.active_project_index].path.clone();
        let full_path = project_path.join(&file);
        let Some(code_view) = self.projects[self.active_project_index].code_view().cloned() else {
          return;
        };
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
