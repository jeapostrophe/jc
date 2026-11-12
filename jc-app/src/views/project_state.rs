use crate::views::code_view::CodeView;
use crate::views::pane::PaneContentKind;
use crate::views::session_state::{SessionId, SessionState};
use crate::views::todo_view::TodoView;
use gpui::*;
use jc_core::background::Claude;
use jc_terminal::Palette;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `$HOME` as a path, or `None` if unset/empty. With no home, Claude's
/// transcript buckets can't be located at all, so callers must treat `None` as
/// "can't tell" rather than "nothing there".
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
}

impl ProjectState {
  pub fn create(
    path: PathBuf,
    name: String,
    palette: &Palette,
    claude: &Claude,
    window: &mut Window,
    cx: &mut App,
  ) -> Self {
    let todo_view = cx.new(|cx| TodoView::new(path.clone(), window, cx));

    // Bound each session's message log first, so the session-restore loop
    // below sees the bounded document.
    todo_view.update(cx, |tv, cx| tv.truncate_logs(window, cx));

    // Adopt every active TODO session so the full set of open sessions is
    // restored, not just one. Each starts with an empty Claude pane: what it
    // attaches to (its running job, a migrated `> uuid=`, or a fresh session
    // when its job is gone) needs `claude agents --json`, which
    // `Workspace::launch_sessions` asks off the main thread.
    let document = todo_view.read(cx).document().clone();
    let mut sessions = HashMap::new();
    let mut next_session_id: SessionId = 0;

    // The active session is the one with the most recent `> last=` submit;
    // sessions without one sort as 0, so ties fall back to document order (first).
    let mut best_active: Option<(SessionId, u64)> = None;

    // Two headings can carry the same id if a session block was copy-pasted;
    // adopting both would attach two panes to one session, or migrate one
    // `> uuid=` twice (the second forks a copy), so only the first is adopted.
    // The later duplicates are then *unreachable* rather than dormant — every
    // lookup resolves an id to the first match — and jc does not repair them.
    // Editing the id line by hand is the fix.
    let mut adopted_uuids: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for todo_session in document
      .sessions
      .iter()
      .filter(|s| s.status == jc_core::todo::SessionStatus::Active)
      // A blank `> uuid=` is a heading an older jc created and never got a hook
      // for. jc cannot know which transcript (if any) belongs to it, so it is
      // NOT launched here — starting a session for it at startup would bind
      // the heading to an empty conversation and orphan the real one,
      // rewriting the user's file before they ever saw it. It stays adoptable
      // in both pickers instead; adopting is where it gets a session.
      .filter(|s| !s.id.is_empty())
      .filter(|s| adopted_uuids.insert(s.id.as_str()))
    {
      let id = next_session_id;
      next_session_id += 1;

      let state = SessionState::create(
        id,
        todo_session.id.clone(),
        String::new(),
        todo_session.label.clone(),
        todo_session.cira.clone(),
        todo_session.dangerous,
        claude,
        &path,
        palette,
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
      todo_view.update(cx, |tv, cx| tv.set_active_key(Some(&session.key), cx));
    }

    Self { path, name, sessions, active_session, next_session_id, todo_view }
  }

  /// All of Claude's JSONL session buckets for this project (root + worktrees),
  /// or `None` if `$HOME` is unset. The `Some` vec always has at least the root
  /// bucket.
  pub fn session_dirs(project_path: &Path) -> Option<Vec<PathBuf>> {
    home_dir().map(|home| jc_core::claude::session_dirs(&home, project_path))
  }

  /// Is this heading's session already running? Both pickers ask this to decide
  /// what to list as adoptable, so the rule lives here, once: a bound heading
  /// is adopted if a session carries its id; an unbound heading (empty id) is
  /// never adopted, because that empty string names no session.
  pub fn is_adopted(&self, key: &str) -> bool {
    !key.is_empty() && self.sessions.values().any(|s| s.key == key)
  }

  /// The earliest-created running session, or `None` if the project has none.
  /// Lowest id rather than `HashMap` order, so the same action in the same state
  /// always lands in the same place.
  pub fn first_session(&self) -> Option<SessionId> {
    self.sessions.keys().copied().min()
  }

  pub fn active_session(&self) -> Option<&SessionState> {
    self.active_session.and_then(|id| self.sessions.get(&id))
  }

  pub fn active_session_mut(&mut self) -> Option<&mut SessionState> {
    self.active_session.and_then(|id| self.sessions.get_mut(&id))
  }

  /// The active session's heading id — the address every TODO.md text
  /// operation keys on (see `jc_core::todo::SessionKey`). `None` while a new
  /// session is still starting and has no heading.
  pub fn active_key(&self) -> Option<&str> {
    self.active_session().map(|s| s.key.as_str()).filter(|k| !k.is_empty())
  }

  /// Convenience: the active session's code view.
  pub fn code_view(&self) -> Option<&Entity<CodeView>> {
    self.active_session().map(|s| &s.code_view)
  }

  /// Re-sync each running session's label from the TODO document, so a heading
  /// renamed in TODO.md is picked up. Returns `true` if anything changed.
  pub fn sync_sessions_from_todo(&mut self, cx: &App) -> bool {
    let todo_view = self.todo_view.clone();
    let document = todo_view.read(cx).document();
    let mut changed = false;

    // Match on the heading id and nothing else. A label fallback looks tempting
    // but is actively harmful: a heading for a session that ISN'T running
    // matches no id, and would then claim a running session sharing its label.
    //
    // Driven from the running sessions (a handful) rather than the headings
    // (all of them), so the steady state allocates nothing. `find` takes the
    // first heading with a matching id — the same first-wins rule `create`
    // applies, for the same copy-pasted-block case.
    for session in self.sessions.values_mut().filter(|s| !s.key.is_empty()) {
      if let Some(heading) = document.sessions.iter().find(|s| s.id == session.key)
        && session.label != heading.label
      {
        session.label = heading.label.clone();
        changed = true;
      }
    }
    changed
  }

  /// Find a session by its heading id.
  pub fn session_by_key(&self, key: &str) -> Option<(SessionId, &SessionState)> {
    if key.is_empty() {
      return None;
    }
    self.sessions.iter().find(|(_, s)| s.key == key).map(|(&id, s)| (id, s))
  }
}
