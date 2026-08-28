use crate::views::code_view::CodeView;
use crate::views::pane::PaneContentKind;
use crate::views::session_state::{Launch, SessionId, SessionState};
use crate::views::todo_view::TodoView;
use gpui::*;
use jc_terminal::Palette;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `$HOME` as a path, or `None` if unset/empty. With no home, Claude's
/// transcript buckets can't be located at all, so callers must treat `None` as
/// "can't tell" rather than "nothing there".
pub(crate) fn home_dir() -> Option<PathBuf> {
  std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// The launch flag for `uuid` given `dirs`, the project's transcript buckets.
/// `None` dirs means `$HOME` is unset, so nothing can be located at all; assume
/// the common case and resume.
fn launch_from(dirs: Option<&[PathBuf]>, uuid: &str) -> Launch {
  match dirs {
    Some(dirs) if !jc_core::claude::transcript_in(dirs, uuid) => Launch::New,
    _ => Launch::Resume,
  }
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
  /// Cached transcript buckets for this project (see [`Self::launch_for`]).
  /// `None` only when `$HOME` is unset.
  session_dirs: Option<Vec<PathBuf>>,
}

impl ProjectState {
  pub fn create(
    path: PathBuf,
    name: String,
    palette: &Palette,
    window: &mut Window,
    cx: &mut App,
  ) -> Self {
    let todo_view = cx.new(|cx| TodoView::new(path.clone(), window, cx));

    // Bound each session's message log first, so the session-restore loop
    // below sees the bounded document.
    todo_view.update(cx, |tv, cx| tv.truncate_logs(window, cx));

    todo_view.update(cx, |tv, cx| tv.dedupe_labels(window, cx));

    // Adopt every active TODO session so the full set of open sessions is
    // restored, not just one. A session whose transcript Claude has since
    // garbage-collected is revived rather than retired: `launch_for` sees the
    // missing `<uuid>.jsonl` and claims the UUID with `--session-id`.
    let document = todo_view.read(cx).document().clone();
    let mut sessions = HashMap::new();
    let mut next_session_id: SessionId = 0;

    // The active session is the one with the most recent `> last=` submit;
    // sessions without one sort as 0, so ties fall back to document order (first).
    let mut best_active: Option<(SessionId, u64)> = None;

    // One bucket scan for the whole project, cached on the struct and reused by
    // every later `launch_for`.
    let session_dirs = Self::session_dirs(&path);

    // Two headings can carry the same `> uuid=` if a session block was
    // copy-pasted; adopting both would put two `claude --resume <same-uuid>` on
    // one transcript, so only the first is adopted. The later duplicates are
    // then *unreachable* rather than dormant — every lookup resolves a UUID to
    // the first match — and jc does not repair them: the only non-destructive
    // repair would mint a new UUID, orphaning whatever conversation the block
    // was copied from. Editing the `> uuid=` by hand is the fix.
    let mut adopted_uuids: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for todo_session in document
      .sessions
      .iter()
      .filter(|s| s.status == jc_core::todo::SessionStatus::Active)
      // A blank `> uuid=` is a heading an older jc created and never got a hook
      // for. jc cannot know which transcript (if any) belongs to it, so it is
      // NOT launched and NOT given a UUID here — minting one at startup would
      // bind the heading to an empty conversation and orphan the real one,
      // rewriting the user's file before they ever saw it. It stays adoptable
      // in both pickers instead; adopting is where the UUID gets minted.
      .filter(|s| !s.uuid.is_empty())
      .filter(|s| adopted_uuids.insert(s.uuid.as_str()))
    {
      let id = next_session_id;
      next_session_id += 1;

      let state = SessionState::create(
        id,
        todo_session.uuid.clone(),
        todo_session.label.clone(),
        &path,
        palette,
        todo_session.dangerous,
        launch_from(session_dirs.as_deref(), &todo_session.uuid),
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

    Self { path, name, sessions, active_session, next_session_id, todo_view, session_dirs }
  }

  /// Attach to `uuid` the only way that works: `--resume` needs the transcript
  /// to exist, and `--session-id` needs it not to (see [`Launch`]).
  ///
  /// Answers from the bucket list cached at construction, which costs only a
  /// few `stat`s — enumerating buckets means a `read_dir` of the shared
  /// `~/.claude/projects`, and this runs on the main thread from a picker
  /// confirm. A *miss* is the dangerous direction (resuming as new would fail
  /// on an existing conversation) and is also the case a stale cache causes, so
  /// the scan is paid exactly there: a worktree bucket created since startup is
  /// picked up before concluding the transcript is gone.
  ///
  /// With `$HOME` unset no bucket can be located at all; assume the common case
  /// and resume.
  pub fn launch_for(&mut self, uuid: &str) -> Launch {
    if launch_from(self.session_dirs.as_deref(), uuid) == Launch::Resume {
      return Launch::Resume;
    }
    self.session_dirs = Self::session_dirs(&self.path);
    launch_from(self.session_dirs.as_deref(), uuid)
  }

  /// All of Claude's JSONL session buckets for this project (root + worktrees),
  /// or `None` if `$HOME` is unset. The `Some` vec always has at least the root
  /// bucket.
  pub fn session_dirs(project_path: &Path) -> Option<Vec<PathBuf>> {
    home_dir().map(|home| jc_core::claude::session_dirs(&home, project_path))
  }

  /// Is this heading's session already running? Both pickers ask this to decide
  /// what to list as adoptable, so the rule lives here, once: a bound UUID is
  /// adopted if a session carries it; an unbound heading (empty UUID) is never
  /// adopted, because that empty string names no session.
  pub fn is_adopted(&self, uuid: &str) -> bool {
    !uuid.is_empty() && self.sessions.values().any(|s| s.uuid == uuid)
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

  pub fn active_label(&self) -> Option<&str> {
    self.active_session().map(|s| s.label.as_str())
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

    // Match on UUID and nothing else. A label fallback looks tempting but is
    // actively harmful: a heading for a session that ISN'T running matches no
    // UUID, and would then claim a running session sharing its label, stamping
    // the wrong UUID onto it and breaking every hook for it.
    //
    // Driven from the running sessions (a handful) rather than the headings
    // (all of them), so the steady state allocates nothing. `find` takes the
    // first heading with a matching UUID — the same first-wins rule `create`
    // applies, for the same copy-pasted-block case.
    for session in self.sessions.values_mut() {
      if let Some(heading) = document.sessions.iter().find(|s| s.uuid == session.uuid)
        && session.label != heading.label
      {
        session.label = heading.label.clone();
        changed = true;
      }
    }
    changed
  }

  /// Find a session by its label.
  pub fn session_by_label(&self, label: &str) -> Option<(SessionId, &SessionState)> {
    self.sessions.iter().find(|(_, s)| s.label == label).map(|(&id, s)| (id, s))
  }

  /// Find a session by UUID.
  pub fn session_by_uuid(&self, uuid: &str) -> Option<(SessionId, &SessionState)> {
    self.sessions.iter().find(|(_, s)| s.uuid == uuid).map(|(&id, s)| (id, s))
  }
}
