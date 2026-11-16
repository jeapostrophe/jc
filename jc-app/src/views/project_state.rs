use crate::views::code_view::CodeView;
use crate::views::pane::PaneContentKind;
use crate::views::scratch_view::ScratchView;
use crate::views::session_state::{Heading, SessionId, SessionState, Start};
use crate::views::todo_view::TodoView;
use gpui::*;
use jc_core::cira::{OpenCounts, Status};
use jc_core::claude::Launch;
use jc_core::flavor::Flavor;
use jc_terminal::Palette;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// `$HOME` as a path, or `None` if unset/empty. With no home, Claude's
/// transcript buckets can't be located at all, so callers must treat `None` as
/// "can't tell" rather than "nothing there".
pub(crate) fn home_dir() -> Option<PathBuf> {
  std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// jc: the launch flag for `uuid` given `dirs`, the project's transcript
/// buckets. `None` dirs means `$HOME` is unset, so nothing can be located at
/// all; assume the common case and resume.
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
  /// jcc: the Cira project's scratch pad (Cira PLAN TMUX-11), named at the
  /// Cira read that names the project.
  pub scratch_view: Entity<ScratchView>,
  /// jcc: added to jcc's list in this run, or named by `jcc <dir>`: only
  /// such a project registers a Cira project (`lifecycle::plan_todos`).
  pub added: bool,
  /// What waits on Jay in the Cira project this one belongs to, from the last
  /// `cira status --json`; `None` while Cira is unreachable or has no project
  /// containing this one.
  pub cira_open: Option<OpenCounts>,
  /// Conversation uuids and job short ids of the sessions Cira lists for
  /// this project, so the transcript picker does not offer them again.
  pub cira_known: HashSet<String>,
  /// jc: cached transcript buckets for this project (see [`Self::launch_for`]).
  /// `None` when `$HOME` is unset, and always for jcc, which asks Cira.
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
    let flavor = Flavor::current();
    // jc's TODO is the project's TODO.md; jcc's is its own todo.md in Cira's
    // store, opened once a status read names the Cira project (`open_todo`).
    let todo = flavor.todo_path(&path, None);
    let todo_view = cx.new(|cx| TodoView::new(todo, window, cx));
    let scratch_view = cx.new(|cx| ScratchView::new(window, cx));
    // jc: one bucket scan for the whole project, cached on the struct and
    // reused by every later `launch_for`.
    let session_dirs = match flavor {
      Flavor::Jc => Self::session_dirs(&path),
      Flavor::Jcc => None,
    };
    let mut project = Self {
      path,
      name,
      sessions: HashMap::new(),
      active_session: None,
      next_session_id: 0,
      todo_view,
      scratch_view,
      added: false,
      cira_open: None,
      cira_known: HashSet::new(),
      session_dirs,
    };
    project.restore_sessions(palette, window, cx);
    project
  }

  /// jcc: open the todo.md a status read named for this project
  /// (`Flavor::todo_path`), and start its sessions.
  pub fn open_todo(&mut self, todo: PathBuf, palette: &Palette, window: &mut Window, cx: &mut App) {
    self.todo_view.update(cx, |tv, cx| tv.open(todo, window, cx));
    self.restore_sessions(palette, window, cx);
  }

  /// Whether this app's TODO for the project is open: always for jc; for jcc
  /// once Cira has named the project.
  pub fn todo_open(&self, cx: &App) -> bool {
    self.todo_view.read(cx).is_open(cx)
  }

  /// Start a session for every active heading in the TODO, once it is open.
  fn restore_sessions(&mut self, palette: &Palette, window: &mut Window, cx: &mut App) {
    let todo_view = self.todo_view.clone();
    // Bound each session's message log first, so the session-restore loop
    // below sees the bounded document.
    todo_view.update(cx, |tv, cx| tv.truncate_logs(window, cx));

    // Adopt every active TODO session so the full set of open sessions is
    // restored, not just one. jc runs `claude` on each at once; a session whose
    // transcript Claude has since garbage-collected is revived rather than
    // retired: `launch_for` sees the missing `<uuid>.jsonl` and claims the uuid
    // with `--session-id`. jcc starts each with an empty Claude pane: what it
    // attaches to is the job Cira names (`Workspace::drive_sessions`), and a
    // legacy `> uuid=` heading is first migrated through Cira.
    let flavor = Flavor::current();
    let document = todo_view.read(cx).document().clone();

    // The active session is the one with the most recent `> last=` submit;
    // sessions without one sort as 0, so ties fall back to document order (first).
    let mut best_active: Option<(SessionId, u64)> = None;

    // Two headings can carry the same id if a session block was copy-pasted;
    // adopting both would attach two panes to one session, or migrate one
    // `> uuid=` twice (the second forks a copy), so only the first is adopted.
    // The later duplicates are then *unreachable* rather than dormant — every
    // lookup resolves an id to the first match — and jc does not repair them.
    // Editing the id line by hand is the fix.
    let mut adopted_ids: HashSet<&str> = HashSet::new();

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
      .filter(|s| adopted_ids.insert(s.id.as_str()))
    {
      let id = self.next_session_id;
      self.next_session_id += 1;

      let start = match flavor {
        Flavor::Jc => {
          let launch = launch_from(self.session_dirs.as_deref(), &todo_session.id);
          Start::Own(jc_core::claude::claude_argv(&todo_session.id, launch, todo_session.dangerous))
        }
        Flavor::Jcc => Start::Cira,
      };
      let state =
        SessionState::create(id, Heading::of(todo_session), start, &self.path, palette, window, cx);
      self.sessions.insert(id, state);

      let last = todo_session.last_active.unwrap_or(0);
      if best_active.is_none_or(|(_, best_last)| last > best_last) {
        best_active = Some((id, last));
      }
    }

    if let Some((id, _)) = best_active {
      self.active_session = Some(id);
      // Highlight the initial active session in the TODO view.
      let key = self.sessions[&id].key.clone();
      todo_view.update(cx, |tv, cx| tv.set_active_key(Some(&key), cx));
    }
  }

  /// jc: start `claude` on `uuid` the only way that works: `--resume` needs the
  /// transcript to exist, and `--session-id` needs it not to (see [`Launch`]).
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
  /// what to list as adoptable, so the rule lives here, once: a bound heading
  /// is adopted if a session carries its id; an unbound heading (empty id) is
  /// never adopted, because that empty string names no session. A session
  /// whose start failed does not count, so its heading can be adopted again.
  pub fn is_adopted(&self, key: &str) -> bool {
    !key.is_empty() && self.sessions.values().any(|s| s.key == key && !s.failed)
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

  /// Take this project's counts and its sessions' states from a `cira status
  /// --json` (`None`: Cira is unreachable). Returns whether anything changed.
  pub fn apply_cira(&mut self, status: Option<&Status>) -> bool {
    let project = status.and_then(|s| s.project_for(&self.path));
    let open = project.map(|p| p.open);
    let mut changed = std::mem::replace(&mut self.cira_open, open) != open;
    self.cira_known = project
      .into_iter()
      .flat_map(|p| &p.sessions)
      .flat_map(|s| [s.uuid.clone(), s.short.clone(), s.job.as_ref().map(|j| j.short.clone())])
      .flatten()
      .collect();
    for session in self.sessions.values_mut() {
      // A session jc made has its Cira id, found in whichever Cira project
      // holds it; a legacy one, before its migration, only by short or uuid.
      let listed = match &session.cira {
        Some(id) => status.and_then(|s| s.session(id)),
        None => project.and_then(|p| p.session(&session.bg, &session.uuid)),
      };
      let state = listed.map(|s| (s.state, s.since()));
      changed |= std::mem::replace(&mut session.cira_state, state) != state;
      if session.cira.is_some() {
        if let Some(uuid) = listed.and_then(|s| s.uuid.as_ref()) {
          session.uuid.clone_from(uuid);
        }
        let listed = listed.cloned();
        if session.cira_session != listed {
          session.cira_session = listed;
          changed = true;
        }
      }
    }
    changed
  }

  /// The open blocks of the Cira project holding it; none while unknown.
  pub fn open_blocks(&self) -> u64 {
    self.cira_open.map_or(0, |o| o.blocks)
  }

  /// Find a session by its heading id.
  pub fn session_by_key(&self, key: &str) -> Option<(SessionId, &SessionState)> {
    if key.is_empty() {
      return None;
    }
    self.sessions.iter().find(|(_, s)| s.key == key).map(|(&id, s)| (id, s))
  }
}
