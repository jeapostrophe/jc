mod pickers;
mod render;

use crate::views::close_confirm::{CloseConfirm, CloseConfirmEvent};
use crate::views::keybinding_help::{DismissHelpEvent, KeybindingHelp};
use crate::views::pane::{Pane, PaneContent, PaneContentKind};
use crate::views::project_state::{ProjectState, SavedPaneLayout};
use crate::views::session_state::{Heading, SavedViewState, SessionId, SessionState};
use crate::views::todo_view::ScheduledFire;
use chrono::NaiveDateTime;
use gpui::*;
use gpui_component::input::InputState;
use gpui_component::theme::Theme;
use jc_core::background::{self, Claude, LaunchPlan, Permissions, Poll, Settled, StartSpec};
use jc_core::config::{AppConfig, AppState};
use jc_core::hooks::{HookEvent, HookEventKind, HookServer};
use jc_core::theme::Appearance;
use jc_core::todo::{IdKind, SessionKey, TodoDocument, TodoSession};
use jc_terminal::{ChildEvent, Palette, TerminalView};
use std::collections::{HashSet, VecDeque};
use std::ops::DerefMut;
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;
use std::time::{Instant, SystemTime};

actions!(
  workspace,
  [
    CloseWindow,
    MinimizeWindow,
    Quit,
    FocusPrevPane,
    FocusNextPane,
    SetLayoutOne,
    SetLayoutTwo,
    SetLayoutThree,
    ShowClaudeTerminal,
    ShowGeneralTerminal,
    ShowCodeViewer,
    ShowTodoEditor,
    SaveFile,
    SendToTerminal,
    JumpToWait,
    RotateNextProject,
    ShowKeybindingHelp,
    ScrollOtherUp,
    ScrollOtherDown,
    ScrollOtherPageUp,
    ScrollOtherPageDown,
  ]
);

