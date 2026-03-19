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

    // Lazy adoption: only launch a terminal for the first TODO session whose
    // UUID still has a JSONL file on disk (i.e. the session is resumable).
    // Fall back to the first session if none have a live UUID.
    let document = todo_view.read(cx).document().clone();
    let mut sessions = HashMap::new();
    let mut next_session_id: SessionId = 0;

    let session_dir = Self::session_dir(&path);
    let best = document
      .sessions
      .iter()
      .filter(|s| !s.disabled)
      .find(|s| {
        !s.uuid.is_empty()
          && session_dir.join(format!("{}.jsonl", s.uuid)).exists()
      })
      .or_else(|| document.sessions.iter().find(|s| !s.disabled));

    if let Some(todo_session) = best {
      let uuid = if todo_session.uuid.is_empty() {
        None
      } else {
        Some(todo_session.uuid.clone())
      };
      let id = next_session_id;
      next_session_id += 1;
      let state = SessionState::create(
        id,
        uuid,
        todo_session.label.clone(),
        &path,
        palette,
        window,
        cx,
      );
      sessions.insert(id, state);
    }

    let active_session = sessions.keys().next().copied();

    // Highlight the initial active session in the TODO view.
    if let Some(id) = active_session {
      if let Some(session) = sessions.get(&id) {
        todo_view.update(cx, |tv, cx| tv.set_active_label(Some(&session.label), cx));
      }
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

  /// Path to Claude's JSONL session directory for this project.
  fn session_dir(project_path: &Path) -> PathBuf {
    let encoded = project_path.to_string_lossy().replace('/', "-");
    let home = std::env::var("HOME").expect("HOME not set");
    PathBuf::from(home).join(".claude/projects").join(encoded)
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
      let new_uuid = if todo_session.uuid.is_empty() {
        None
      } else {
        Some(todo_session.uuid.as_str())
      };

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
    self
      .sessions
      .iter()
      .find(|(_, s)| s.uuid.as_deref() == Some(uuid))
      .map(|(&id, s)| (id, s))
  }
}
