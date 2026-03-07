use crate::views::code_view::CodeView;
use crate::views::diff_view::DiffView;
use crate::views::session_state::SessionState;
use crate::views::todo_view::TodoView;
use gpui::*;
use jc_core::problem::{DiffProblem, ProjectProblem, ScriptProblem};
use jc_core::session::discover_latest_session_group;
use jc_core::status_script;
use jc_core::todo;
use jc_terminal::Palette;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use super::pane::PaneContentKind;
use super::workspace::PaneLayout;

pub struct SavedPaneLayout {
  pub pane_kinds: [Option<PaneContentKind>; 3],
  pub active_pane_index: usize,
  pub layout: PaneLayout,
}

pub struct ProjectState {
  pub path: PathBuf,
  pub name: String,
  pub sessions: Vec<SessionState>,
  pub active_session_index: Option<usize>,
  pub todo_view: Entity<TodoView>,
  pub diff_view: Entity<DiffView>,
  pub code_view: Entity<CodeView>,
  pub problems: Vec<ProjectProblem>,
  pub script_problems: Vec<ScriptProblem>,
  pub last_script_run: Option<Instant>,
  pub saved_layout: Option<SavedPaneLayout>,
}

impl ProjectState {
  pub fn create(
    path: PathBuf,
    name: String,
    palette: &Palette,
    window: &mut Window,
    cx: &mut App,
  ) -> Self {
    let diff_view = cx.new(|cx| DiffView::new(path.clone(), window, cx));
    let code_view = cx.new(|cx| CodeView::new(window, cx));
    let todo_view = cx.new(|cx| TodoView::new(path.clone(), window, cx));

    // If TODO.md has no valid sessions, try to discover the most recent
    // JSONL session group and insert a heading automatically.
    if !todo::has_valid_sessions(todo_view.read(cx).document(), &path)
      && let Some(group) = discover_latest_session_group(&path)
    {
      todo_view.update(cx, |tv, cx| {
        tv.insert_session_heading(&group.slug, &group.slug, window, cx);
      });
    }

    // Build sessions, skipping any with invalid slugs to avoid creating
    // broken SessionState entries (terminals that can't resume).
    let document = todo_view.read(cx).document().clone();
    let invalid_slugs: HashSet<String> = todo_view
      .read(cx)
      .problems()
      .iter()
      .filter_map(|p| match p {
        todo::TodoProblem::InvalidSessionSlug { slug, .. } => Some(slug.clone()),
        todo::TodoProblem::UnsentWait { .. } => None,
      })
      .collect();

    let mut sessions = Vec::new();
    for todo_session in &document.sessions {
      if invalid_slugs.contains(&todo_session.slug) {
        continue;
      }
      sessions.push(SessionState::create(
        todo_session.slug.clone(),
        todo_session.label.clone(),
        &path,
        palette,
        window,
        cx,
      ));
    }

    let active_session_index = if sessions.is_empty() { None } else { Some(0) };

    // Highlight the initial active session in the TODO view.
    if let Some(slug) = active_session_index.and_then(|i| sessions.get(i)).map(|s| s.slug.clone()) {
      todo_view.update(cx, |tv, cx| tv.set_active_slug(Some(&slug), cx));
    }

    Self {
      path,
      name,
      sessions,
      active_session_index,
      todo_view,
      diff_view,
      code_view,
      problems: Vec::new(),
      script_problems: Vec::new(),
      last_script_run: None,
      saved_layout: None,
    }
  }

  pub fn active_session(&self) -> Option<&SessionState> {
    self.active_session_index.and_then(|i| self.sessions.get(i))
  }

  pub fn active_slug(&self) -> Option<&str> {
    self.active_session().map(|s| s.slug.as_str())
  }

  /// Refresh problems for all sessions and the project itself.
  /// Returns `true` if any problem list changed.
  pub fn refresh_problems(&mut self, cx: &App) -> bool {
    let todo_view = self.todo_view.read(cx);
    let todo_problems = todo_view.problems();

    let mut changed = false;
    for session in &mut self.sessions {
      changed |= session.refresh_problems(todo_problems);
    }

    // Run status.sh at most once every 10 seconds.
    let script_interval = std::time::Duration::from_secs(10);
    let should_run_script = self.last_script_run.map_or(true, |t| t.elapsed() >= script_interval);
    if should_run_script {
      self.script_problems = status_script::run_status_script(&self.path);
      self.last_script_run = Some(Instant::now());
    }

    // Project-level problems: unreviewed diff files + script problems.
    let mut problems = Vec::<ProjectProblem>::new();
    for path in self.diff_view.read(cx).unreviewed_files() {
      problems.push(ProjectProblem::Diff(DiffProblem::UnreviewedFile(path)));
    }
    for sp in &self.script_problems {
      problems.push(ProjectProblem::Script(sp.clone()));
    }
    problems.sort_by_key(|p| p.rank());
    if self.problems != problems {
      changed = true;
    }
    self.problems = problems;
    changed
  }

  /// Total problem count across all sessions and the project.
  pub fn total_problem_count(&self) -> usize {
    let session_count: usize = self.sessions.iter().map(|s| s.problems.len()).sum();
    session_count + self.problems.len()
  }
}
