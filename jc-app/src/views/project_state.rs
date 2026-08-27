use crate::views::code_view::CodeView;
use crate::views::diff_view::DiffView;
use crate::views::pane::PaneContentKind;
use crate::views::session_state::{SessionId, SessionState};
use crate::views::todo_view::TodoView;
use gpui::*;
use jc_core::problem::{DiffProblem, ProjectProblem, ScriptProblem};
use jc_core::status_script;
use jc_terminal::Palette;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// `$HOME` as a path, or `None` if unset/empty. Callers deciding session
/// liveness MUST treat `None` as "can't tell" and take no destructive action
/// (never mark sessions expired): an empty home resolves every transcript bucket
/// to a nonexistent relative path, which would falsely expire every live session.
pub(crate) fn home_dir() -> Option<PathBuf> {
  std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

pub struct SavedPaneLayout {
  pub pane_kinds: [Option<PaneContentKind>; 3],
  pub active_pane_index: usize,
}

pub struct ProjectState {
  pub path: PathBuf,
  pub name: String,
  pub sessions: HashMap<SessionId, SessionState>,
  pub active_session: Option<SessionId>,
  pub next_session_id: SessionId,
  pub todo_view: Entity<TodoView>,
  pub diff_view: Entity<DiffView>,
  pub problems: Vec<ProjectProblem>,
  pub script_problems: Vec<ScriptProblem>,
  pub last_script_run: Option<Instant>,
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
    let todo_view = cx.new(|cx| TodoView::new(path.clone(), window, cx));

    // Bound each session's message log first, so the expiry pass and the
    // session-restore loop below both see the bounded document.
    todo_view.update(cx, |tv, cx| tv.truncate_logs(window, cx));

    // Mark sessions whose JSONL files have been garbage-collected by Claude. A
    // session's transcript may live in the project's root bucket or in any of its
    // git-worktree buckets, so check all of them before declaring it gone. With
    // no $HOME we can't locate any bucket, so expire nothing rather than wrongly
    // persisting `[X]` for live sessions.
    {
      let document = todo_view.read(cx).document().clone();
      let expired_labels: Vec<String> = match Self::session_dirs(&path) {
        None => Vec::new(),
        Some(session_dirs) => document
          .sessions
          .iter()
          .filter(|s| {
            !s.uuid.is_empty()
              && s.status != jc_core::todo::SessionStatus::Expired
              && !jc_core::claude::transcript_in(&session_dirs, &s.uuid)
          })
          .map(|s| s.label.clone())
          .collect(),
      };
      if !expired_labels.is_empty() {
        todo_view.update(cx, |tv, cx| {
          for label in &expired_labels {
            tv.mark_session_expired(label, window, cx);
          }
          tv.save(cx);
        });
      }
    }

    // Adopt every active TODO session so the full set of open sessions is
    // restored, not just one. Sessions whose JSONL was GC'd are already marked
    // `[X]` (Expired) above and filtered out here; empty-UUID (pending) sessions
    // relaunch a plain `claude` and re-acquire a UUID on their first hook.
    let document = todo_view.read(cx).document().clone();
    let mut sessions = HashMap::new();
    let mut next_session_id: SessionId = 0;

    // The active session is the one with the most recent `> last=` submit;
    // sessions without one sort as 0, so ties fall back to document order (first).
    let mut best_active: Option<(SessionId, u64)> = None;

    for todo_session in
      document.sessions.iter().filter(|s| s.status == jc_core::todo::SessionStatus::Active)
    {
      let uuid = if todo_session.uuid.is_empty() { None } else { Some(todo_session.uuid.clone()) };
      let id = next_session_id;
      next_session_id += 1;
      let state = SessionState::create(
        id,
        uuid,
        todo_session.label.clone(),
        &path,
        palette,
        todo_session.dangerous,
        window,
        cx,
      );
      sessions.insert(id, state);

      let last = todo_session.last_active.unwrap_or(0);
      if best_active.is_none_or(|(_, best_last)| last > best_last) {
        best_active = Some((id, last));
      }
    }

    let active_session = best_active.map(|(id, _)| id);

    // Highlight the initial active session in the TODO view.
    if let Some(id) = active_session
      && let Some(session) = sessions.get(&id)
    {
      todo_view.update(cx, |tv, cx| tv.set_active_label(Some(&session.label), cx));
    }

    Self {
      path,
      name,
      sessions,
      active_session,
      next_session_id,
      todo_view,
      diff_view,
      problems: Vec::new(),
      script_problems: Vec::new(),
      last_script_run: None,
    }
  }

  /// All of Claude's JSONL session buckets for this project (root + worktrees),
  /// or `None` if `$HOME` is unset — buckets then can't be located and callers
  /// must take no destructive action (never expire/prune sessions). The `Some`
  /// vec always has at least the root bucket.
  pub fn session_dirs(project_path: &Path) -> Option<Vec<PathBuf>> {
    home_dir().map(|home| jc_core::claude::session_dirs(&home, project_path))
  }

  pub fn active_session(&self) -> Option<&SessionState> {
    self.active_session.and_then(|id| self.sessions.get(&id))
  }

  pub fn active_session_mut(&mut self) -> Option<&mut SessionState> {
    self.active_session.and_then(|id| self.sessions.get_mut(&id))
  }

  pub fn active_label(&self) -> Option<&str> {
    self.active_session().map(|s| s.label.as_str())
  }

  /// Convenience: the active session's code view.
  pub fn code_view(&self) -> Option<&Entity<CodeView>> {
    self.active_session().map(|s| &s.code_view)
  }

  /// Refresh problems for all sessions and the project itself.
  /// Returns `true` if any problem list changed.
  pub fn refresh_problems(&mut self, cx: &App) -> bool {
    let todo_view = self.todo_view.read(cx);
    let todo_problems = todo_view.problems();

    let mut changed = false;

    // Sync session state from the TODO document.
    // Match by UUID (stable) first, then fall back to label for sessions without a UUID.
    let document = todo_view.document();
    for todo_session in &document.sessions {
      let new_uuid =
        if todo_session.uuid.is_empty() { None } else { Some(todo_session.uuid.as_str()) };

      let matched = self.sessions.values_mut().find(|session| {
        // Primary match: both have UUIDs and they match.
        if let (Some(s_uuid), Some(t_uuid)) = (session.uuid.as_deref(), new_uuid) {
          return s_uuid == t_uuid;
        }
        // Fallback: session has no UUID yet, match by label.
        session.uuid.is_none() && session.label == todo_session.label
      });

      if let Some(session) = matched {
        // Update UUID if it was assigned or changed.
        let owned_uuid = new_uuid.map(str::to_string);
        if session.uuid != owned_uuid {
          session.uuid = owned_uuid;
          changed = true;
        }
        // Keep label in sync with the TODO heading.
        if session.label != todo_session.label {
          session.label = todo_session.label.clone();
          changed = true;
        }
      }
    }

    for session in self.sessions.values_mut() {
      changed |= session.refresh_problems(todo_problems);
    }

    // Run status.sh at most once every 10 seconds.
    let script_interval = std::time::Duration::from_secs(10);
    let should_run_script = self.last_script_run.is_none_or(|t| t.elapsed() >= script_interval);
    if should_run_script {
      self.script_problems = status_script::run_status_script(&self.path);
      self.last_script_run = Some(Instant::now());
    }

    // Project-level problems: unreviewed diff files + script problems.
    let mut problems: Vec<ProjectProblem> = self
      .diff_view
      .read(cx)
      .unreviewed_files()
      .into_iter()
      .map(|path| ProjectProblem::Diff(DiffProblem::UnreviewedFile(path)))
      .chain(self.script_problems.iter().map(|sp| ProjectProblem::Script(sp.clone())))
      .collect();
    problems.sort_by_key(|p| p.rank());
    changed |= self.problems != problems;
    self.problems = problems;
    changed
  }

  /// Find a session by its label.
  pub fn session_by_label(&self, label: &str) -> Option<(SessionId, &SessionState)> {
    self.sessions.iter().find(|(_, s)| s.label == label).map(|(&id, s)| (id, s))
  }

  /// Find a session by UUID.
  pub fn session_by_uuid(&self, uuid: &str) -> Option<(SessionId, &SessionState)> {
    self.sessions.iter().find(|(_, s)| s.uuid.as_deref() == Some(uuid)).map(|(&id, s)| (id, s))
  }
}
