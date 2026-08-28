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

    Self::dedupe_labels(&todo_view, window, cx);

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
    // copy-pasted. Adopting both would run two `claude --resume <same-uuid>`
    // against one transcript and make hook routing depend on HashMap iteration
    // order, so only the first is adopted.
    //
    // The later duplicates are then *unreachable*, not merely dormant: both
    // pickers hide a heading whose UUID is already adopted, and `SessionKey`
    // resolves a UUID to the first match, so they cannot be adopted, disabled,
    // or renamed from the UI. `dedupe_labels` does not repair this the way it
    // repairs duplicate labels — the only non-destructive repair would be
    // minting a new UUID, which orphans whatever conversation the block was
    // copied from. Editing the `> uuid=` by hand is the fix.
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
        match session_dirs.as_deref() {
          Some(dirs) if !jc_core::claude::transcript_in(dirs, &todo_session.uuid) => Launch::New,
          _ => Launch::Resume,
        },
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

  /// Rename any heading whose label duplicates an earlier one, so the label is
  /// a real address again. Startup only, deliberately: renaming a heading the
  /// user is in the middle of typing would be worse than the ambiguity, and
  /// every path that jc itself creates a label through already uses
  /// `todo::unique_label`.
  ///
  /// jc keeps labels unique at every creation point, but a file written by an
  /// older jc typically has several `## New Session` headings — and TODO.md
  /// addresses a session by label almost everywhere (`ensure_wait`,
  /// `send_selection`, the `@jc(...)` scheduled-send timers), each resolving to
  /// the FIRST match. Left alone, a Cmd-Enter or a scheduled delivery aimed at
  /// the second session silently lands on the first, or is dropped.
  fn dedupe_labels(todo_view: &Entity<TodoView>, window: &mut Window, cx: &mut App) {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let duplicates: Vec<usize> = todo_view
      .read(cx)
      .document()
      .sessions
      .iter()
      .enumerate()
      .filter(|(_, s)| !seen.insert(s.label.clone()))
      .map(|(i, _)| i)
      .collect();
    if duplicates.is_empty() {
      return;
    }
    todo_view.update(cx, |tv, cx| {
      for index in duplicates {
        let Some(base) = tv.document().sessions.get(index).map(|s| s.label.clone()) else {
          continue;
        };
        let fresh = jc_core::todo::unique_label(tv.document(), &base);
        tv.rename_session_at(index, &fresh, window, cx);
      }
      tv.save(cx);
    });
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
    let Some(dirs) = self.session_dirs.as_deref() else { return Launch::Resume };
    if jc_core::claude::transcript_in(dirs, uuid) {
      return Launch::Resume;
    }
    self.session_dirs = Self::session_dirs(&self.path);
    match self.session_dirs.as_deref() {
      Some(dirs) if !jc_core::claude::transcript_in(dirs, uuid) => Launch::New,
      _ => Launch::Resume,
    }
  }

  /// All of Claude's JSONL session buckets for this project (root + worktrees),
  /// or `None` if `$HOME` is unset. The `Some` vec always has at least the root
  /// bucket.
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

  /// Re-sync each running session's label from the TODO document, so a heading
  /// renamed in TODO.md is picked up. Returns `true` if anything changed.
  pub fn sync_sessions_from_todo(&mut self, cx: &App) -> bool {
    // Only the (uuid, label) pairs are needed, so lift those out rather than
    // cloning the whole document every tick.
    let entries: Vec<(String, String)> = self
      .todo_view
      .read(cx)
      .document()
      .sessions
      .iter()
      .filter(|s| !s.uuid.is_empty())
      .map(|s| (s.uuid.clone(), s.label.clone()))
      .collect();
    let mut changed = false;

    // Match on UUID and nothing else. The UUID is now the session's identity —
    // assigned at launch, rewritten only by `/clear` — so it is the one key that
    // survives a rename, which is the whole reason this sync exists. A label
    // fallback looks tempting but is actively harmful: a TODO entry for a
    // session that ISN'T running (disabled, never adopted) matches no UUID, and
    // would then claim a running session that happens to share its label,
    // stamping the wrong UUID onto it and silently breaking every hook for that
    // session. Labels are not unique, so there is no safe version of that.
    // First heading per UUID wins, the same rule `create` applies when adopting.
    // Two headings can share a `> uuid=` (a copy-pasted block), and without this
    // the running session would take the LAST duplicate's label — which is a
    // dormant heading, so every label-keyed TODO operation (`ensure_wait`,
    // `send_selection`, `wait_line`) would then file work under the wrong block.
    // It would also make `changed` true on every tick, forever.
    let mut claimed: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (uuid, label) in entries {
      if !claimed.insert(uuid.clone()) {
        continue;
      }
      if let Some(session) = self.sessions.values_mut().find(|s| s.uuid == uuid)
        && session.label != label
      {
        session.label = label;
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
