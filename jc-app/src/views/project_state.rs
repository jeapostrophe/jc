use crate::views::code_view::CodeView;
use crate::views::diff_view::DiffView;
use crate::views::session_state::SessionState;
use crate::views::todo_view::TodoView;
use gpui::*;
use jc_core::problem::Problem;
use jc_core::session::discover_latest_session_group;
use jc_core::todo;
use jc_terminal::Palette;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct ProjectState {
  pub path: PathBuf,
  pub name: String,
  pub sessions: Vec<SessionState>,
  pub active_session_index: Option<usize>,
  pub todo_view: Entity<TodoView>,
  pub diff_view: Entity<DiffView>,
  pub code_view: Entity<CodeView>,
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
      .map(|p| match p {
        todo::TodoProblem::InvalidSessionSlug { slug, .. } => slug.clone(),
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

    Self { path, name, sessions, active_session_index, todo_view, diff_view, code_view }
  }

  pub fn active_session(&self) -> Option<&SessionState> {
    self.active_session_index.and_then(|i| self.sessions.get(i))
  }

  pub fn active_slug(&self) -> Option<&str> {
    self.active_session().map(|s| s.slug.as_str())
  }

  pub fn collect_problems(&self, cx: &App) -> Vec<Problem> {
    let mut problems = Vec::new();

    // TodoProblems -> Problems (invalid slugs)
    for tp in self.todo_view.read(cx).problems() {
      match tp {
        todo::TodoProblem::InvalidSessionSlug { slug, .. } => {
          problems.push(Problem { rank: 5, description: format!("Invalid slug: {slug}") });
        }
      }
    }

    // Per-session problems
    for session in &self.sessions {
      problems.extend(session.problems.iter().cloned());
    }

    // Git dirty working directory
    if is_git_dirty(&self.path) {
      problems.push(Problem { rank: 1, description: "Dirty working directory".into() });
    }

    problems
  }
}

fn is_git_dirty(path: &Path) -> bool {
  let Ok(repo) = git2::Repository::open(path) else {
    return false;
  };
  let mut opts = git2::StatusOptions::default();
  opts.include_untracked(true);
  repo
    .statuses(Some(&mut opts))
    .ok()
    .is_some_and(|statuses| statuses.iter().any(|e| !e.status().is_empty()))
}