enum OtherPaneScrollable {
  Editor(Entity<InputState>),
  Terminal(Entity<TerminalView>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneLayout {
  One,
  Two,
  #[default]
  Three,
}

/// Map a GPUI WindowAppearance to our Appearance enum.
fn appearance_from_window(appearance: WindowAppearance) -> Appearance {
  match appearance {
    WindowAppearance::Dark | WindowAppearance::VibrantDark => Appearance::Dark,
    WindowAppearance::Light | WindowAppearance::VibrantLight => Appearance::Light,
  }
}

/// Get the terminal palette matching the current window appearance.
fn palette_from_window(window: &Window) -> Palette {
  Palette::for_appearance(appearance_from_window(window.appearance()))
}

pub struct Workspace {
  panes: Vec<Entity<Pane>>,
  active_pane_index: usize,
  layout: PaneLayout,
  projects: Vec<ProjectState>,
  active_project_index: usize,
  #[allow(dead_code)]
  config: AppConfig,
  focus: FocusHandle,
  active_picker: Option<AnyView>,
  pre_picker_focus: Option<FocusHandle>,
  _picker_subscription: Option<Subscription>,
  split_generation: usize,
  recent_files: Vec<PathBuf>,
  _appearance_subscription: Subscription,
  _focus_in_subscriptions: Vec<Subscription>,
  _hook_server: Option<HookServer>,
  _hook_poll_task: Option<Task<()>>,
  _ipc_poll_task: Option<Task<()>>,
  _breadcrumb_observers: Vec<Subscription>,
  /// Periodic task that re-arms scheduled-send timers from the live TODO markers.
  _schedule_reconcile_task: Option<Task<()>>,
  /// Scheduled sends that already have a timer armed, so reconciliation doesn't
  /// double-arm the same one. Keyed `(project path, heading id, delivery
  /// time)` — the id, not the label: two headings can share a label at any
  /// instant (see `SessionKey`).
  armed_schedules: std::collections::HashSet<(PathBuf, String, NaiveDateTime)>,
  global_todo_view: Entity<crate::views::code_view::CodeView>,
  keybinding_help: Option<(AnyView, Subscription)>,
  pre_help_focus: Option<FocusHandle>,
  window_active: bool,
  _window_activation_subscription: Subscription,
  _notification_poll_task: Option<Task<()>>,
  close_confirm: Option<(AnyView, Subscription)>,
  pre_close_confirm_focus: Option<FocusHandle>,
  /// Whether the pending close is a quit (vs window close).
  close_confirm_is_quit: bool,
  /// The `claude` CLI every background-session command goes through.
  claude: Claude,
  /// Hooks from a known project that matched no session, kept until the next
  /// `claude agents --json` refresh teaches jc the uuid they carry.
  unmatched_hooks: VecDeque<HookEvent>,
  /// A `claude agents --json` refresh for unmatched hooks is in flight.
  refreshing_agents: bool,
  /// When the last such refresh started. Hooks from a `claude` jc does not
  /// manage also match nothing, so refreshes are rate-limited.
  last_agents_refresh: Option<Instant>,
  /// Short ids of `[DELETED]` headings jc has already stopped and removed.
  removed_shorts: HashSet<String>,
  /// Receives each `cira status --json` read by the Cira poll thread.
  _cira_poll_task: Task<()>,
  /// Whether the last read reached Cira; `None` before the first. Logged on change.
  cira_reachable: Option<bool>,
}

/// Unmatched hooks kept for re-dispatch; older ones are dropped.
const UNMATCHED_HOOKS_MAX: usize = 32;
/// The shortest gap between two `claude agents --json` refreshes for
/// unmatched hooks.
const AGENTS_REFRESH_GAP: StdDuration = StdDuration::from_secs(30);
/// The pause between two reads of `cira status --json`.
const CIRA_POLL: StdDuration = StdDuration::from_secs(2);

impl Workspace {
  pub fn new(
    state: AppState,
    config: AppConfig,
    ipc_rx: flume::Receiver<PathBuf>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let palette = palette_from_window(window);
    let claude = Claude::default();

    // Build a ProjectState per registered project.
    let mut projects = Vec::new();
    for project in &state.projects {
      projects.push(ProjectState::create(
        project.path.clone(),
        project.name(),
        &palette,
        window,
        cx,
      ));
    }

    // If no projects registered, create a default one from cwd.
    if projects.is_empty() {
      let path = std::env::current_dir().unwrap_or_default();
      let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".into());
      projects.push(ProjectState::create(path, name, &palette, window, cx));
    }

    // Create global TODO view (~/.claude/TODO.md) as a read-only CodeView.
    let global_todo_path =
      PathBuf::from(std::env::var("HOME").expect("HOME not set")).join(".claude/TODO.md");
    let global_todo_view = cx.new(|cx| {
      let mut cv = crate::views::code_view::CodeView::new(window, cx);
      if global_todo_path.exists() {
        cv.open_file(global_todo_path, window, cx);
      }
      cv
    });

    // Determine initial pane content from first project's first session.
    let initial_contents = Self::initial_pane_contents(&projects[0], &global_todo_view, cx);
    let panes: Vec<Entity<Pane>> = initial_contents
      .into_iter()
      .map(|content| cx.new(|cx| Pane::with_content(content, cx)))
      .collect();

    panes[0].read(cx).focus_content(window);

    // Observe system appearance changes and update themes accordingly.
    let appearance_subscription =
      cx.observe_window_appearance(window, |this: &mut Self, window, cx| {
        this.apply_appearance(appearance_from_window(window.appearance()), window, cx);
      });

    // Track window activation so notifications only fire when jc is in the background.
    let window_activation_subscription =
      cx.observe_window_activation(window, move |this: &mut Self, window, _cx| {
        this.window_active = window.is_window_active();
      });

    let mut focus_in_subscriptions = Vec::new();
    for pane in panes.iter() {
      let focus = pane.read(cx).focus_handle(cx);
      // Capture the pane *entity*, not its index: set_layout can reorder the
      // `panes` Vec, so a captured index goes stale and the cache would point
      // at the wrong pane. Resolve the live index when the event fires.
      let pane_entity = pane.clone();
      focus_in_subscriptions.push(cx.on_focus_in(&focus, window, move |this, _window, cx| {
        if let Some(idx) = this.panes.iter().position(|p| p == &pane_entity)
          && this.active_pane_index != idx
        {
          this.active_pane_index = idx;
          cx.notify();
        }
      }));
    }

    // Start hook server for Claude Code integration.
    let project_paths: Vec<PathBuf> = projects.iter().map(|p| p.path.clone()).collect();
    let (hook_server, hook_poll_task) = match HookServer::start(project_paths.clone()) {
      Ok(server) => {
        let port = server.port;
        // Install hooks into each project's settings (fire and forget).
        for path in &project_paths {
          let path = path.clone();
          std::thread::spawn(move || {
            if let Err(e) = jc_core::hooks_settings::install_hooks(&path, port) {
              eprintln!("failed to install hooks for {}: {e}", path.display());
            }
          });
        }
        // Spawn async task to consume hook events.
        let rx = server.rx.clone();
        let task =
          cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            while let Ok(event) = rx.recv_async().await {
              let Ok(should_continue) = this.update_in(cx, |view, window, cx| {
                view.handle_hook_event(event, window, cx);
                true
              }) else {
                break;
              };
              if !should_continue {
                break;
              }
            }
          });
        (Some(server), Some(task))
      }
      Err(e) => {
        eprintln!("failed to start hook server: {e}");
        (None, None)
      }
    };

    // Poll IPC channel for open_project requests from other `jc` invocations.
    let ipc_poll_task =
      cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
        while let Ok(path) = ipc_rx.recv_async().await {
          let Ok(should_continue) = cx.update(|window, cx| {
            if let Some(entity) = this.upgrade() {
              entity.update(cx, |ws, cx| ws.open_project(path, window, cx));
              window.activate_window();
              true
            } else {
              false
            }
          }) else {
            break;
          };
          if !should_continue {
            break;
          }
        }
      });

    // Initialize notification system and poll for notification click responses.
    let notification_action_rx = crate::notify::action_receiver();
    crate::notify::init();
    let notification_poll_task =
      cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
        while let Ok(session_id) = notification_action_rx.recv_async().await {
          let Ok(should_continue) = cx.update(|window, cx| {
            if let Some(entity) = this.upgrade() {
              entity.update(cx, |ws, cx| ws.switch_to_session_id(&session_id, window, cx));
              window.activate_window();
              true
            } else {
              false
            }
          }) else {
            break;
          };
          if !should_continue {
            break;
          }
        }
      });

    // Cira's pending counts and session states (Cira PLAN TMUX-05/06), read on
    // the poll's own thread; only applying a read runs here.
    let cira = jc_core::cira::Cira::default();
    let cira_rx = jc_core::cira::poll(CIRA_POLL, move || cira.status());
    let cira_poll_task =
      cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
        while let Ok(status) = cira_rx.recv_async().await {
          let applied = this.update(cx, |ws, cx| {
            if ws.apply_cira(status.as_ref()) {
              cx.notify();
            }
          });
          if applied.is_err() {
            break; // workspace dropped; dropping the receiver ends the poll thread
          }
        }
      });

    let mut ws = Self {
      panes,
      active_pane_index: 0,
      layout: PaneLayout::default(),
      projects,
      active_project_index: 0,
      config,
      focus: cx.focus_handle(),
      active_picker: None,
      pre_picker_focus: None,
      _picker_subscription: None,
      split_generation: 0,
      recent_files: Vec::new(),
      _appearance_subscription: appearance_subscription,
      _focus_in_subscriptions: focus_in_subscriptions,
      _hook_server: hook_server,
      _hook_poll_task: hook_poll_task,
      _ipc_poll_task: Some(ipc_poll_task),
      _breadcrumb_observers: Vec::new(),
      _schedule_reconcile_task: None,
      armed_schedules: std::collections::HashSet::new(),
      global_todo_view,
      keybinding_help: None,
      pre_help_focus: None,
      window_active: true,
      _window_activation_subscription: window_activation_subscription,
      _notification_poll_task: Some(notification_poll_task),
      close_confirm: None,
      pre_close_confirm_focus: None,
      close_confirm_is_quit: false,
      claude,
      unmatched_hooks: VecDeque::new(),
      refreshing_agents: false,
      last_agents_refresh: None,
      removed_shorts: HashSet::new(),
      _cira_poll_task: cira_poll_task,
      cira_reachable: None,
    };

    for pi in 0..ws.projects.len() {
      ws.observe_claude_panes(pi, window, cx);
    }
    ws.launch_sessions(window, cx);
    ws.refresh_breadcrumb_observers(cx);
    ws.reconcile_schedules(window, cx);
    ws.start_schedule_reconcile_loop(window, cx);

    // Intercept the native close button (red circle) so it goes through
    // the same confirmation path as Cmd-W instead of closing immediately.
    window.on_window_should_close(cx, move |window, cx| {
      window.dispatch_action(Box::new(CloseWindow), cx);
      false
    });

    ws
  }

  /// Build initial PaneContent for all 3 panes from a project.
  fn initial_pane_contents(
    project: &ProjectState,
    global_todo_view: &Entity<crate::views::code_view::CodeView>,
    cx: &App,
  ) -> Vec<PaneContent> {
    let first = if let Some(session) = project.active_session() {
      let focus = session.claude_terminal.read(cx).focus_handle(cx);
      PaneContent {
        kind: PaneContentKind::ClaudeTerminal,
        view: session.claude_terminal.clone().into(),
        focus,
      }
    } else {
      let focus = project.todo_view.read(cx).focus_handle(cx);
      PaneContent {
        kind: PaneContentKind::TodoEditor,
        view: project.todo_view.clone().into(),
        focus,
      }
    };

    let second = {
      let focus = project.todo_view.read(cx).focus_handle(cx);
      PaneContent {
        kind: PaneContentKind::TodoEditor,
        view: project.todo_view.clone().into(),
        focus,
      }
    };

    let third = {
      let focus = global_todo_view.read(cx).focus_handle(cx);
      PaneContent {
        kind: PaneContentKind::GlobalTodo,
        view: global_todo_view.clone().into(),
        focus,
      }
    };

    vec![first, second, third]
  }

  /// Observe CodeView entities so the pane header re-renders when breadcrumbs change.
  fn refresh_breadcrumb_observers(&mut self, cx: &mut Context<Self>) {
    let mut observers = Vec::new();

    // Global TODO view.
    observers.push(cx.observe(&self.global_todo_view, |_, _, cx| cx.notify()));

    let project = &self.projects[self.active_project_index];

    // Active project's todo_view inner code_view.
    let todo_cv = project.todo_view.read(cx).code_view().clone();
    observers.push(cx.observe(&todo_cv, |_, _, cx| cx.notify()));

    // Active session's code view.
    if let Some(cv) = project.code_view() {
      let cv = cv.clone();
      observers.push(cx.observe(&cv, |_, _, cx| cx.notify()));
    }

    self._breadcrumb_observers = observers;
  }

  // ---------------------------------------------------------------------------
  // Accessors
  // ---------------------------------------------------------------------------

  fn active_project(&self) -> &ProjectState {
    &self.projects[self.active_project_index]
  }

  // ---------------------------------------------------------------------------
  // Appearance
  // ---------------------------------------------------------------------------

  fn apply_appearance(
    &mut self,
    appearance: Appearance,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    Theme::sync_system_appearance(Some(window), cx.deref_mut());
    self.update_terminal_palettes(appearance, cx);
    cx.notify();
  }

  fn update_terminal_palettes(&mut self, appearance: Appearance, cx: &mut Context<Self>) {
    let palette = Palette::for_appearance(appearance);
    for project in &self.projects {
      for session in project.sessions.values() {
        session.claude_terminal.update(cx, |view, _cx| {
          view.set_palette(palette.clone());
        });
        session.general_terminal.update(cx, |view, _cx| {
          view.set_palette(palette.clone());
        });
      }
    }
  }

  // ---------------------------------------------------------------------------
  // Window actions
  // ---------------------------------------------------------------------------

  fn close_window(&mut self, _: &CloseWindow, window: &mut Window, cx: &mut Context<Self>) {
    if self.close_confirm.is_some() {
      return;
    }
    self.try_close(false, window, cx);
  }

  fn minimize_window(&mut self, _: &MinimizeWindow, window: &mut Window, _cx: &mut Context<Self>) {
    window.minimize_window();
  }

  fn quit(&mut self, _: &Quit, window: &mut Window, cx: &mut Context<Self>) {
    if self.close_confirm.is_some() {
      return;
    }
    self.try_close(true, window, cx);
  }

  /// Auto-save dirty buffers and close/quit. Shows confirm dialog if there
  /// are buffers with merge conflicts that can't be saved. Working sessions
  /// are no reason to ask: closing only detaches from background sessions,
  /// which keep running.
  fn try_close(&mut self, is_quit: bool, window: &mut Window, cx: &mut Context<Self>) {
    let conflicts = self.save_all_dirty(cx);

    if conflicts.is_empty() {
      // Nothing to warn about — just close.
      if is_quit {
        cx.quit();
      } else {
        window.remove_window();
      }
    } else {
      self.show_close_confirm(conflicts, is_quit, window, cx);
    }
  }

  /// Save all dirty buffers (TODO views, code views, global TODO).
  /// Returns a list of file names that had merge conflicts and couldn't be saved.
  fn save_all_dirty(&mut self, cx: &mut Context<Self>) -> Vec<String> {
    let mut conflicts = Vec::new();

    for project in &self.projects {
      // TODO view
      if project.todo_view.read(cx).is_dirty(cx) {
        project.todo_view.update(cx, |tv, cx| tv.save(cx));
      }

      // Per-session code views
      for session in project.sessions.values() {
        let cv = session.code_view.read(cx);
        if cv.is_dirty(cx) {
          if cv.has_conflict() {
            if let Some(path) = cv.file_path() {
              let relative = path.strip_prefix(&project.path).unwrap_or(path);
              conflicts.push(relative.display().to_string());
            }
          } else {
            session.code_view.update(cx, |v, cx| v.save(cx));
          }
        }
      }
    }

    // Global TODO
    if self.global_todo_view.read(cx).is_dirty(cx) {
      if self.global_todo_view.read(cx).has_conflict() {
        conflicts.push("~/.claude/TODO.md".to_string());
      } else {
        self.global_todo_view.update(cx, |v, cx| v.save(cx));
      }
    }

    conflicts
  }

  fn show_close_confirm(
    &mut self,
    conflicts: Vec<String>,
    is_quit: bool,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.close_confirm_is_quit = is_quit;
    self.pre_close_confirm_focus = window.focused(cx);
    let view = cx.new(|cx| CloseConfirm::new(conflicts, is_quit, cx));
    let sub = cx.subscribe_in(&view, window, |this: &mut Self, _, event, window, cx| match event {
      CloseConfirmEvent::Confirmed => {
        this.close_confirm = None;
        this.pre_close_confirm_focus = None;
        if this.close_confirm_is_quit {
          cx.quit();
        } else {
          window.remove_window();
        }
      }
      CloseConfirmEvent::Cancelled => {
        this.close_confirm = None;
        if let Some(focus) = this.pre_close_confirm_focus.take() {
          focus.focus(window);
        }
        cx.notify();
      }
    });
    view.read(cx).focus_handle(cx).focus(window);
    self.close_confirm = Some((view.into(), sub));
    cx.notify();
  }

  // ---------------------------------------------------------------------------
  // Pane focus
  // ---------------------------------------------------------------------------

  fn visible_pane_count(&self) -> usize {
    match self.layout {
      PaneLayout::One => 1,
      PaneLayout::Two => 2,
      PaneLayout::Three => 3,
    }
  }

  /// Visible pane that currently holds keyboard focus, if any. Returns `None`
  /// when focus is outside every pane (e.g. a modal/picker is open). This is the
  /// source of truth for "which pane is active"; `active_pane_index` is only a
  /// cache that can briefly lag real focus.
  fn focused_pane_index(&self, window: &Window, cx: &App) -> Option<usize> {
    (0..self.visible_pane_count())
      .find(|&i| self.panes[i].read(cx).focus_handle(cx).contains_focused(window, cx))
  }

  /// Correct the cached `active_pane_index` to match real keyboard focus.
  /// Call before reading the cache to drive an action, so a briefly-stale cache
  /// can't make e.g. Cmd-Enter or Cmd-S act on the wrong pane.
  fn sync_active_pane_to_focus(&mut self, window: &Window, cx: &App) {
    if let Some(idx) = self.focused_pane_index(window, cx) {
      self.active_pane_index = idx;
    }
  }

  fn focus_prev_pane(&mut self, _: &FocusPrevPane, window: &mut Window, cx: &mut Context<Self>) {
    // Start from where focus actually is, not the (possibly stale) cache, so the
    // first press never gets "wasted" resyncing and appears to skip a pane.
    let current = self.focused_pane_index(window, cx).unwrap_or(self.active_pane_index);
    if current > 0 {
      self.active_pane_index = current - 1;
      self.panes[self.active_pane_index].read(cx).focus_content(window);
      cx.notify();
    }
  }

  fn focus_next_pane(&mut self, _: &FocusNextPane, window: &mut Window, cx: &mut Context<Self>) {
    let count = self.visible_pane_count();
    let current = self.focused_pane_index(window, cx).unwrap_or(self.active_pane_index);
    if current + 1 < count {
      self.active_pane_index = current + 1;
      self.panes[self.active_pane_index].read(cx).focus_content(window);
      cx.notify();
    }
  }

  // ---------------------------------------------------------------------------
  // Scroll other pane
  // ---------------------------------------------------------------------------

  /// Returns the index of the leftmost non-active visible pane, if any.
  fn other_pane_index(&self) -> Option<usize> {
    let visible = self.visible_pane_count();
    (0..visible).find(|&i| i != self.active_pane_index)
  }

  /// Resolve the other pane's scrollable target into a concrete entity.
  fn other_pane_scrollable(&self, cx: &App) -> Option<OtherPaneScrollable> {
    let idx = self.other_pane_index()?;
    let kind = self.panes[idx].read(cx).content_kind()?;
    let project = self.active_project();
    match kind {
      PaneContentKind::CodeViewer => {
        project.code_view().map(|cv| OtherPaneScrollable::Editor(cv.read(cx).editor().clone()))
      }
      PaneContentKind::TodoEditor => {
        let editor = project.todo_view.read(cx).editor(cx);
        Some(OtherPaneScrollable::Editor(editor))
      }
      PaneContentKind::GlobalTodo => {
        Some(OtherPaneScrollable::Editor(self.global_todo_view.read(cx).editor().clone()))
      }
      PaneContentKind::ClaudeTerminal => {
        project.active_session().map(|s| OtherPaneScrollable::Terminal(s.claude_terminal.clone()))
      }
      PaneContentKind::GeneralTerminal => {
        project.active_session().map(|s| OtherPaneScrollable::Terminal(s.general_terminal.clone()))
      }
    }
  }

  fn scroll_other_by(&mut self, lines: isize, cx: &mut Context<Self>) {
    match self.other_pane_scrollable(cx) {
      Some(OtherPaneScrollable::Editor(e)) => {
        e.update(cx, |s, cx| s.scroll_by_lines(lines, cx));
      }
      Some(OtherPaneScrollable::Terminal(t)) => {
        t.update(cx, |tv, cx| tv.scroll_lines(lines as i32, cx));
      }
      None => {}
    }
  }

  fn scroll_other_by_pages(&mut self, pages: isize, cx: &mut Context<Self>) {
    match self.other_pane_scrollable(cx) {
      Some(OtherPaneScrollable::Editor(e)) => {
        e.update(cx, |s, cx| s.scroll_by_pages(pages, cx));
      }
      Some(OtherPaneScrollable::Terminal(t)) => {
        t.update(cx, |tv, cx| tv.scroll_pages(pages as i32, cx));
      }
      None => {}
    }
  }

  fn scroll_other_up(&mut self, _: &ScrollOtherUp, _window: &mut Window, cx: &mut Context<Self>) {
    self.scroll_other_by(-3, cx);
  }

  fn scroll_other_down(
    &mut self,
    _: &ScrollOtherDown,
    _window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.scroll_other_by(3, cx);
  }

  fn scroll_other_page_up(
    &mut self,
    _: &ScrollOtherPageUp,
    _window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.scroll_other_by_pages(-1, cx);
  }

  fn scroll_other_page_down(
    &mut self,
    _: &ScrollOtherPageDown,
    _window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.scroll_other_by_pages(1, cx);
  }

  fn set_layout(&mut self, layout: PaneLayout, window: &mut Window, cx: &mut Context<Self>) {
    self.layout = layout;
    let count = self.visible_pane_count();
    // If the focused pane would be hidden, swap it into a visible position.
    if self.active_pane_index >= count {
      self.panes.swap(0, self.active_pane_index);
      self.active_pane_index = 0;
    }
    self.panes[self.active_pane_index].read(cx).focus_content(window);
    self.split_generation += 1;
    cx.notify();
  }

  fn set_layout_one(&mut self, _: &SetLayoutOne, window: &mut Window, cx: &mut Context<Self>) {
    self.set_layout(PaneLayout::One, window, cx);
  }

  fn set_layout_two(&mut self, _: &SetLayoutTwo, window: &mut Window, cx: &mut Context<Self>) {
    self.set_layout(PaneLayout::Two, window, cx);
  }

  fn set_layout_three(&mut self, _: &SetLayoutThree, window: &mut Window, cx: &mut Context<Self>) {
    self.set_layout(PaneLayout::Three, window, cx);
  }

  fn active_pane_entity(&self) -> &Entity<Pane> {
    &self.panes[self.active_pane_index]
  }

  // ---------------------------------------------------------------------------
  // View switching
  // ---------------------------------------------------------------------------

  fn set_active_pane_view(
    &mut self,
    kind: PaneContentKind,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.show_in_pane(self.active_pane_index, kind, window, cx);
  }

  pub(super) fn show_in_pane(
    &mut self,
    pane_idx: usize,
    kind: PaneContentKind,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.set_pane_view(pane_idx, kind, cx);
    self.active_pane_index = pane_idx;

    // When switching to the TODO editor, ensure a WAIT section exists and scroll to it.
    if kind == PaneContentKind::TodoEditor {
      let project = &self.projects[self.active_project_index];
      if let Some(uuid) = project.active_key().map(str::to_string) {
        let todo_view = project.todo_view.clone();
        todo_view.update(cx, |tv, cx| tv.ensure_wait(&uuid, window, cx));
        if let Some(wait_line) = todo_view.read(cx).wait_line(&uuid, cx) {
          todo_view.update(cx, |tv, cx| tv.scroll_to_line(wait_line, window, cx));
        }
      }
    }

    // Focus last so nothing after can clobber it.
    self.panes[pane_idx].read(cx).focus_content(window);
    cx.notify();
  }

  fn show_claude_terminal(
    &mut self,
    _: &ShowClaudeTerminal,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.set_active_pane_view(PaneContentKind::ClaudeTerminal, window, cx);
  }

  fn show_general_terminal(
    &mut self,
    _: &ShowGeneralTerminal,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.set_active_pane_view(PaneContentKind::GeneralTerminal, window, cx);
  }

  fn show_code_viewer(&mut self, _: &ShowCodeViewer, window: &mut Window, cx: &mut Context<Self>) {
    self.set_active_pane_view(PaneContentKind::CodeViewer, window, cx);
  }

  fn show_todo_editor(&mut self, _: &ShowTodoEditor, window: &mut Window, cx: &mut Context<Self>) {
    self.set_active_pane_view(PaneContentKind::TodoEditor, window, cx);
  }

  fn toggle_keybinding_help(
    &mut self,
    _: &ShowKeybindingHelp,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.keybinding_help.is_some() {
      self.keybinding_help = None;
      if let Some(focus) = self.pre_help_focus.take() {
        focus.focus(window);
      }
      cx.notify();
    } else {
      self.pre_help_focus = window.focused(cx);
      let view = cx.new(KeybindingHelp::new);
      let sub =
        cx.subscribe_in(&view, window, |this: &mut Self, _, _: &DismissHelpEvent, window, cx| {
          this.keybinding_help = None;
          if let Some(focus) = this.pre_help_focus.take() {
            focus.focus(window);
          }
          cx.notify();
        });
      view.read(cx).focus_handle(cx).focus(window);
      self.keybinding_help = Some((view.into(), sub));
      cx.notify();
    }
  }

  // ---------------------------------------------------------------------------
  // Project opening (from IPC)
  // ---------------------------------------------------------------------------

  pub fn open_project(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
    let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());

    // If the project is already loaded, just switch to it.
    if let Some(idx) = self.projects.iter().position(|p| p.path == canonical) {
      let active = self.projects[idx].active_session;
      self.switch_to_session(idx, active, window, cx);
      return;
    }

    // Create a new ProjectState and switch to it.
    let palette = palette_from_window(window);
    let name = canonical
      .file_name()
      .map(|n| n.to_string_lossy().into_owned())
      .unwrap_or_else(|| "unknown".into());

    let project = ProjectState::create(canonical.clone(), name, &palette, window, cx);
    self.projects.push(project);

    let project_idx = self.projects.len() - 1;
    self.observe_claude_panes(project_idx, window, cx);
    self.launch_sessions(window, cx);
    let active = self.projects[project_idx].active_session;
    self.switch_to_session(project_idx, active, window, cx);

    // Persist to state.toml.
    if let Ok(mut state) = jc_core::config::load_state() {
      state.register_project(&canonical);
      let _ = jc_core::config::save_state(&state);
    }
  }

  // ---------------------------------------------------------------------------
  // Session switching
  // ---------------------------------------------------------------------------

  fn switch_to_session(
    &mut self,
    project_idx: usize,
    session_id: Option<SessionId>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    // Save the outgoing session's viewport state.
    let todo_editor = self.projects[self.active_project_index].todo_view.read(cx).editor(cx);
    let todo_cursor = todo_editor.read(cx).cursor_position();
    let todo_scroll = todo_editor.read(cx).scroll_offset();
    if let Some(session) = self.projects[self.active_project_index].active_session_mut() {
      session.saved_view = Some(SavedViewState {
        layout: SavedPaneLayout {
          pane_kinds: std::array::from_fn(|i| self.panes[i].read(cx).content_kind()),
          active_pane_index: self.active_pane_index,
        },
        todo_cursor,
        todo_scroll,
        claude_scroll: session.claude_terminal.read(cx).display_offset(),
        general_scroll: session.general_terminal.read(cx).display_offset(),
      });
      session.claude_terminal.read(cx).set_visible(false);
      session.general_terminal.read(cx).set_visible(false);
      // Whatever this session finished while it was on screen has been seen,
      // so it must not come back marked in the Cmd-P picker. Stamping on the
      // way OUT rather than on the way in is what makes that true regardless
      // of how the switch was made (picker, Cmd-`, a notification click).
      session.seen = Some(SystemTime::now());
    }

    self.active_project_index = project_idx;
    self.projects[project_idx].active_session = session_id;

    // Restore incoming session's terminal visibility and scroll positions.
    if let Some(session) = self.projects[project_idx].active_session() {
      session.claude_terminal.read(cx).set_visible(true);
      session.general_terminal.read(cx).set_visible(true);
      if let Some(saved) = &session.saved_view {
        session.claude_terminal.read(cx).set_display_offset(saved.claude_scroll);
        session.general_terminal.read(cx).set_display_offset(saved.general_scroll);
      }
    }

    // Restore incoming session's TODO cursor and scroll position.
    if let Some(saved) =
      self.projects[project_idx].active_session().and_then(|s| s.saved_view.as_ref())
    {
      let cursor = saved.todo_cursor;
      let scroll = saved.todo_scroll;
      let todo_editor = self.projects[project_idx].todo_view.read(cx).editor(cx);
      todo_editor.update(cx, |state, cx| {
        state.set_cursor_position(cursor, window, cx);
        state.set_scroll_offset(scroll, cx);
      });
    }

    // Update the TODO view's active session highlight.
    let uuid = self.projects[project_idx].active_key().map(|s| s.to_string());
    let todo_view = self.projects[project_idx].todo_view.clone();
    todo_view.update(cx, |tv, cx| tv.set_active_key(uuid.as_deref(), cx));

    // Breadcrumb observers depend on the active session's code_view.
    self.refresh_breadcrumb_observers(cx);

    self.restore_or_default_panes(window, cx);

    cx.notify();
  }

  fn rotate_next_project(
    &mut self,
    _: &RotateNextProject,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    // Build a flat list of (project_index, session_id) in deterministic order.
    // Skip projects with no attached sessions.
    let mut slots: Vec<(usize, Option<SessionId>)> = Vec::new();
    for (pi, project) in self.projects.iter().enumerate() {
      if project.sessions.is_empty() {
        continue;
      }
      let mut ids: Vec<SessionId> = project.sessions.keys().copied().collect();
      ids.sort();
      for id in ids {
        slots.push((pi, Some(id)));
      }
    }
    if slots.len() <= 1 {
      return;
    }

    let current =
      (self.active_project_index, self.projects[self.active_project_index].active_session);
    let pos = slots.iter().position(|s| *s == current).unwrap_or(0);
    let (next_pi, next_sid) = slots[(pos + 1) % slots.len()];
    self.switch_to_session(next_pi, next_sid, window, cx);
  }

  /// Switch to the session a clicked notification names. Notifications carry
  /// the hook's conversation uuid.
  fn switch_to_session_id(
    &mut self,
    session_id: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if let Some((pi, id)) = self.session_for_hook(session_id) {
      self.switch_to_session(pi, Some(id), window, cx);
    }
  }

  /// Restore saved pane contents for the active session, or use defaults.
  /// The pane layout (1/2/3) is window-level and not restored here.
  fn restore_or_default_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    // Copy saved layout data to avoid borrow conflict with set_pane_view.
    let saved = self.projects[self.active_project_index]
      .active_session()
      .and_then(|s| s.saved_view.as_ref())
      .map(|s| (s.layout.pane_kinds, s.layout.active_pane_index));

    if let Some((kinds, active)) = saved {
      for (i, kind) in kinds.iter().enumerate() {
        if let Some(kind) = kind {
          self.set_pane_view(i, *kind, cx);
        }
      }
      self.active_pane_index = active.min(self.visible_pane_count() - 1);
      self.split_generation += 1;
      self.panes[self.active_pane_index].read(cx).focus_content(window);
    } else {
      // First visit: default layout. With no active session there is no Claude
      // terminal to show, and `set_pane_view` would leave pane 0 holding
      // whatever it had — including the terminal of a session just detached,
      // which the stale `AnyView` would also keep alive and focusable. Show the
      // project's TODO instead, which needs no session.
      let first = if self.active_project().active_session().is_some() {
        PaneContentKind::ClaudeTerminal
      } else {
        PaneContentKind::TodoEditor
      };
      self.set_pane_view(0, first, cx);
      self.set_pane_view(1, PaneContentKind::TodoEditor, cx);
      self.set_pane_view(2, PaneContentKind::GlobalTodo, cx);
      self.panes[0].read(cx).focus_content(window);
      self.active_pane_index = 0;
    }
  }

  /// Set a specific pane to show a view kind from the active project/session.
  fn set_pane_view(&mut self, pane_idx: usize, kind: PaneContentKind, cx: &mut App) {
    let project = &self.projects[self.active_project_index];
    let result: Option<(AnyView, FocusHandle)> = match kind {
      PaneContentKind::ClaudeTerminal => project.active_session().map(|s| {
        let focus = s.claude_terminal.read(cx).focus_handle(cx);
        (s.claude_terminal.clone().into(), focus)
      }),
      PaneContentKind::GeneralTerminal => project.active_session().map(|s| {
        let focus = s.general_terminal.read(cx).focus_handle(cx);
        (s.general_terminal.clone().into(), focus)
      }),
      PaneContentKind::CodeViewer => project.active_session().map(|s| {
        let focus = s.code_view.read(cx).focus_handle(cx);
        (s.code_view.clone().into(), focus)
      }),
      PaneContentKind::TodoEditor => {
        let focus = project.todo_view.read(cx).focus_handle(cx);
        Some((project.todo_view.clone().into(), focus))
      }
      PaneContentKind::GlobalTodo => {
        let focus = self.global_todo_view.read(cx).focus_handle(cx);
        Some((self.global_todo_view.clone().into(), focus))
      }
    };

    if let Some((view, focus)) = result {
      self.panes[pane_idx].update(cx, |p, cx| {
        p.set_content(PaneContent { kind, view, focus }, cx);
      });
    }
  }

  // ---------------------------------------------------------------------------
  // Session creation and background sessions
  // ---------------------------------------------------------------------------

  /// Run `work` on its own thread (it may block on `claude` for minutes), then
  /// `done` with its result on the main thread, if the workspace still exists.
  fn run_blocking<T: Send + 'static>(
    &self,
    window: &mut Window,
    cx: &mut Context<Self>,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
  ) {
    let (tx, rx) = flume::bounded(1);
    std::thread::spawn(move || {
      let _ = tx.send(work());
    });
    cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| {
      if let Ok(value) = rx.recv_async().await {
        let _ = this.update_in(cx, |ws, window, cx| done(ws, value, window, cx));
      }
    })
    .detach();
  }

  /// Run `f` with the `claude` CLI on its own thread, result unwanted.
  fn detached(&self, f: impl FnOnce(&Claude) + Send + 'static) {
    let claude = self.claude.clone();
    std::thread::spawn(move || f(&claude));
  }

  fn project_index(&self, path: &Path) -> Option<usize> {
    self.projects.iter().position(|p| p.path == path)
  }

  fn session_mut(&mut self, path: &Path, sid: SessionId) -> Option<&mut SessionState> {
    let pi = self.project_index(path)?;
    self.projects[pi].sessions.get_mut(&sid)
  }

  /// Watch every Claude pane in project `pi` that is not watched yet, so an
  /// attach client's restart is noticed (see [`Self::on_claude_child`]).
  fn observe_claude_panes(&mut self, pi: usize, window: &mut Window, cx: &mut Context<Self>) {
    let path = self.projects[pi].path.clone();
    let unwatched: Vec<(SessionId, Entity<TerminalView>)> = self.projects[pi]
      .sessions
      .iter()
      .filter(|(_, s)| s.claude_child_events.is_none())
      .map(|(&sid, s)| (sid, s.claude_terminal.clone()))
      .collect();
    for (sid, terminal) in unwatched {
      let path = path.clone();
      let sub = cx.subscribe_in(&terminal, window, move |ws, _, event: &ChildEvent, window, cx| {
        ws.on_claude_child(&path, sid, *event, window, cx);
      });
      if let Some(session) = self.projects[pi].sessions.get_mut(&sid) {
        session.claude_child_events = Some(sub);
      }
    }
  }

  /// An attach client (re)started or exited. Either way the session is not
  /// known to be reachable until `claude agents --json` or a hook says so.
  fn on_claude_child(
    &mut self,
    path: &Path,
    sid: SessionId,
    event: ChildEvent,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(session) = self.session_mut(path, sid) else { return };
    session.live = false;
    if event == ChildEvent::Spawned {
      self.watch_live(path.to_path_buf(), sid, window, cx);
    }
  }

  /// Poll `claude agents --json` until the session behind `sid`'s pane runs,
  /// then mark it live, learn its uuid, and send what waited for it.
  fn watch_live(
    &mut self,
    path: PathBuf,
    sid: SessionId,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(session) = self.session_mut(&path, sid) else { return };
    if session.bg.is_empty() || session.watching_live {
      return;
    }
    session.watching_live = true;
    let short = session.bg.clone();
    let claude = self.claude.clone();
    self.run_blocking(
      window,
      cx,
      move || claude.wait_settled(&short, Poll::START).map(|a| (short, a)),
      move |ws, result, window, cx| {
        let Some(session) = ws.session_mut(&path, sid) else { return };
        session.watching_live = false;
        match result {
          Ok((short, _)) if session.bg != short || session.changing => {}
          Ok((_, Settled::Live(agent))) => {
            session.live = true;
            if let Some(uuid) = agent.session_id {
              session.uuid = uuid;
            }
          }
          Ok((_, Settled::Gone)) => {
            // Removed elsewhere (`claude rm`, the agents view): attaching can
            // only fail now. Stop trying, and give the heading a session again
            // the way a start does (`plan_launch`: a fresh one).
            session.claude_terminal.update(cx, |t, cx| t.stop_child(cx));
            session.bg.clear();
            ws.launch_sessions(window, cx);
          }
          Ok(_) => {}
          Err(e) => eprintln!("claude agents: {e:#}"),
        }
      },
    );
  }

  /// Give every Claude pane that has no background session yet (`bg` empty,
  /// heading id known, nothing in flight) the session its heading names —
  /// attaching, migrating a `> uuid=`, or starting fresh, per
  /// [`background::plan_launch`] — and, in the same `claude agents --json`
  /// pass, stop the running jobs of `[D]` headings and remove the jobs of
  /// `[DELETED]` ones (Cira Q-37).
  fn launch_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    struct Request {
      path: PathBuf,
      sid: SessionId,
      key: String,
      kind: IdKind,
    }
    let mut requests = Vec::new();
    let mut disabled: Vec<String> = Vec::new();
    let mut deleted: Vec<String> = Vec::new();
    for project in &mut self.projects {
      let doc = project.todo_view.read(cx).document().clone();
      for session in project.sessions.values_mut() {
        if !session.bg.is_empty() || session.key.is_empty() || session.changing {
          continue;
        }
        // Its heading must still be there and enabled: a session left waiting
        // (blocked, failed) whose heading was since deleted or disabled is not
        // started behind the user's back.
        let Some(kind) =
          doc.session_by_id(&session.key).filter(|h| !h.is_disabled()).map(|h| h.id_kind)
        else {
          continue;
        };
        session.failed = false;
        session.changing = true;
        session.status("finding this session's background session…", cx);
        requests.push(Request {
          path: project.path.clone(),
          sid: session.id,
          key: session.key.clone(),
          kind,
        });
      }
      disabled.extend(doc.stoppable_ids().into_iter().map(str::to_string));
      deleted.extend(
        doc
          .removable_ids()
          .into_iter()
          .filter(|id| !self.removed_shorts.contains(*id))
          .map(str::to_string),
      );
    }
    if requests.is_empty() && disabled.is_empty() && deleted.is_empty() {
      return;
    }

    let claude = self.claude.clone();
    let work = move || {
      let agents = claude.agents();
      // `[DELETED]` jobs dealt with: removed, or already unknown to Claude
      // Code. Nothing is recorded when the list itself failed.
      let mut removed = Vec::new();
      if let Ok(agents) = &agents {
        for short in &disabled {
          if background::find(agents, short).is_some_and(|a| a.is_live()) {
            let _ = claude.stop(short).inspect_err(|e| eprintln!("[D] {short}: {e:#}"));
          }
        }
        for short in deleted {
          if background::find(agents, &short).is_some() {
            claude.remove(&short);
          }
          removed.push(short);
        }
      }
      let plans = requests
        .into_iter()
        .map(|r| {
          let plan = match &agents {
            Ok(agents) => {
              let transcript =
                r.kind == IdKind::Uuid && ProjectState::has_transcript(&r.path, &r.key);
              Ok(background::plan_launch(&r.key, r.kind, agents, transcript))
            }
            // Attaching to a known short id is safe without the list; anything
            // that would start a session is not.
            Err(_) if r.kind == IdKind::Bg => {
              Ok(LaunchPlan::Attach { short: r.key.clone(), uuid: None })
            }
            Err(e) => Err(format!("cannot list background sessions: {e:#}")),
          };
          (r.path, r.sid, plan)
        })
        .collect::<Vec<_>>();
      (plans, removed)
    };
    self.run_blocking(window, cx, work, |ws, (plans, removed), window, cx| {
      ws.removed_shorts.extend(removed);
      for (path, sid, plan) in plans {
        match plan {
          Ok(LaunchPlan::Attach { short, uuid }) => {
            ws.finish_launch(&path, sid, &short, uuid, window, cx)
          }
          Ok(LaunchPlan::Start { resume }) => ws.start_session(path, sid, resume, window, cx),
          Ok(LaunchPlan::Blocked) => {
            let msg = "this conversation is open in another Claude Code process (an older \
                       jc?). Close it, then pick this session in Cmd-P or Cmd-Shift-P to move \
                       it to a background session.";
            ws.launch_failed(&path, sid, msg, cx);
          }
          Err(e) => ws.launch_failed(&path, sid, &e, cx),
        }
      }
    });
  }

  /// Start a background session for `sid` with `claude --bg`, continuing
  /// conversation `resume` when given, then bind and attach it.
  fn start_session(
    &mut self,
    path: PathBuf,
    sid: SessionId,
    resume: Option<String>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(session) = self.session_mut(&path, sid) else { return };
    session.changing = true;
    let cira = session.cira.get_or_insert_with(|| uuid::Uuid::new_v4().to_string()).clone();
    let spec = StartSpec {
      label: session.label.clone(),
      cira: Some(cira),
      permissions: if session.dangerous { Permissions::Dangerous } else { Permissions::Unset },
      resume: resume.clone(),
    };
    session.status("starting a background session…", cx);
    let claude = self.claude.clone();
    let cwd = path.clone();
    self.run_blocking(
      window,
      cx,
      move || claude.start(&spec, &cwd, Poll::START),
      move |ws, result, window, cx| match result {
        Ok(short) => {
          // A continued conversation keeps its uuid; a fresh one is learned
          // from its first hook.
          ws.finish_launch(&path, sid, &short, resume, window, cx);
        }
        Err(e) => {
          let msg = format!("could not start a background session: {e:#}");
          ws.launch_failed(&path, sid, &msg, cx);
        }
      },
    );
  }

  /// `sid`'s start failed or was refused: say why in its pane, and stop
  /// counting it as adopted so its heading is offered again.
  fn launch_failed(&mut self, path: &Path, sid: SessionId, why: &str, cx: &mut Context<Self>) {
    let Some(session) = self.session_mut(path, sid) else { return };
    session.changing = false;
    session.failed = true;
    session.status(why, cx);
    cx.notify();
  }

  /// Bind `sid`'s heading to background session `short` — rewriting its id
  /// line, or inserting the heading for a session that has none yet — and
  /// attach the Claude pane to it.
  fn finish_launch(
    &mut self,
    path: &Path,
    sid: SessionId,
    short: &str,
    uuid: Option<String>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(pi) = self.project_index(path) else { return };
    let project = &mut self.projects[pi];
    let todo_view = project.todo_view.clone();
    let is_active = project.active_session == Some(sid);
    let Some(session) = project.sessions.get_mut(&sid) else { return };
    let cira = session.cira.clone();

    let doc = todo_view.read(cx).document();
    let index = match (&session.unbound_key, session.key.is_empty()) {
      (Some(key), _) => doc.index_of(key),
      (None, false) => doc.index_by_id(&session.key),
      (None, true) => None,
    };
    match index {
      Some(index) => {
        let heading = &doc.sessions[index];
        if heading.id != short
          || heading.id_kind != IdKind::Bg
          || (heading.cira.is_none() && cira.is_some())
        {
          todo_view.update(cx, |tv, cx| tv.bind_bg_at(index, short, cira.as_deref(), window, cx));
        }
      }
      None if session.key.is_empty() && session.unbound_key.is_none() => {
        let cira = cira.unwrap_or_default();
        let label = session.label.clone();
        todo_view.update(cx, |tv, cx| tv.insert_session_heading(&label, short, &cira, window, cx));
      }
      None => eprintln!(
        "launch: the heading for {} moved; attaching {short} without recording it",
        session.key
      ),
    }

    session.unbound_key = None;
    session.changing = false;
    session.uuid = uuid.unwrap_or_default();
    session.failed = false;
    session.adopting = None;
    session.attach(short, &self.claude, cx);
    if is_active {
      todo_view.update(cx, |tv, cx| tv.set_active_key(Some(short), cx));
    }
    cx.notify();
  }

  /// Add a session whose pane waits for a background session, and switch to it.
  fn add_pending_session(
    &mut self,
    pi: usize,
    heading: Heading,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> SessionId {
    let project_path = self.projects[pi].path.clone();
    let palette = palette_from_window(window);
    let project = &mut self.projects[pi];
    let id = project.next_session_id;
    project.next_session_id += 1;
    let session = SessionState::create(id, heading, &project_path, &palette, window, cx);
    project.sessions.insert(id, session);
    self.observe_claude_panes(pi, window, cx);
    self.switch_to_session(pi, Some(id), window, cx);
    id
  }

  /// Start a brand-new background session; its heading is written once
  /// `claude --bg` has printed the short id that addresses it.
  fn create_new_session(
    &mut self,
    project_idx: usize,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    // Cosmetic only — nothing addresses a session by label (see
    // `jc_core::todo::SessionKey`); this just stops the picker showing
    // several identical rows.
    let label = jc_core::todo::unique_label(
      self.projects[project_idx].todo_view.read(cx).document(),
      "New Session",
    );
    self.start_unrecorded(project_idx, label, None, window, cx);
  }

  /// Continue transcript `uuid`, which no heading names, as a background
  /// session under a new heading labelled from `summary`.
  pub(super) fn adopt_transcript(
    &mut self,
    project_idx: usize,
    uuid: String,
    summary: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    // A second confirm while the first start is in flight would fork the
    // conversation a second time.
    let in_flight = |s: &SessionState| s.adopting.as_deref() == Some(uuid.as_str());
    if self.projects[project_idx].sessions.values().any(in_flight) {
      crate::notify::beep();
      return;
    }
    // The summary is the transcript's first user message, so two sessions that
    // opened with the same prompt would otherwise get the same display name.
    let label = jc_core::todo::unique_label(
      self.projects[project_idx].todo_view.read(cx).document(),
      summary,
    );
    self.start_unrecorded(project_idx, label, Some(uuid), window, cx);
  }

  fn start_unrecorded(
    &mut self,
    pi: usize,
    label: String,
    resume: Option<String>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let heading = Heading::new(String::new(), IdKind::Bg, label, None, false);
    let id = self.add_pending_session(pi, heading, window, cx);
    let path = self.projects[pi].path.clone();
    if let Some(session) = self.projects[pi].sessions.get_mut(&id) {
      session.adopting = resume.clone();
    }
    self.start_session(path, id, resume, window, cx);
  }

  /// Adopt a TODO.md session that isn't running yet: its pane attaches to the
  /// background session the heading names, migrating a `> uuid=` heading or
  /// starting fresh as [`background::plan_launch`] decides.
  ///
  /// `key` may be a `SessionKey::Unbound` — a heading an older jc left with a
  /// blank `> uuid=`. It gets a fresh background session, and its id line is
  /// written when that session's short id is known; that is something the
  /// user chose to do, never done at startup. The heading is re-resolved
  /// against the *live* document, so a key taken from a picker snapshot cannot
  /// write to a heading that has moved since.
  fn adopt_session(
    &mut self,
    project_idx: usize,
    key: &SessionKey,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let todo_view = self.projects[project_idx].todo_view.clone();
    let resolved = todo_view.read(cx).document().session_of(key).cloned();

    // Refuse to launch a session whose heading isn't in the file. It could
    // never be label-synced or disabled afterwards, and the heading would stay
    // adoptable — so a second press would start a second session for it.
    let Some(heading) = resolved else {
      eprintln!("adopt: no TODO heading for {key:?}; not launching");
      // The picker already dropped `pre_picker_focus` on the assumption that
      // this call would focus something. It won't, so do it here or the next
      // keystroke goes nowhere.
      self.panes[self.active_pane_index].read(cx).focus_content(window);
      return;
    };

    // A heading already being started (an unbound one adopted twice) gets
    // one session; a session whose start failed makes way for a new try.
    let project = &self.projects[project_idx];
    if project.sessions.values().any(|s| s.unbound_key.as_ref() == Some(key) && !s.failed) {
      crate::notify::beep();
      return;
    }
    let failed: Vec<SessionId> = project
      .sessions
      .values()
      .filter(|s| {
        s.failed
          && (s.unbound_key.as_ref() == Some(key)
            || (!heading.id.is_empty() && s.key == heading.id))
      })
      .map(|s| s.id)
      .collect();
    for sid in failed {
      self.remove_running_session(project_idx, sid, window, cx);
    }

    // Clear `[D]` if the heading was dormant (re-attaching resumes the
    // stopped session).
    if heading.is_disabled() {
      todo_view.update(cx, |tv, cx| {
        tv.toggle_session_disabled(key, window, cx);
        tv.save(cx);
      });
    }

    let unbound = heading.id.is_empty();
    let pending = Heading::new(
      heading.id.clone(),
      heading.id_kind,
      heading.label.clone(),
      heading.cira.clone(),
      heading.dangerous,
    );
    let id = self.add_pending_session(project_idx, pending, window, cx);
    if unbound {
      let path = self.projects[project_idx].path.clone();
      if let Some(session) = self.projects[project_idx].sessions.get_mut(&id) {
        session.unbound_key = Some(key.clone());
      }
      self.start_session(path, id, None, window, cx);
    } else {
      self.launch_sessions(window, cx);
    }
  }

  /// Recreate `sid`'s background session with `dangerous` as its permission
  /// mode (Cira Q-52): Claude Code keeps a job's saved options, so this stops
  /// it, continues its conversation in a copy with the new mode, points the
  /// heading at the copy, and removes the old job.
  fn change_dangerous(
    &mut self,
    path: PathBuf,
    sid: SessionId,
    dangerous: bool,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(session) = self.session_mut(&path, sid) else { return };
    session.changing = true;
    session.live = false;
    let old = session.bg.clone();
    let known_uuid = session.uuid.clone();
    let label = session.label.clone();
    let cira = session.cira.clone();
    // Detach first (see `SessionState::attach`).
    session.claude_terminal.update(cx, |t, cx| {
      t.stop_child(cx);
      t.show_status(
        if dangerous {
          "restarting with --dangerously-skip-permissions…"
        } else {
          "restarting without --dangerously-skip-permissions…"
        },
        cx,
      );
    });
    let claude = self.claude.clone();
    let cwd = path.clone();
    let old_for_work = old.clone();
    let work = move || -> anyhow::Result<String> {
      let uuid = if known_uuid.is_empty() {
        let agents = claude.agents()?;
        background::find(&agents, &old_for_work).and_then(|a| a.session_id.clone())
      } else {
        Some(known_uuid)
      };
      let transcript = uuid.as_deref().is_some_and(|uuid| ProjectState::has_transcript(&cwd, uuid));
      let spec =
        background::toggle_spec(&label, cira.as_deref(), dangerous, uuid.as_deref(), transcript);
      claude.toggle_dangerous(&old_for_work, &spec, &cwd, Poll::START)
    };
    self.run_blocking(window, cx, work, move |ws, result, window, cx| {
      let claude = ws.claude.clone();
      let Some(session) = ws.session_mut(&path, sid) else { return };
      match result {
        Ok(new_short) => {
          session.dangerous = dangerous;
          // The copy is a new conversation uuid, learned from its first hook.
          ws.finish_launch(&path, sid, &new_short, None, window, cx);
        }
        Err(e) => {
          session.changing = false;
          session.refused_dangerous = Some(dangerous);
          session.status(&format!("could not change > dangerous: {e:#}"), cx);
          // Back to the old job; attaching resumes it if it was stopped.
          session.attach(&old, &claude, cx);
        }
      }
    });
  }

  /// Bring running background sessions in line with TODO.md: recreate one
  /// whose `> dangerous` changed, detach, stop and remove one whose heading
  /// became `[DELETED]`, and send messages that were waiting for a session to
  /// come up.
  fn reconcile_background(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let mut toggles = Vec::new();
    let mut gone = Vec::new();
    for (pi, project) in self.projects.iter_mut().enumerate() {
      let doc = project.todo_view.read(cx).document();
      let removable = doc.removable_ids();
      for session in project.sessions.values_mut() {
        if session.bg.is_empty() || session.changing {
          continue;
        }
        if removable.contains(&session.key.as_str()) {
          gone.push((pi, session.id, session.bg.clone()));
          continue;
        }
        if let Some(heading) = doc.session_by_id(&session.key) {
          if session.refused_dangerous.is_some_and(|r| r != heading.dangerous) {
            session.refused_dangerous = None;
          }
          if heading.dangerous != session.dangerous
            && session.refused_dangerous != Some(heading.dangerous)
          {
            toggles.push((project.path.clone(), session.id, heading.dangerous));
          }
        }
      }
    }
    for (path, sid, dangerous) in toggles {
      self.change_dangerous(path, sid, dangerous, window, cx);
    }
    for (pi, sid, short) in gone {
      self.removed_shorts.insert(short.clone());
      self.remove_running_session(pi, sid, window, cx);
      self.detached(move |claude| claude.remove(&short));
    }
  }

  /// Drop running session `sid` from project `pi` — its attach client is hung
  /// up, the background session itself is untouched — and show another
  /// session if it was on screen.
  fn remove_running_session(
    &mut self,
    pi: usize,
    sid: SessionId,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> bool {
    let on_screen = pi == self.active_project_index;
    let project = &mut self.projects[pi];
    let was_project_active = project.active_session == Some(sid);
    project.sessions.remove(&sid);

    if was_project_active && on_screen {
      // The session we were sitting on: it needs a replacement view. Note
      // that `active_session` is deliberately left pointing at the id just
      // removed — it now resolves to `None`, so `switch_to_session` sees no
      // outgoing session and skips the save-viewport step. Repointing it
      // *first* would make the incoming session look like the outgoing one
      // and overwrite its saved layout, TODO cursor and scroll with the
      // removed session's, and clear its activity marker.
      self.detach_active_session(pi, window, cx);
      return true;
    } else if was_project_active {
      // Same, but for a project we are not looking at. Repoint it so it still
      // has a session to show next time, and do NOT switch: removing a
      // background session must not yank the user into another project.
      project.active_session = project.first_session();
    }
    cx.notify();
    false
  }

  /// Collect TodoDocument references from each project's todo_view.
  fn todo_documents<'a>(&'a self, cx: &'a App) -> Vec<&'a TodoDocument> {
    self.projects.iter().map(|p| p.todo_view.read(cx).document()).collect()
  }

  // ---------------------------------------------------------------------------
  // Session disable toggle
  // ---------------------------------------------------------------------------

  /// Toggle the `[D]` marker on the heading `key` names. Disabling a running
  /// session detaches its pane and stops its background session (`claude
  /// stop`; its conversation is kept, and adopting it again resumes it). Never
  /// addressed by label: with two same-labelled headings a label-keyed toggle
  /// marks the wrong one and stops a live session (see `SessionKey`).
  fn toggle_session_disabled(
    &mut self,
    project_idx: usize,
    key: &SessionKey,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let project = &mut self.projects[project_idx];
    let todo_view = project.todo_view.clone();

    // Check if the session is currently adopted (running). An unbound heading
    // never is, so this is `None` for an `Unbound` key.
    let adopted = match key {
      SessionKey::Id(id) => project.session_by_key(id).map(|(sid, s)| (sid, s.bg.clone())),
      SessionKey::Unbound { .. } => None,
    };
    // A start or `> dangerous` change in flight must finish binding the
    // heading to the job it starts; disabling now would orphan that job.
    if let Some((sid, _)) = &adopted
      && project.sessions.get(sid).is_some_and(|s| s.changing)
    {
      crate::notify::beep();
      self.panes[self.active_pane_index].read(cx).focus_content(window);
      return;
    }

    todo_view.update(cx, |tv, cx| {
      tv.toggle_session_disabled(key, window, cx);
      tv.save(cx);
    });

    let is_now_disabled =
      todo_view.read(cx).document().session_of(key).is_some_and(TodoSession::is_disabled);
    // Whether control ended up in `switch_to_session`, which focuses the pane it
    // lands on. When it doesn't, nothing else will: the picker's confirm handler
    // has already dropped `pre_picker_focus`, so focus would be stranded on the
    // dismissed picker input and keystrokes would go nowhere.
    let mut switched = false;

    if is_now_disabled && let Some((sid, short)) = adopted {
      // Detach before stopping (see `SessionState::attach`).
      switched = self.remove_running_session(project_idx, sid, window, cx);
      if !short.is_empty() {
        self.detached(move |claude| {
          let _ = claude.stop(&short).inspect_err(|e| eprintln!("[D] {short}: {e:#}"));
        });
      }
    }

    if !switched {
      // Disabling a background session, or re-enabling one: the view didn't
      // move, so put focus back where it was before the picker opened.
      self.panes[self.active_pane_index].read(cx).focus_content(window);
    }
    cx.notify();
  }

  /// Pick a new session to show after the active one was detached, preferring
  /// another session in the same project and falling back to another project.
  fn detach_active_session(
    &mut self,
    project_idx: usize,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if let Some(next) = self.projects[project_idx].first_session() {
      // Another session in the same project — switch to it.
      self.switch_to_session(project_idx, Some(next), window, cx);
      return;
    }
    // Last session in this project is gone — jump to the next project that has
    // sessions, falling back to staying put with nothing selected.
    let next =
      self.projects.iter().enumerate().find(|(pi, p)| *pi != project_idx && !p.sessions.is_empty());
    match next {
      Some((pi, p)) => {
        let sid = p.active_session;
        self.switch_to_session(pi, sid, window, cx);
      }
      None => self.switch_to_session(project_idx, None, window, cx),
    }
  }

  // ---------------------------------------------------------------------------
  // Save file
  // ---------------------------------------------------------------------------

  fn save_file(&mut self, _: &SaveFile, window: &mut Window, cx: &mut Context<Self>) {
    self.sync_active_pane_to_focus(window, cx);
    let pane = self.active_pane_entity().clone();
    let kind = pane.read(cx).content_kind();
    let project = &self.projects[self.active_project_index];

    match kind {
      Some(PaneContentKind::CodeViewer) => {
        if let Some(cv) = project.code_view() {
          cv.update(cx, |v, cx| v.save(cx));
        }
      }
      Some(PaneContentKind::TodoEditor) => {
        project.todo_view.update(cx, |v, cx| v.save(cx));
      }
      Some(PaneContentKind::GlobalTodo) => {
        self.global_todo_view.update(cx, |v, cx| v.save(cx));
      }
      _ => {}
    }
  }

  // ---------------------------------------------------------------------------
  // Send to terminal
  // ---------------------------------------------------------------------------

  fn send_to_terminal(&mut self, _: &SendToTerminal, window: &mut Window, cx: &mut Context<Self>) {
    // Only send when the TODO editor is focused. Resync the cache from real
    // focus first so a stale index can't make this silently no-op.
    self.sync_active_pane_to_focus(window, cx);
    let active_kind = self.panes[self.active_pane_index].read(cx).content_kind();
    if active_kind != Some(PaneContentKind::TodoEditor) {
      return;
    }

    let project = &self.projects[self.active_project_index];
    let Some(session) = project.active_session() else {
      return;
    };
    let uuid = session.key.clone();
    let claude_terminal = session.claude_terminal.clone();
    let todo_view = project.todo_view.clone();
    let ready = Self::ready_for_input(session, cx);

    // Block all sends while a scheduled message is pending on this session
    // (including one waiting for the session to come up, below) — a second
    // queued send to the same Claude is undefined. Beep like a rejected
    // keystroke and no-op; the user cancels a schedule by editing out the
    // `@jc(...)` marker. A session still starting has no heading to send from.
    if uuid.is_empty() || todo_view.read(cx).has_pending_schedule(&uuid) {
      crate::notify::beep();
      return;
    }

    // Insert a WAIT section if the session doesn't have one.
    todo_view.update(cx, |tv, cx| {
      tv.ensure_wait(&uuid, window, cx);
    });

    let Some((message_text, message_index, schedule)) =
      todo_view.update(cx, |tv, cx| tv.send_selection(&uuid, window, cx))
    else {
      return;
    };

    // Re-run ensure_wait so the empty WAIT body gets a blank line for typing.
    todo_view.update(cx, |tv, cx| {
      tv.ensure_wait(&uuid, window, cx);
    });

    // Scroll to the WAIT section so the user sees their new typing area.
    if let Some(wait_line) = todo_view.read(cx).wait_line(&uuid, cx) {
      todo_view.update(cx, |tv, cx| tv.scroll_to_line(wait_line, window, cx));
    }

    if let Some(when) = schedule {
      // Deferred send: arm a timer. Delivery and `> last=` happen at
      // fire time (see `fire_scheduled_send`), not now. No workspace state
      // changed here (the TodoView mutated itself and notified), so we don't.
      let project_path = self.projects[self.active_project_index].path.clone();
      self.ensure_scheduled_armed(project_path, uuid, when, window, cx);
    } else if !ready {
      // Typing now would land in an attach client still connecting to a
      // session that is resuming. Record the message as a past-due scheduled
      // send instead: the file then says it is pending, not sent, it survives
      // a jc quit, and `reconcile_schedules` delivers it once the session is
      // up.
      let now = chrono::Local::now().naive_local();
      todo_view.update(cx, |tv, cx| tv.defer_message(&uuid, message_index, now, window, cx));
      if let Some(session) = self.projects[self.active_project_index].active_session() {
        session.status("the message is sent once the session is up", cx);
      }
    } else {
      Self::deliver_to_terminal(&claude_terminal, &message_text, cx);
      cx.notify();
    }
  }

  /// Text typed into `session`'s Claude pane reaches the session: the session
  /// process runs, and the attach client has drawn its first frame since it
  /// (re)started.
  fn ready_for_input(session: &SessionState, cx: &App) -> bool {
    session.live && session.claude_terminal.read(cx).has_output_since_spawn()
  }

  /// Paste `message_text` into the Claude terminal, then submit with a delayed
  /// Enter so the app has time to process the pasted content.
  fn deliver_to_terminal(claude_terminal: &Entity<TerminalView>, message_text: &str, cx: &App) {
    claude_terminal.read(cx).write_text(message_text);
    let Some(pty) = claude_terminal.read(cx).pty_handle() else { return };
    std::thread::spawn(move || {
      std::thread::sleep(StdDuration::from_millis(200));
      let _ = pty.write_all(b"\r");
    });
  }

  /// Arm a timer to deliver a scheduled message at `when`; a past-due one
  /// fires at once. Only sessions ready for input are armed
  /// ([`Self::reconcile_schedules`]), so a catch-up send at startup waits for
  /// the session to come up rather than for a fixed grace. The timer re-reads
  /// the live TODO at fire time, so a cancelled or rescheduled marker is
  /// handled there rather than tracked here.
  fn arm_scheduled_send(
    &mut self,
    project_path: PathBuf,
    uuid: String,
    when: NaiveDateTime,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let now = chrono::Local::now().naive_local();
    let delay = (when - now).to_std().unwrap_or(StdDuration::ZERO);
    cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| {
      Timer::after(delay).await;
      let _ = this.update_in(cx, |ws, window, cx| {
        ws.fire_scheduled_send(&project_path, &uuid, when, window, cx);
      });
    })
    .detach();
  }

  /// Deliver (or re-arm / drop) a scheduled send when its timer fires. `when` is
  /// the instant this timer was armed for.
  fn fire_scheduled_send(
    &mut self,
    project_path: &Path,
    uuid: &str,
    when: NaiveDateTime,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    // Release the dedup slot up front, on EVERY exit path. `armed_schedules` is
    // the only thing stopping `reconcile_schedules` from re-arming this marker,
    // so bailing out below while still holding the slot would strand the send
    // for the rest of the run — e.g. a session disabled before its `@jc(...)`
    // time, then re-adopted after it. Releasing here instead means an
    // undeliverable marker is simply retried on a later tick, and a delivered
    // one is gone from the file so there is nothing left to re-arm.
    self.armed_schedules.remove(&(project_path.to_path_buf(), uuid.to_string(), when));

    let Some(project_idx) = self.projects.iter().position(|p| p.path == project_path) else {
      return;
    };

    // The target session must still be up to receive the message. Resolve its
    // terminal BEFORE `deliver_scheduled` consumes the marker, so a message is
    // never marked delivered (and lost) when there's nowhere to send it — the
    // marker stays pending and re-arms on a later tick.
    let Some(claude_terminal) = self.projects[project_idx]
      .session_by_key(uuid)
      .filter(|(_, s)| Self::ready_for_input(s, cx))
      .map(|(_, s)| s.claude_terminal.clone())
    else {
      return;
    };

    let todo_view = self.projects[project_idx].todo_view.clone();
    let now = chrono::Local::now().naive_local();

    match todo_view.update(cx, |tv, cx| tv.deliver_scheduled(uuid, now, window, cx)) {
      ScheduledFire::Deliver(body) => {
        Self::deliver_to_terminal(&claude_terminal, &body, cx);
        cx.notify();
      }
      ScheduledFire::Reschedule(new_when) => {
        self.ensure_scheduled_armed(
          project_path.to_path_buf(),
          uuid.to_string(),
          new_when,
          window,
          cx,
        );
      }
      ScheduledFire::Cancelled => {}
    }
  }

  /// Arm a timer for a `(path, uuid, when)` scheduled send unless one is already
  /// armed for that exact tuple. Editing a marker's time yields a new `when`, so
  /// the edited instant gets its own timer (and the stale one harmlessly no-ops
  /// at fire, since `deliver_scheduled` re-reads the live marker).
  fn ensure_scheduled_armed(
    &mut self,
    project_path: PathBuf,
    uuid: String,
    when: NaiveDateTime,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.armed_schedules.insert((project_path.clone(), uuid.clone(), when)) {
      self.arm_scheduled_send(project_path, uuid, when, window, cx);
    }
  }

  /// Arm timers for every scheduled marker currently pending in the live TODO
  /// documents (idempotent via `armed_schedules`). Runs at startup and on a
  /// short interval, so adding/editing/removing a `@jc(...)` time takes effect —
  /// including edits to an *earlier* time, which the fire-time re-check alone
  /// can't catch. Restricted to headings whose session is actually running (see
  /// the filter below).
  fn reconcile_schedules(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let pending: Vec<(PathBuf, String, NaiveDateTime)> = self
      .projects
      .iter()
      .flat_map(|project| {
        let path = project.path.clone();
        project
          .todo_view
          .read(cx)
          .document()
          .sessions
          .iter()
          // Only headings whose session is up: delivery needs a terminal that
          // reaches it. A marker on a dormant, still-starting or still-unbound
          // heading is not dropped — it stays in the file with its pending
          // highlight, and is armed on the first tick after the session comes
          // up (firing immediately if its time has already passed).
          .filter(|s| {
            project.session_by_key(&s.id).is_some_and(|(_, r)| Self::ready_for_input(r, cx))
          })
          .filter_map(move |s| {
            s.pending_scheduled()
              .and_then(|m| m.schedule.map(|dt| (path.clone(), s.id.clone(), dt)))
          })
      })
      .collect();
    for (path, uuid, when) in pending {
      self.ensure_scheduled_armed(path, uuid, when, window, cx);
    }
  }

  /// Spawn the periodic task that reconciles jc's state against the live TODO
  /// documents: scheduled-send timers (so time edits are picked up without
  /// waiting for the original, possibly later, timer to fire), background
  /// sessions ([`Self::reconcile_background`]: `> dangerous` changes,
  /// `[DELETED]` headings, waiting sends), and each running session's label
  /// (so a heading renamed in TODO.md takes effect). The main thread does no
  /// I/O here; `claude` commands run on their own threads.
  fn start_schedule_reconcile_loop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let task = cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| {
      loop {
        Timer::after(StdDuration::from_secs(2)).await;
        let stepped = this.update_in(cx, |ws, window, cx| {
          ws.reconcile_schedules(window, cx);
          ws.reconcile_background(window, cx);
          let mut changed = false;
          for i in 0..ws.projects.len() {
            changed |= ws.projects[i].sync_sessions_from_todo(cx);
          }
          if changed {
            // Only the label moved: `sync_sessions_from_todo` never rewrites a
            // session's id, and the TODO view highlights by id, so there is
            // nothing to re-point — just redraw the label wherever it shows.
            cx.notify();
          }
        });
        if stepped.is_err() {
          break; // workspace dropped
        }
      }
    });
    self._schedule_reconcile_task = Some(task);
  }

  // ---------------------------------------------------------------------------
  // Jump to WAIT
  // ---------------------------------------------------------------------------

  fn jump_to_wait(&mut self, _: &JumpToWait, window: &mut Window, cx: &mut Context<Self>) {
    let project = &self.projects[self.active_project_index];
    let Some(uuid) = project.active_key().map(str::to_string) else {
      return;
    };
    let todo_view = project.todo_view.clone();

    // Insert a WAIT section if the session doesn't have one.
    todo_view.update(cx, |tv, cx| {
      tv.ensure_wait(&uuid, window, cx);
    });

    let Some(wait_line) = todo_view.read(cx).wait_line(&uuid, cx) else {
      return;
    };

    // If a visible pane already shows the TODO editor, focus it instead of
    // replacing the current pane.
    let visible = self.visible_pane_count();
    let existing = (0..visible)
      .find(|&i| self.panes[i].read(cx).content_kind() == Some(PaneContentKind::TodoEditor));
    if let Some(idx) = existing {
      self.active_pane_index = idx;
      self.panes[idx].read(cx).focus_content(window);
    } else {
      self.set_active_pane_view(PaneContentKind::TodoEditor, window, cx);
    }
    todo_view.update(cx, |tv, cx| tv.scroll_to_line(wait_line, window, cx));
    cx.notify();
  }

  // ---------------------------------------------------------------------------
  // Hook events
  // ---------------------------------------------------------------------------

  fn handle_hook_event(&mut self, event: HookEvent, window: &mut Window, cx: &mut Context<Self>) {
    eprintln!("hook: {:?} session={}", event.kind, event.session_id);

    // Handle session clear: follow the session onto its new uuid.
    if let HookEventKind::SessionClear { ref old_session_id, ref new_session_id } = event.kind {
      self.handle_session_clear(old_session_id, new_session_id);
      cx.notify();
      return;
    }

    let Some((project_name, session_label)) = self.apply_hook_to_session(&event) else {
      // From a project jc has open but no session it knows: most likely one of
      // its own whose uuid changed while jc was not looking (a `/clear` in
      // another attach). Ask `claude agents --json`, then try again.
      if event.project_path.as_ref().is_some_and(|p| self.project_index(p).is_some()) {
        if self.unmatched_hooks.len() == UNMATCHED_HOOKS_MAX {
          self.unmatched_hooks.pop_front();
        }
        self.unmatched_hooks.push_back(event);
        self.refresh_session_uuids(window, cx);
      }
      return;
    };
    // Notify when the window is not active (user is in another app). Only a
    // blocked session is worth interrupting for; Stop/IdlePrompt are ambient.
    let message = match event.kind {
      HookEventKind::PermissionPrompt => Some("Permission needed"),
      HookEventKind::StopFailure => Some("API error"),
      _ => None,
    };
    if let Some(message) = message
      && !self.window_active
    {
      let title = format!("{project_name} > {session_label}");
      crate::notify::notify(&title, message, &event.session_id);
    }

    cx.notify();
  }

  /// Apply a hook event to the session it names (see
  /// [`background::hook_matches`]): the session is live, and the uuid is
  /// learned. Returns the matched `(project name, session label)`, or `None` when no
  /// session owns it.
  fn apply_hook_to_session(&mut self, event: &HookEvent) -> Option<(String, String)> {
    let (pi, sid) = self.session_for_hook(&event.session_id)?;
    let project = &mut self.projects[pi];
    let project_name = project.name.clone();
    let session = project.sessions.get_mut(&sid)?;

    session.uuid = event.session_id.clone();
    session.live = true;
    session.note_hook(&event.kind, SystemTime::now());
    Some((project_name, session.label.clone()))
  }

  /// Apply a `cira status --json` read (`None`: Cira is unreachable) to every
  /// project. Returns whether anything shown changed.
  fn apply_cira(&mut self, status: Option<&jc_core::cira::Status>) -> bool {
    if self.cira_reachable != Some(status.is_some()) {
      self.cira_reachable = Some(status.is_some());
      eprintln!("cira: {}", if status.is_some() { "reachable" } else { "unreachable" });
    }
    let mut changed = false;
    for project in &mut self.projects {
      if project.apply_cira(status) {
        changed = true;
        let sessions: Vec<String> = project
          .sessions
          .values()
          .map(|s| format!("{}={:?}", s.label, s.cira_state.map(|(state, _)| state)))
          .collect();
        eprintln!("cira: {} open={:?} {}", project.name, project.cira_open, sessions.join(" "));
      }
    }
    changed
  }

  /// The `(project index, session)` a hook naming `hook_session` comes from.
  fn session_for_hook(&self, hook_session: &str) -> Option<(usize, SessionId)> {
    self.projects.iter().enumerate().find_map(|(pi, p)| {
      p.sessions.iter().find(|(_, s)| s.owns_hook(hook_session)).map(|(&sid, _)| (pi, sid))
    })
  }

  /// Ask `claude agents --json` for the current uuid of every running session
  /// jc has, then re-dispatch the hooks that matched none.
  fn refresh_session_uuids(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.refreshing_agents
      || self.last_agents_refresh.is_some_and(|t| t.elapsed() < AGENTS_REFRESH_GAP)
    {
      return;
    }
    self.refreshing_agents = true;
    self.last_agents_refresh = Some(Instant::now());
    let claude = self.claude.clone();
    self.run_blocking(
      window,
      cx,
      move || claude.agents(),
      |ws, agents, window, cx| {
        ws.refreshing_agents = false;
        let agents = match agents {
          Ok(agents) => agents,
          Err(e) => {
            eprintln!("claude agents: {e:#}");
            return;
          }
        };
        for session in ws.projects.iter_mut().flat_map(|p| p.sessions.values_mut()) {
          if let Some(uuid) = background::find(&agents, &session.bg)
            .filter(|a| a.is_live())
            .and_then(|a| a.session_id.clone())
          {
            session.uuid = uuid;
          }
        }
        // Each is tried once more; one still unmatched is not jc's.
        for event in std::mem::take(&mut ws.unmatched_hooks) {
          if ws.session_for_hook(&event.session_id).is_some() {
            ws.handle_hook_event(event, window, cx);
          }
        }
      },
    );
  }

  /// Handle a `/clear` event: the session's conversation moved to a new uuid
  /// in the same Claude process. Only the in-memory uuid follows it: TODO.md
  /// names the background session by its short id, which a `/clear` keeps.
  fn handle_session_clear(&mut self, old_session_id: &str, new_session_id: &str) {
    let session = self
      .projects
      .iter_mut()
      .flat_map(|p| p.sessions.values_mut())
      .find(|s| s.owns_hook(old_session_id));
    match session {
      Some(session) => {
        eprintln!(
          "hook: session {} cleared, uuid {old_session_id} -> {new_session_id}",
          session.bg
        );
        session.uuid = new_session_id.to_string();
      }
      None => eprintln!("hook: session-clear for unknown uuid {old_session_id}"),
    }
  }
}

