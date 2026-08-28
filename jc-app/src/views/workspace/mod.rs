mod pickers;
mod render;

use crate::views::close_confirm::{CloseConfirm, CloseConfirmEvent};
use crate::views::keybinding_help::{DismissHelpEvent, KeybindingHelp};
use crate::views::pane::{Pane, PaneContent, PaneContentKind};
use crate::views::project_state::{ProjectState, SavedPaneLayout};
use crate::views::session_state::{
  ActivityBaseline, Launch, SavedViewState, SessionId, SessionState,
};
use crate::views::todo_view::ScheduledFire;
use chrono::NaiveDateTime;
use gpui::*;
use gpui_component::input::InputState;
use gpui_component::theme::Theme;
use jc_core::config::{AppConfig, AppState};
use jc_core::hooks::{HookEvent, HookEventKind, HookServer};
use jc_core::theme::Appearance;
use jc_terminal::{Palette, TerminalView};
use std::ops::DerefMut;
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;

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
  /// (project path, session label, delivery time) tuples that already have a
  /// timer armed, so reconciliation doesn't double-arm the same scheduled send.
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
}

impl Workspace {
  pub fn new(
    state: AppState,
    config: AppConfig,
    ipc_rx: flume::Receiver<PathBuf>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let palette = palette_from_window(window);

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
    };

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
  /// are active sessions or buffers with merge conflicts that can't be saved.
  fn try_close(&mut self, is_quit: bool, window: &mut Window, cx: &mut Context<Self>) {
    let conflicts = self.save_all_dirty(cx);
    let active = self.active_session_count();

    if conflicts.is_empty() && active == 0 {
      // Nothing to warn about — just close.
      if is_quit {
        cx.quit();
      } else {
        window.remove_window();
      }
    } else {
      self.show_close_confirm(active, conflicts, is_quit, window, cx);
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

  /// Count sessions that are actively working (not idle/stopped).
  fn active_session_count(&self) -> usize {
    self.projects.iter().flat_map(|p| p.sessions.values()).filter(|s| s.busy).count()
  }

  fn show_close_confirm(
    &mut self,
    session_count: usize,
    conflicts: Vec<String>,
    is_quit: bool,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.close_confirm_is_quit = is_quit;
    self.pre_close_confirm_focus = window.focused(cx);
    let view = cx.new(|cx| CloseConfirm::new(session_count, conflicts, is_quit, cx));
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
      if let Some(label) = project.active_label() {
        let todo_view = project.todo_view.clone();
        todo_view.update(cx, |tv, cx| tv.ensure_wait(label, window, cx));
        let project = &self.projects[self.active_project_index];
        let tv = project.todo_view.read(cx);
        let text = tv.editor_text(cx);
        if let Some(wait_line) = tv.document().wait_body_end_line(label, &text) {
          let wait_line_0 = wait_line.saturating_sub(1);
          let _ = tv;
          project.todo_view.update(cx, |tv, cx| tv.scroll_to_line(wait_line_0, window, cx));
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
      // Everything this session printed while it was on screen has been seen,
      // so it must not come back marked in the Cmd-P picker. Clearing on the way
      // OUT rather than on the way in is what makes that true regardless of how
      // the switch was made (picker, Cmd-`, a notification click).
      session.claude_terminal.read(cx).clear_output_seen();
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
    let label = self.projects[project_idx].active_label().map(|s| s.to_string());
    let todo_view = self.projects[project_idx].todo_view.clone();
    todo_view.update(cx, |tv, cx| tv.set_active_label(label.as_deref(), cx));

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

  /// Switch to the session a clicked notification names. Notifications always
  /// carry the session UUID.
  fn switch_to_session_id(
    &mut self,
    session_id: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let found = self
      .projects
      .iter()
      .enumerate()
      .find_map(|(pi, p)| p.session_by_uuid(session_id).map(|(id, _)| (pi, id)));
    if let Some((pi, id)) = found {
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
  // Session creation
  // ---------------------------------------------------------------------------

  /// Launch a brand new Claude session under a UUID jc mints itself, so the
  /// session is identified from the first instant rather than being detected
  /// after the fact.
  fn create_new_session(
    &mut self,
    project_idx: usize,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let project_path = self.projects[project_idx].path.clone();
    let palette = palette_from_window(window);

    let project = &mut self.projects[project_idx];
    let id = project.next_session_id;
    project.next_session_id += 1;

    // Uniquify — see `jc_core::todo::SessionKey` for why a duplicate label
    // silently misroutes a send.
    let label = jc_core::todo::unique_label(project.todo_view.read(cx).document(), "New Session");
    let uuid = uuid::Uuid::new_v4().to_string();

    let session = SessionState::create(
      id,
      uuid.clone(),
      label.clone(),
      &project_path,
      &palette,
      false,
      Launch::New,
      window,
      cx,
    );

    project.sessions.insert(id, session);

    let todo_view = project.todo_view.clone();
    todo_view.update(cx, |tv, cx| {
      tv.insert_session_heading(&uuid, &label, window, cx);
      tv.save(cx);
    });

    self.switch_to_session(project_idx, Some(id), window, cx);
  }

  /// Adopt a TODO.md session that isn't running yet. The launch flag comes from
  /// whether the transcript is still on disk (`ProjectState::launch_for`), so a
  /// session Claude has garbage-collected comes back as a fresh conversation
  /// under its own UUID instead of a dead pane.
  ///
  /// An empty `uuid` is a heading an older jc left unbound. One is minted and
  /// written here rather than at startup, so binding a legacy heading to a new
  /// (empty) conversation is something the user chose to do.
  fn adopt_session(
    &mut self,
    project_idx: usize,
    uuid: &str,
    label: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let todo_view = self.projects[project_idx].todo_view.clone();

    // Bind an unbound heading now, in one write. A minted UUID has no
    // conversation *by construction*, so it is `Launch::New` outright — asking
    // `launch_for` would get `Resume` when the filesystem can't be seen at all
    // (`$HOME` unset) and spawn `claude --resume <fresh-uuid>` into a dead pane,
    // permanently, since the heading is bound by then.
    let (uuid, minted_launch) = if uuid.is_empty() {
      let minted = uuid::Uuid::new_v4().to_string();
      let key = jc_core::todo::SessionKey::Label(label.to_string());
      let written = todo_view.update(cx, |tv, cx| {
        let Some(index) = tv.document().index_of(&key) else { return false };
        tv.update_session_uuid_at(index, &minted, window, cx);
        true
      });
      // Refuse to launch a session whose UUID isn't in the file. It could never
      // be label-synced or disabled afterwards (both key on the UUID), and the
      // still-unbound heading would stay adoptable — so a second press would
      // spawn a second process for the same heading.
      if !written {
        eprintln!("adopt: no TODO heading labelled {label:?}; not launching");
        // The picker already dropped `pre_picker_focus` on the assumption that
        // this call would focus something. It won't, so do it here or the next
        // keystroke goes nowhere.
        self.panes[self.active_pane_index].read(cx).focus_content(window);
        return;
      }
      (minted, Some(Launch::New))
    } else {
      (uuid.to_string(), None)
    };
    let uuid = &uuid;

    // Clear `[D]` if the heading was dormant, then save once for both edits —
    // each save is a full-document copy and a blocking write on the main thread.
    let key = jc_core::todo::SessionKey::Uuid(uuid.clone());
    let is_disabled = todo_view
      .read(cx)
      .document()
      .session_of(&key)
      .is_some_and(|s| s.status == jc_core::todo::SessionStatus::Disabled);
    todo_view.update(cx, |tv, cx| {
      if is_disabled {
        tv.toggle_session_disabled(&key, window, cx);
      }
      tv.save(cx);
    });

    let project_path = self.projects[project_idx].path.clone();
    let palette = palette_from_window(window);

    let project = &mut self.projects[project_idx];
    let dangerous =
      project.todo_view.read(cx).document().session_by_uuid(uuid).is_some_and(|s| s.dangerous);
    let launch = minted_launch.unwrap_or_else(|| project.launch_for(uuid));
    let id = project.next_session_id;
    project.next_session_id += 1;

    let session = SessionState::create(
      id,
      uuid.to_string(),
      label.to_string(),
      &project_path,
      &palette,
      dangerous,
      launch,
      window,
      cx,
    );

    project.sessions.insert(id, session);
    self.switch_to_session(project_idx, Some(id), window, cx);
  }

  /// Collect TodoDocument references from each project's todo_view.
  fn todo_documents<'a>(&'a self, cx: &'a App) -> Vec<&'a jc_core::todo::TodoDocument> {
    self.projects.iter().map(|p| p.todo_view.read(cx).document()).collect()
  }

  // ---------------------------------------------------------------------------
  // Session disable toggle
  // ---------------------------------------------------------------------------

  /// Toggle the `[D]` marker on the session owning `uuid`, detaching its
  /// terminal if it is running and just became disabled. Addressed by UUID
  /// throughout: with two same-labelled headings a label-keyed toggle marks the
  /// wrong one and detaches a live session.
  fn toggle_session_disabled(
    &mut self,
    project_idx: usize,
    key: &jc_core::todo::SessionKey,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let project = &mut self.projects[project_idx];
    let todo_view = project.todo_view.clone();

    // Check if the session is currently adopted (running). An unbound heading
    // never is, so this is `None` for a `Label` key.
    let adopted_id = match key {
      jc_core::todo::SessionKey::Uuid(uuid) => project.session_by_uuid(uuid).map(|(id, _)| id),
      jc_core::todo::SessionKey::Label(_) => None,
    };

    todo_view.update(cx, |tv, cx| {
      tv.toggle_session_disabled(key, window, cx);
      tv.save(cx);
    });

    // If the session was adopted and is now being disabled, detach it.
    let is_now_disabled = todo_view
      .read(cx)
      .document()
      .session_of(key)
      .is_some_and(|s| s.status == jc_core::todo::SessionStatus::Disabled);
    // Whether control ended up in `switch_to_session`, which focuses the pane it
    // lands on. When it doesn't, nothing else will: the picker's confirm handler
    // has already dropped `pre_picker_focus`, so focus would be stranded on the
    // dismissed picker input and keystrokes would go nowhere.
    let mut switched = false;

    if is_now_disabled && let Some(id) = adopted_id {
      let on_screen = project_idx == self.active_project_index;
      let project = &mut self.projects[project_idx];
      let was_project_active = project.active_session == Some(id);
      project.sessions.remove(&id);

      if was_project_active && on_screen {
        // The session we were sitting on: it needs a replacement view. Note
        // that `active_session` is deliberately left pointing at the id just
        // removed — it now resolves to `None`, so `switch_to_session` sees no
        // outgoing session and skips the save-viewport step. Repointing it
        // *first* would make the incoming session look like the outgoing one
        // and overwrite its saved layout, TODO cursor and scroll with the
        // disabled session's, and clear its activity marker.
        switched = true;
        self.detach_active_session(project_idx, window, cx);
      } else if was_project_active {
        // Same, but for a project we are not looking at. Repoint it so it still
        // has a session to show next time, and do NOT switch: disabling a
        // background session must not yank the user into another project.
        project.active_session = project.first_session();
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
    let Some(label) = project.active_label().map(str::to_string) else {
      return;
    };
    let Some(session) = project.active_session() else {
      return;
    };
    let claude_terminal = session.claude_terminal.clone();
    let todo_view = project.todo_view.clone();

    // Block all sends while a scheduled message is pending on this session — a
    // second queued send to the same Claude is undefined. Beep like a rejected
    // keystroke and no-op; the user cancels by editing out the `@jc(...)` marker.
    if todo_view.read(cx).has_pending_schedule(&label) {
      crate::notify::beep();
      return;
    }

    // Insert a WAIT section if the session doesn't have one.
    todo_view.update(cx, |tv, cx| {
      tv.ensure_wait(&label, window, cx);
    });

    let Some((message_text, schedule)) =
      todo_view.update(cx, |tv, cx| tv.send_selection(&label, window, cx))
    else {
      return;
    };

    // Re-run ensure_wait so the empty WAIT body gets a blank line for typing.
    todo_view.update(cx, |tv, cx| {
      tv.ensure_wait(&label, window, cx);
    });

    // Scroll to the WAIT section so the user sees their new typing area.
    if let Some(wait_line) = todo_view.read(cx).wait_line(&label, cx) {
      todo_view.update(cx, |tv, cx| tv.scroll_to_line(wait_line, window, cx));
    }

    if let Some(when) = schedule {
      // Deferred send: arm a timer. Delivery, busy, and `> last=` happen at
      // fire time (see `fire_scheduled_send`), not now. No workspace state
      // changed here (the TodoView mutated itself and notified), so we don't.
      let project_path = self.projects[self.active_project_index].path.clone();
      self.ensure_scheduled_armed(project_path, label, when, window, cx);
    } else {
      // Immediate send: mark busy and deliver now.
      if let Some(session) = self.projects[self.active_project_index].active_session_mut() {
        session.mark_user_input();
      }
      Self::deliver_to_terminal(&claude_terminal, &message_text, cx);
      cx.notify();
    }
  }

  /// Paste `message_text` into the Claude terminal, then submit with a delayed
  /// Enter so the app has time to process the pasted content.
  fn deliver_to_terminal(claude_terminal: &Entity<TerminalView>, message_text: &str, cx: &App) {
    claude_terminal.read(cx).write_text(message_text);
    let pty = claude_terminal.read(cx).pty_handle();
    std::thread::spawn(move || {
      std::thread::sleep(StdDuration::from_millis(200));
      let _ = pty.write_all(b"\r");
    });
  }

  /// Arm a timer to deliver a scheduled message at `when`. A past-due target
  /// (e.g. a catch-up send re-armed at startup) fires after a short grace so a
  /// freshly-spawned Claude terminal has time to reach its prompt. The timer
  /// re-reads the live TODO at fire time, so a cancelled or rescheduled marker
  /// is handled there rather than tracked here.
  fn arm_scheduled_send(
    &mut self,
    project_path: PathBuf,
    label: String,
    when: NaiveDateTime,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    /// Grace before firing a past-due send, so a just-launched terminal is ready.
    const CATCHUP_GRACE: StdDuration = StdDuration::from_secs(5);

    let now = chrono::Local::now().naive_local();
    let raw = (when - now).to_std().unwrap_or(StdDuration::ZERO);
    let delay = if raw.is_zero() { CATCHUP_GRACE } else { raw };
    cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| {
      Timer::after(delay).await;
      let _ = this.update_in(cx, |ws, window, cx| {
        ws.fire_scheduled_send(&project_path, &label, when, window, cx);
      });
    })
    .detach();
  }

  /// Deliver (or re-arm / drop) a scheduled send when its timer fires. `when` is
  /// the instant this timer was armed for.
  fn fire_scheduled_send(
    &mut self,
    project_path: &Path,
    label: &str,
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
    self.armed_schedules.remove(&(project_path.to_path_buf(), label.to_string(), when));

    let Some(project_idx) = self.projects.iter().position(|p| p.path == project_path) else {
      return;
    };

    // The target session must still be live to receive the message. Resolve its
    // terminal BEFORE `deliver_scheduled` consumes the marker, so a message is
    // never marked delivered (and lost) when there's nowhere to send it — the
    // marker stays pending and re-arms on a later tick.
    let Some((id, claude_terminal)) = self.projects[project_idx]
      .session_by_label(label)
      .map(|(id, s)| (id, s.claude_terminal.clone()))
    else {
      return;
    };

    let todo_view = self.projects[project_idx].todo_view.clone();
    let now = chrono::Local::now().naive_local();

    match todo_view.update(cx, |tv, cx| tv.deliver_scheduled(label, now, window, cx)) {
      ScheduledFire::Deliver(body) => {
        Self::deliver_to_terminal(&claude_terminal, &body, cx);
        if let Some(session) = self.projects[project_idx].sessions.get_mut(&id) {
          session.mark_user_input();
        }
        cx.notify();
      }
      ScheduledFire::Reschedule(new_when) => {
        self.ensure_scheduled_armed(
          project_path.to_path_buf(),
          label.to_string(),
          new_when,
          window,
          cx,
        );
      }
      ScheduledFire::Cancelled => {}
    }
  }

  /// Arm a timer for a `(path, label, when)` scheduled send unless one is already
  /// armed for that exact tuple. Editing a marker's time yields a new `when`, so
  /// the edited instant gets its own timer (and the stale one harmlessly no-ops
  /// at fire, since `deliver_scheduled` re-reads the live marker).
  fn ensure_scheduled_armed(
    &mut self,
    project_path: PathBuf,
    label: String,
    when: NaiveDateTime,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.armed_schedules.insert((project_path.clone(), label.clone(), when)) {
      self.arm_scheduled_send(project_path, label, when, window, cx);
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
          // Only headings with a session actually running: delivery needs a
          // terminal. A marker on a dormant or still-unbound heading is not
          // dropped — it stays in the file with its pending highlight, and
          // adopting the session arms it on the next tick (firing immediately,
          // after the catch-up grace, if its time has already passed).
          .filter(|s| project.sessions.values().any(|r| r.label == s.label))
          .filter_map(move |s| {
            s.pending_scheduled()
              .and_then(|m| m.schedule.map(|dt| (path.clone(), s.label.clone(), dt)))
          })
      })
      .collect();
    for (path, label, when) in pending {
      self.ensure_scheduled_armed(path, label, when, window, cx);
    }
  }

  /// Discount the startup output of every session that hasn't been baselined
  /// yet, so the Cmd-P activity marker means "work happened while you were
  /// away" rather than "jc launched this".
  ///
  /// Driven from the reconcile loop rather than a one-shot startup task, and
  /// keyed per session rather than per launch, because `Workspace::open_project`
  /// can restore a whole project's sessions at any point in the run — a
  /// startup-only pass would mark every session of a later-opened project. A
  /// session is baselined exactly once and then left alone: re-clearing a
  /// settled session on later ticks would swallow the real output the marker
  /// exists to report.
  fn step_activity_baselines(&mut self, cx: &mut Context<Self>) {
    /// Consecutive quiet ticks that count as "the child has settled". More than
    /// one because `claude --resume` pauses between banner and transcript replay.
    const QUIET_TICKS: usize = 2;
    /// Stop waiting after this many ticks, so a child that never goes quiet
    /// isn't tracked for the life of the process.
    const MAX_TICKS: usize = 15;

    for project in &mut self.projects {
      for session in project.sessions.values_mut() {
        let ActivityBaseline::Pending { last_batches, quiet_ticks, ticks } =
          session.activity_baseline
        else {
          continue;
        };
        let terminal = session.claude_terminal.read(cx);
        let batches = terminal.output_batches();

        // A child that has printed nothing yet is not quiet, it is slow to
        // start; baselining now would let its banner land above the baseline.
        let quiet = batches > 0 && batches == last_batches;
        let quiet_ticks = if quiet { quiet_ticks + 1 } else { 0 };

        // Clear on both exits, including the give-up. A child that never
        // settles is still printing, so it re-marks itself on the next batch;
        // a marker left set here would never retire (only switching away
        // clears one) and `has_activity` is the picker's primary sort key, so
        // the session would outrank genuinely-changed ones forever.
        if quiet_ticks >= QUIET_TICKS || ticks + 1 >= MAX_TICKS {
          terminal.clear_output_seen();
          session.activity_baseline = ActivityBaseline::Taken;
        } else {
          session.activity_baseline =
            ActivityBaseline::Pending { last_batches: batches, quiet_ticks, ticks: ticks + 1 };
        }
      }
    }
  }

  /// Spawn the periodic task that reconciles jc's state against the live TODO
  /// documents: scheduled-send timers (so time edits are picked up without
  /// waiting for the original, possibly later, timer to fire), each running
  /// session's label (so a heading renamed in TODO.md takes effect), and the
  /// per-session activity baselines. All in-memory — no I/O.
  fn start_schedule_reconcile_loop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let task = cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| {
      loop {
        Timer::after(StdDuration::from_secs(2)).await;
        let stepped = this.update_in(cx, |ws, window, cx| {
          ws.reconcile_schedules(window, cx);
          ws.step_activity_baselines(cx);
          let mut changed = false;
          for i in 0..ws.projects.len() {
            changed |= ws.projects[i].sync_sessions_from_todo(cx);
          }
          if changed {
            // A renamed heading changes the label the TODO view highlights by,
            // so re-point it — otherwise `apply_session_highlights` matches
            // nothing and the active session's colouring silently disappears
            // until the next session switch.
            let pi = ws.active_project_index;
            let label = ws.projects[pi].active_label().map(str::to_string);
            let todo_view = ws.projects[pi].todo_view.clone();
            todo_view.update(cx, |tv, cx| tv.set_active_label(label.as_deref(), cx));
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
    let Some(label) = project.active_label().map(str::to_string) else {
      return;
    };
    let todo_view = project.todo_view.clone();

    // Insert a WAIT section if the session doesn't have one.
    todo_view.update(cx, |tv, cx| {
      tv.ensure_wait(&label, window, cx);
    });

    let Some(wait_line) = todo_view.read(cx).wait_line(&label, cx) else {
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

    // Handle session clear: update the session's UUID.
    if let HookEventKind::SessionClear { ref old_session_id, ref new_session_id } = event.kind {
      self.handle_session_clear(
        event.project_path.as_deref(),
        old_session_id,
        new_session_id,
        window,
        cx,
      );
      cx.notify();
      return;
    }

    // Every remaining event is addressed to a session identified by its UUID.
    // UUIDs are assigned by jc at launch (`--session-id`), so a hook for an
    // unknown UUID belongs to a session jc doesn't manage — ignore it.
    let Some((project_name, session_label)) = self.apply_hook_to_session(&event) else {
      cx.notify();
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

  /// Apply a hook event to the session it names. Returns the matched
  /// `(project name, session label)`, or `None` when no session owns the UUID.
  fn apply_hook_to_session(&mut self, event: &HookEvent) -> Option<(String, String)> {
    if event.session_id.is_empty() {
      return None;
    }
    let (project_name, session) = self.projects.iter_mut().find_map(|p| {
      let name = p.name.clone();
      p.sessions.values_mut().find(|s| s.uuid == event.session_id).map(|s| (name, s))
    })?;

    match event.kind {
      HookEventKind::PromptSubmit => session.mark_user_input(),
      HookEventKind::Stop
      | HookEventKind::StopFailure
      | HookEventKind::PermissionPrompt
      | HookEventKind::IdlePrompt => session.busy = false,
      HookEventKind::SessionClear { .. } => unreachable!("handled before dispatch"),
    }
    Some((project_name, session.label.clone()))
  }

  /// Handle a `/clear` event: the old session ended and a new one started in
  /// the same Claude process. Update the session's UUID to the new one.
  /// No terminal relaunch needed — `/clear` resets the conversation but the
  /// Claude process keeps running in the same terminal.
  fn handle_session_clear(
    &mut self,
    project_path: Option<&Path>,
    old_session_id: &str,
    new_session_id: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(project_path) = project_path else { return };
    let Some(project) = self.projects.iter().find(|p| p.path == *project_path) else { return };
    if !project.sessions.values().any(|s| s.uuid == old_session_id) {
      eprintln!("hook: session-clear for unknown uuid {old_session_id}");
      return;
    }

    eprintln!("hook: session cleared, uuid {old_session_id} -> {new_session_id}");

    // Update TODO.md FIRST, and only adopt the new UUID in memory if that
    // succeeded. The two must not diverge: a session whose in-memory UUID has no
    // heading stops syncing its label, can't be disabled (both look the heading
    // up by UUID), and on the next start resumes whatever the file still says —
    // a wrong conversation, not merely a dead one.
    //
    // The heading is located by the OLD uuid, not by label: two headings can
    // share a label, and writing to the first would point it at this session's
    // conversation while leaving this one behind.
    let todo_view = project.todo_view.clone();
    let old_session_id = old_session_id.to_string();
    let new_session_id = new_session_id.to_string();
    let written = todo_view.update(cx, |tv, cx| {
      let Some(index) = tv.document().index_by_uuid(&old_session_id) else { return false };
      tv.update_session_uuid_at(index, &new_session_id, window, cx);
      tv.save(cx);
      true
    });

    if written {
      if let Some(project) = self.projects.iter_mut().find(|p| p.path == *project_path)
        && let Some(session) = project.sessions.values_mut().find(|s| s.uuid == old_session_id)
      {
        session.uuid = new_session_id;
      }
    } else {
      eprintln!(
        "hook: session-clear could not find a heading for uuid {old_session_id}; \
         leaving the session on it so memory and TODO.md stay in step"
      );
    }
    cx.notify();
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