impl Drop for Workspace {
  fn drop(&mut self) {
    for project in &self.projects {
      let _ = jc_core::hooks_settings::uninstall_hooks(&project.path);
    }
    if let Some(server) = &self._hook_server {
      server.shutdown();
    }
  }
}

impl Focusable for Workspace {
  fn focus_handle(&self, _: &App) -> FocusHandle {
    self.focus.clone()
  }
}

pub fn init(cx: &mut App) {
  cx.bind_keys([
    KeyBinding::new("cmd-w", CloseWindow, Some("Workspace")),
    KeyBinding::new("cmd-m", MinimizeWindow, Some("Workspace")),
    KeyBinding::new("cmd-q", Quit, Some("Workspace")),
    KeyBinding::new("cmd-[", FocusPrevPane, Some("Workspace")),
    KeyBinding::new("cmd-]", FocusNextPane, Some("Workspace")),
    KeyBinding::new("cmd-1", SetLayoutOne, Some("Workspace")),
    KeyBinding::new("cmd-2", SetLayoutTwo, Some("Workspace")),
    KeyBinding::new("cmd-3", SetLayoutThree, Some("Workspace")),
    KeyBinding::new("cmd-p", crate::views::picker::ShowSessionPicker, Some("Workspace")),
    KeyBinding::new("cmd-s", SaveFile, Some("Workspace")),
    KeyBinding::new("cmd-enter", SendToTerminal, Some("Workspace")),
    KeyBinding::new("cmd-.", JumpToWait, Some("Workspace")),
    KeyBinding::new("cmd-shift-p", crate::views::picker::ProjectActionsPicker, Some("Workspace")),
    KeyBinding::new("cmd-`", RotateNextProject, Some("Workspace")),
    KeyBinding::new("cmd-?", ShowKeybindingHelp, Some("Workspace")),
    KeyBinding::new("cmd-alt-up", ScrollOtherUp, Some("Workspace")),
    KeyBinding::new("cmd-alt-down", ScrollOtherDown, Some("Workspace")),
    KeyBinding::new("cmd-alt-pageup", ScrollOtherPageUp, Some("Workspace")),
    KeyBinding::new("cmd-alt-pagedown", ScrollOtherPageDown, Some("Workspace")),
  ]);

  cx.bind_keys([
    KeyBinding::new("cmd-[", FocusPrevPane, Some("Input")),
    KeyBinding::new("cmd-]", FocusNextPane, Some("Input")),
    KeyBinding::new("cmd-s", SaveFile, Some("Input")),
    KeyBinding::new("cmd-enter", SendToTerminal, Some("Input")),
    KeyBinding::new("cmd-.", JumpToWait, Some("Input")),
    KeyBinding::new("cmd-`", RotateNextProject, Some("Input")),
    KeyBinding::new("cmd-alt-up", ScrollOtherUp, Some("Input")),
    KeyBinding::new("cmd-alt-down", ScrollOtherDown, Some("Input")),
    KeyBinding::new("cmd-alt-pageup", ScrollOtherPageUp, Some("Input")),
    KeyBinding::new("cmd-alt-pagedown", ScrollOtherPageDown, Some("Input")),
  ]);
}
