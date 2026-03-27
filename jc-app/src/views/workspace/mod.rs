mod pickers;
mod problems;
mod render;

use crate::views::close_confirm::{CloseConfirm, CloseConfirmEvent};
use crate::views::diff_view::DiffViewEvent;
use crate::views::keybinding_help::{DismissHelpEvent, KeybindingHelp};
use crate::views::pane::{Pane, PaneContent, PaneContentKind};
use crate::views::project_state::{ProjectState, SavedPaneLayout};
use crate::views::session_state::{PendingEvent, SessionId, SessionState};
use gpui::*;
use gpui_component::input::InputState;
use gpui_component::theme::Theme;
use jc_core::config::{AppConfig, AppState};
use jc_core::hooks::{HookEvent, HookEventKind, HookServer};
use jc_core::snippets::{self, SnippetDocument};
use jc_core::theme::Appearance;
use jc_terminal::{Palette, TerminalView, TerminalViewEvent};
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
    ShowGitDiff,
    ShowCodeViewer,
    ShowTodoEditor,
    OpenInExternalEditor,
    OpenCommentPanel,
    SaveFile,
    SendToTerminal,
    CopyReply,
    NextProblem,
    JumpToWait,
    RotateNextProject,
    ShowKeybindingHelp,
    ScrollOtherUp,
    ScrollOtherDown,
    ScrollOtherPageUp,
    ScrollOtherPageDown,
    ToggleCodeDiff,
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

/// Read the current clipboard text contents.
fn clipboard_contents() -> Option<String> {
  arboard::Clipboard::new().ok().and_then(|mut cb| cb.get_text().ok())
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
  active_comment_panel: Option<AnyView>,
  pre_comment_focus: Option<FocusHandle>,
  _comment_subscription: Option<Subscription>,
  split_generation: usize,
  recent_files: Vec<PathBuf>,
  _appearance_subscription: Subscription,
  _diff_view_subscription: Option<Subscription>,
  _focus_in_subscriptions: Vec<Subscription>,
  _hook_server: Option<HookServer>,
  _hook_poll_task: Option<Task<()>>,
  _ipc_poll_task: Option<Task<()>>,
  _bell_subscriptions: Vec<Subscription>,
  _breadcrumb_observers: Vec<Subscription>,
  _problems_poll_task: Option<Task<()>>,
  /// Home session before first L0 cross-session jump (project_index, session_id).
  pre_layer0_home: Option<(usize, SessionId)>,
  /// Current cycling state within the layered problem rotation.
  problem_cycle: Option<super::workspace::problems::ProblemCycleState>,
  snippets: SnippetDocument,
  _snippet_watcher: Option<notify::RecommendedWatcher>,
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

    // Track window activation for notification suppression.
    let window_activation_subscription =
      cx.observe_window_activation(window, |this: &mut Self, window, _cx| {
        this.window_active = window.is_window_active();
      });

    let mut focus_in_subscriptions = Vec::new();
    for (i, pane) in panes.iter().enumerate() {
      let focus = pane.read(cx).focus_handle(cx);
      focus_in_subscriptions.push(cx.on_focus_in(&focus, window, move |this, _window, cx| {
        if this.active_pane_index != i {
          this.active_pane_index = i;
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

    // Subscribe to bell events from all sessions' claude terminals.
    let bell_subscriptions = Self::subscribe_bells(&projects, cx);

    // Problem refresh poll task — runs immediately, then every 2 seconds.
    // Git diff generation runs on the background executor to avoid blocking
    // the main thread; only the lightweight state update happens on main.
    let problems_poll_task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
      use crate::views::diff_view::{DiffSource, generate_commit_diff, generate_diff};
      loop {
        // 1. Gather stale diff jobs from the main thread (cheap).
        let diff_jobs: Vec<(usize, PathBuf, DiffSource)> = cx
          .update(|cx: &mut App| {
            let Some(entity) = this.upgrade() else { return vec![] };
            entity
              .read(cx)
              .projects
              .iter()
              .enumerate()
              .filter_map(|(i, p)| {
                if p.diff_view.read(cx).is_stale() {
                  let (path, source) = p.diff_view.read(cx).diff_job();
                  Some((i, path, source))
                } else {
                  None
                }
              })
              .collect()
          })
          .unwrap_or_default();

        // 2. Run git diffs on background executor (heavy I/O, off main thread).
        let mut diff_results: Vec<(usize, String)> = Vec::new();
        for (idx, path, source) in diff_jobs {
          let text = cx
            .background_executor()
            .spawn(async move {
              match source {
                DiffSource::WorkingTree => generate_diff(&path),
                DiffSource::Commit { oid, .. } => generate_commit_diff(&path, oid),
              }
            })
            .await;
          diff_results.push((idx, text));
        }

        // 3. Apply results + refresh problems on main thread (cheap).
        let Ok(should_continue) = cx.update(|cx: &mut App| {
          let Some(entity) = this.upgrade() else { return false };
          entity.update(cx, |view, cx| {
            let mut changed = false;
            for (idx, diff_text) in diff_results {
              if idx < view.projects.len() {
                let data_changed =
                  view.projects[idx].diff_view.update(cx, |dv, _cx| dv.apply_diff_text(diff_text));
                changed |= data_changed;
              }
            }
            for project in &mut view.projects {
              changed |= project.refresh_problems(cx);
            }
            if changed {
              let pi = view.active_project_index;
              let label = view.projects[pi].active_label().map(|s| s.to_string());
              let todo_view = view.projects[pi].todo_view.clone();
              todo_view.update(cx, |tv, cx| tv.set_active_label(label.as_deref(), cx));
              cx.notify();
            }
          });
          true
        }) else {
          break;
        };
        if !should_continue {
          break;
        }
        Timer::after(StdDuration::from_secs(2)).await;
      }
    });

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

    // Load snippets and set up file watcher.
    snippets::ensure_file_exists();
    let snippets = snippets::load();
    let snippet_watcher = Self::setup_snippet_watcher(window, cx);

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
      active_comment_panel: None,
      pre_comment_focus: None,
      _comment_subscription: None,
      split_generation: 0,
      recent_files: Vec::new(),
      _appearance_subscription: appearance_subscription,
      _diff_view_subscription: None,
      _focus_in_subscriptions: focus_in_subscriptions,
      _hook_server: hook_server,
      _hook_poll_task: hook_poll_task,
      _ipc_poll_task: Some(ipc_poll_task),
      _bell_subscriptions: bell_subscriptions,
      _breadcrumb_observers: Vec::new(),
      _problems_poll_task: Some(problems_poll_task),
      pre_layer0_home: None,
      problem_cycle: None,
      snippets,
      _snippet_watcher: snippet_watcher,
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

    ws.subscribe_active_project(window, cx);
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

  /// Subscribe to bell events from all sessions' claude terminals.
  fn subscribe_bells(projects: &[ProjectState], cx: &mut Context<Self>) -> Vec<Subscription> {
    let mut subs = Vec::new();
    for (pi, project) in projects.iter().enumerate() {
      for (&id, session) in &project.sessions {
        subs.push(Self::make_bell_subscription(&session.claude_terminal, pi, id, cx));
      }
    }
    subs
  }

  /// Subscribe to bell events for a single newly-created session.
  fn subscribe_session_bell(&mut self, pi: usize, id: SessionId, cx: &mut Context<Self>) {
    let terminal = &self.projects[pi].sessions[&id].claude_terminal;
    let sub = Self::make_bell_subscription(terminal, pi, id, cx);
    self._bell_subscriptions.push(sub);
  }

  fn make_bell_subscription(
    terminal: &Entity<TerminalView>,
    pi: usize,
    session_id: SessionId,
    cx: &mut Context<Self>,
  ) -> Subscription {
    cx.subscribe(
      terminal,
      move |this: &mut Self, _, event: &TerminalViewEvent, cx: &mut Context<Self>| match event {
        TerminalViewEvent::Bell => {
          if let Some(session) =
            this.projects.get_mut(pi).and_then(|p| p.sessions.get_mut(&session_id))
            && session.pending_events.insert(PendingEvent::TerminalBell)
          {
            cx.notify();
          }
        }
      },
    )
  }

  /// Subscribe to the active project's diff_view and todo_view events.
  fn subscribe_active_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let project = &self.projects[self.active_project_index];

    let active_pi = self.active_project_index;
    let diff_view = project.diff_view.clone();
    self._diff_view_subscription = Some(cx.subscribe_in(
      &diff_view,
      window,
      move |this: &mut Self, _, event: &DiffViewEvent, window, cx| match event {
        DiffViewEvent::Reviewed => {
          this.projects[active_pi].refresh_problems(cx);
          this.open_drill_down_picker(&crate::views::picker::DrillDownPicker, window, cx);
        }
      },
    ));

    self.refresh_breadcrumb_observers(cx);
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

  fn focus_prev_pane(&mut self, _: &FocusPrevPane, window: &mut Window, cx: &mut Context<Self>) {
    if self.active_pane_index > 0 {
      self.active_pane_index -= 1;
      self.panes[self.active_pane_index].read(cx).focus_content(window);
      cx.notify();
    }
  }

  fn focus_next_pane(&mut self, _: &FocusNextPane, window: &mut Window, cx: &mut Context<Self>) {
    let count = self.visible_pane_count();
    if self.active_pane_index + 1 < count {
      self.active_pane_index += 1;
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
      PaneContentKind::GitDiff => {
        Some(OtherPaneScrollable::Editor(project.diff_view.read(cx).editor().clone()))
      }
      PaneContentKind::TodoEditor => {
        let editor = project.todo_view.read(cx).code_view().read(cx).editor().clone();
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
    // Clear bell when user switches to the claude terminal.
    if kind == PaneContentKind::ClaudeTerminal
      && let Some(session) = self.projects[self.active_project_index].active_session_mut()
    {
      session.pending_events.remove(&PendingEvent::TerminalBell);
    }

    // Refresh views that need it when actively switched to.
    if kind == PaneContentKind::GitDiff {
      self.projects[self.active_project_index].diff_view.update(cx, |v, cx| v.refresh(window, cx));
    }
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

  fn show_git_diff(&mut self, _: &ShowGitDiff, window: &mut Window, cx: &mut Context<Self>) {
    self.set_active_pane_view(PaneContentKind::GitDiff, window, cx);
  }

  fn show_code_viewer(&mut self, _: &ShowCodeViewer, window: &mut Window, cx: &mut Context<Self>) {
    self.set_active_pane_view(PaneContentKind::CodeViewer, window, cx);
  }

  fn show_todo_editor(&mut self, _: &ShowTodoEditor, window: &mut Window, cx: &mut Context<Self>) {
    self.set_active_pane_view(PaneContentKind::TodoEditor, window, cx);
  }

  /// Toggle between Code and Diff views for the current file.
  fn toggle_code_diff(&mut self, _: &ToggleCodeDiff, window: &mut Window, cx: &mut Context<Self>) {
    use crate::views::diff_view::source_line_to_diff_line;

    let kind = self.active_pane_entity().read(cx).content_kind();
    let pi = self.active_project_index;

    match kind {
      Some(PaneContentKind::CodeViewer) => {
        // Grab relative path and current source line before mutating.
        let (relative, source_line) = {
          let project = &self.projects[pi];
          let rel = project.code_view().and_then(|cv| {
            let cv = cv.read(cx);
            cv.file_path().and_then(|p| {
              p.strip_prefix(&project.path).ok().map(|r| r.to_string_lossy().into_owned())
            })
          });
          let line =
            project.code_view().map(|cv| cv.read(cx).editor().read(cx).cursor_position().line + 1);
          (rel, line)
        };
        self.set_active_pane_view(PaneContentKind::GitDiff, window, cx);
        if let Some(name) = relative {
          let diff_view = self.projects[pi].diff_view.clone();
          let idx = diff_view.read(cx).file_diffs().iter().position(|fd| fd.name == name);
          if let Some(idx) = idx {
            diff_view.update(cx, |v, cx| {
              v.set_file_index(idx, window, cx);
              // Scroll diff to the source line.
              if let Some(src_line) = source_line
                && let Some(content) = v.current_file_content()
                && let Some(diff_line) = source_line_to_diff_line(content, src_line)
              {
                v.scroll_to_line(diff_line, window, cx);
              }
            });
          }
        }
      }
      Some(PaneContentKind::GitDiff) => {
        let (file_name, source_line) = {
          let dv = self.projects[pi].diff_view.read(cx);
          (dv.current_file_name().map(str::to_string), dv.cursor_source_line(cx))
        };
        if let Some(name) = file_name {
          let full_path = self.projects[pi].path.join(&name);
          if let Some(cv) = self.projects[pi].code_view().cloned() {
            cv.update(cx, |v, cx| {
              v.open_file(full_path, window, cx);
              if let Some(line) = source_line {
                v.scroll_to_line(line, window, cx);
              }
            });
          }
          self.set_active_pane_view(PaneContentKind::CodeViewer, window, cx);
        }
      }
      _ => {}
    }
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
  // External editor
  // ---------------------------------------------------------------------------

  fn open_in_external_editor(
    &mut self,
    _: &OpenInExternalEditor,
    _window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let pane = self.active_pane_entity().clone();
    let kind = pane.read(cx).content_kind();
    let project = self.active_project();
    let (file_path, line) = match kind {
      Some(PaneContentKind::CodeViewer) => {
        if let Some(cv) = project.code_view() {
          let cv = cv.read(cx);
          let path = cv.file_path().map(|p| p.to_path_buf());
          let line = cv.editor().read(cx).cursor_position().line;
          (path, line)
        } else {
          (None, 0)
        }
      }
      Some(PaneContentKind::TodoEditor) => {
        let tv = project.todo_view.read(cx);
        let path = tv.file_path().to_path_buf();
        let line = tv.code_view().read(cx).editor().read(cx).cursor_position().line;
        (Some(path), line)
      }
      _ => (None, 0),
    };
    if let Some(path) = file_path {
      // Use `zed path:line` to open at the cursor position within the project.
      let arg = format!("{}:{}", path.display(), line + 1);
      let _ = std::process::Command::new("zed").arg(arg).spawn();
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
    self.switch_to_session_inner(project_idx, session_id, false, window, cx);
  }

  fn switch_to_session_inner(
    &mut self,
    project_idx: usize,
    session_id: Option<SessionId>,
    skip_acknowledge: bool,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    // Save the current session's pane layout before switching away.
    let saved = SavedPaneLayout {
      pane_kinds: std::array::from_fn(|i| self.panes[i].read(cx).content_kind()),
      active_pane_index: self.active_pane_index,
    };
    if let Some(session) = self.projects[self.active_project_index].active_session_mut() {
      session.saved_layout = Some(saved);
      // Mark outgoing session's terminals as hidden so background processing
      // batches more aggressively and skips cx.notify().
      session.claude_terminal.read(cx).set_visible(false);
      session.general_terminal.read(cx).set_visible(false);
    }

    let project_changed = project_idx != self.active_project_index;

    self.active_project_index = project_idx;
    self.projects[project_idx].active_session = session_id;

    // Mark incoming session's terminals as visible.
    if let Some(session) = self.projects[project_idx].active_session() {
      session.claude_terminal.read(cx).set_visible(true);
      session.general_terminal.read(cx).set_visible(true);
    }

    // Acknowledge pending events unless skipped (e.g. L0 problem jump).
    if !skip_acknowledge && let Some(session) = self.projects[project_idx].active_session_mut() {
      session.acknowledge();
    }

    // Reset problem cycle on manual session switches.
    self.problem_cycle = None;

    // Update the TODO view's active session highlight.
    {
      let label = self.projects[project_idx].active_label().map(|s| s.to_string());
      let todo_view = self.projects[project_idx].todo_view.clone();
      todo_view.update(cx, |tv, cx| tv.set_active_label(label.as_deref(), cx));
    }

    if project_changed {
      self.subscribe_active_project(window, cx);
    } else {
      // Breadcrumb observers depend on the active session's code_view.
      self.refresh_breadcrumb_observers(cx);
    }

    // Refresh problems after acknowledge.
    self.projects[project_idx].refresh_problems(cx);

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

  /// Find a session by ID across all projects and switch to it.
  /// Used by notification click handler which passes session UUIDs.
  fn switch_to_session_id(
    &mut self,
    session_id: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    for (pi, project) in self.projects.iter().enumerate() {
      if let Some((id, _)) = project.session_by_uuid(session_id) {
        self.switch_to_session(pi, Some(id), window, cx);
        return;
      }
      // Fall back to label match for older notifications.
      if let Some((id, _)) = project.session_by_label(session_id) {
        self.switch_to_session(pi, Some(id), window, cx);
        return;
      }
    }
  }

  /// Restore saved pane contents for the active session, or use defaults.
  /// The pane layout (1/2/3) is window-level and not restored here.
  fn restore_or_default_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    // Copy saved layout data to avoid borrow conflict with set_pane_view.
    let saved = self.projects[self.active_project_index]
      .active_session()
      .and_then(|s| s.saved_layout.as_ref())
      .map(|s| (s.pane_kinds, s.active_pane_index));

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
      // First visit: default layout.
      self.set_pane_view(0, PaneContentKind::ClaudeTerminal, cx);
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
      PaneContentKind::GitDiff => {
        let focus = project.diff_view.read(cx).focus_handle(cx);
        Some((project.diff_view.clone().into(), focus))
      }
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

  /// Launch a brand new Claude session (no --resume), with a blank UUID.
  /// The UUID will be assigned when the first hook event arrives.
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

    let label = "New Session".to_string();

    let session = SessionState::create(
      id,
      None, // no UUID yet — will be assigned on first hook
      label.clone(),
      &project_path,
      &palette,
      window,
      cx,
    );

    project.sessions.insert(id, session);

    // Insert TODO heading with blank UUID.
    let todo_view = project.todo_view.clone();
    todo_view.update(cx, |tv, cx| {
      tv.insert_session_heading("", &label, window, cx);
      tv.save(cx);
    });

    self.subscribe_session_bell(project_idx, id, cx);
    self.switch_to_session(project_idx, Some(id), window, cx);
  }

  /// Activate an empty project by creating a brand new Claude session.
  fn init_empty_project(
    &mut self,
    project_idx: usize,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.create_new_session(project_idx, window, cx);
  }

  /// Adopt a TODO.md session that isn't running yet.
  /// If it has a UUID, launches `claude --resume <uuid>` (invalid UUIDs just
  /// show an error in the terminal — jc won't crash). If the UUID is empty,
  /// launches a fresh `claude` and the first hook event will assign the UUID.
  fn adopt_session(
    &mut self,
    project_idx: usize,
    uuid: &str,
    label: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    // If the session is disabled, remove the [D] label before adopting.
    {
      let project = &self.projects[project_idx];
      let todo_view = project.todo_view.clone();
      let is_disabled = todo_view
        .read(cx)
        .document()
        .session_by_label(label)
        .is_some_and(|s| s.status == jc_core::todo::SessionStatus::Disabled);
      if is_disabled {
        todo_view.update(cx, |tv, cx| {
          tv.toggle_session_disabled(label, window, cx);
          tv.save(cx);
        });
      }
    }

    let project_path = self.projects[project_idx].path.clone();
    let palette = palette_from_window(window);

    let project = &mut self.projects[project_idx];
    let id = project.next_session_id;
    project.next_session_id += 1;

    let uuid_opt = if uuid.is_empty() { None } else { Some(uuid.to_string()) };
    let session =
      SessionState::create(id, uuid_opt, label.to_string(), &project_path, &palette, window, cx);

    project.sessions.insert(id, session);
    self.subscribe_session_bell(project_idx, id, cx);
    self.switch_to_session(project_idx, Some(id), window, cx);
  }

  /// Collect TodoDocument references from each project's todo_view.
  fn todo_documents<'a>(&'a self, cx: &'a App) -> Vec<&'a jc_core::todo::TodoDocument> {
    self.projects.iter().map(|p| p.todo_view.read(cx).document()).collect()
  }

  // ---------------------------------------------------------------------------
  // Session expiration (GC'd JSONL detection)
  // ---------------------------------------------------------------------------

  /// Scan all projects for unadopted sessions whose JSONL files no longer exist
  /// and mark them `[X]` in TODO.md so they don't appear in the picker.
  fn mark_expired_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    for project in &self.projects {
      let session_dir = ProjectState::session_dir(&project.path);
      let document = project.todo_view.read(cx).document().clone();
      let adopted_uuids: std::collections::HashSet<&str> =
        project.sessions.values().filter_map(|s| s.uuid.as_deref()).collect();
      let expired_labels: Vec<String> = document
        .sessions
        .iter()
        .filter(|s| {
          !s.uuid.is_empty()
            && s.status != jc_core::todo::SessionStatus::Expired
            && !adopted_uuids.contains(s.uuid.as_str())
            && !session_dir.join(format!("{}.jsonl", s.uuid)).exists()
        })
        .map(|s| s.label.clone())
        .collect();
      if !expired_labels.is_empty() {
        let todo_view = project.todo_view.clone();
        todo_view.update(cx, |tv, cx| {
          for label in &expired_labels {
            tv.mark_session_expired(label, window, cx);
          }
          tv.save(cx);
        });
      }
    }
  }

  // ---------------------------------------------------------------------------
  // Session disable toggle
  // ---------------------------------------------------------------------------

  fn toggle_session_disabled(
    &mut self,
    project_idx: usize,
    label: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let project = &mut self.projects[project_idx];
    let todo_view = project.todo_view.clone();

    // Check if the session is currently adopted (running).
    let adopted_id = project.session_by_label(label).map(|(id, _)| id);

    todo_view.update(cx, |tv, cx| {
      tv.toggle_session_disabled(label, window, cx);
      tv.save(cx);
    });

    // If the session was adopted and is now being disabled, detach it.
    let is_now_disabled = todo_view
      .read(cx)
      .document()
      .session_by_label(label)
      .is_some_and(|s| s.status == jc_core::todo::SessionStatus::Disabled);
    if is_now_disabled && let Some(id) = adopted_id {
      let project = &mut self.projects[project_idx];
      project.sessions.remove(&id);

      if project.active_session == Some(id) {
        let next_id = project.sessions.keys().next().copied();
        project.active_session = next_id;
      }

      self._bell_subscriptions = Self::subscribe_bells(&self.projects, cx);
      let active = self.projects[project_idx].active_session;
      if active.is_some() {
        // Another session in the same project — switch to it.
        self.switch_to_session(project_idx, active, window, cx);
      } else {
        // Last session in this project was disabled — jump to the next
        // project that has sessions, falling back to staying put.
        let next = self
          .projects
          .iter()
          .enumerate()
          .find(|(pi, p)| *pi != project_idx && !p.sessions.is_empty());
        if let Some((pi, p)) = next {
          let sid = p.active_session;
          self.switch_to_session(pi, sid, window, cx);
        } else {
          self.switch_to_session(project_idx, None, window, cx);
        }
      }
    }
  }

  // ---------------------------------------------------------------------------
  // Save file
  // ---------------------------------------------------------------------------

  fn save_file(&mut self, _: &SaveFile, _window: &mut Window, cx: &mut Context<Self>) {
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
    // Only send when the TODO editor is focused.
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

    // Insert a WAIT section if the session doesn't have one.
    todo_view.update(cx, |tv, cx| {
      tv.ensure_wait(&label, window, cx);
    });

    let Some((message_text, _)) =
      todo_view.update(cx, |tv, cx| tv.send_selection(&label, window, cx))
    else {
      return;
    };

    // Re-run ensure_wait so the empty WAIT body gets a blank line for typing.
    todo_view.update(cx, |tv, cx| {
      tv.ensure_wait(&label, window, cx);
    });

    // Mark session as busy — we're about to submit work to Claude.
    if let Some(session) = self.projects[self.active_project_index].active_session_mut() {
      session.busy = true;
      session.has_ever_been_busy = true;
    }

    // Paste the message into the Claude terminal, then send Enter to submit.
    claude_terminal.read(cx).write_text(&message_text);

    // Send Enter (\r) from a background thread after a delay so the
    // application has time to process the pasted content.
    let pty = claude_terminal.read(cx).pty_handle();
    std::thread::spawn(move || {
      std::thread::sleep(StdDuration::from_millis(200));
      let _ = pty.write_all(b"\r");
    });

    cx.notify();
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

    let document = todo_view.read(cx).document().clone();
    let Some(session) = document.session_by_label(&label) else {
      return;
    };
    let Some(wait) = &session.wait else {
      return;
    };
    let wait_line = wait.line;

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
  // Copy reply (/copy)
  // ---------------------------------------------------------------------------

  fn copy_reply(&mut self, _: &CopyReply, window: &mut Window, cx: &mut Context<Self>) {
    let project = &self.projects[self.active_project_index];
    let Some(session) = project.active_session() else {
      return;
    };

    // Determine file name: use UUID if available, otherwise the label.
    let filename = session.uuid.as_deref().filter(|u| !u.is_empty()).unwrap_or(&session.label);
    let reply_dir = project.path.join(".jc/replies");
    let reply_path = reply_dir.join(format!("{filename}.md"));

    // Send `/copy\n` to the Claude terminal.
    let claude_terminal = session.claude_terminal.clone();
    claude_terminal.read(cx).write_text("/copy");
    let pty = claude_terminal.read(cx).pty_handle();
    std::thread::spawn(move || {
      std::thread::sleep(StdDuration::from_millis(200));
      let _ = pty.write_all(b"\r");
    });

    // Read clipboard now, then poll for change.
    let code_view = session.code_view.clone();
    cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
      // Read initial clipboard.
      let initial = clipboard_contents().unwrap_or_default();

      // Poll for clipboard change (up to 3s, every 200ms).
      let mut new_content = None;
      for _ in 0..15 {
        Timer::after(StdDuration::from_millis(200)).await;
        if let Some(current) = clipboard_contents()
          && current != initial
          && !current.is_empty()
        {
          new_content = Some(current);
          break;
        }
      }

      let Some(content) = new_content else {
        return;
      };

      // Create directory and write file.
      let _ = std::fs::create_dir_all(&reply_dir);
      if let Err(e) = std::fs::write(&reply_path, &content) {
        eprintln!("failed to write reply file: {e}");
        return;
      }

      // Open the file in the code view and switch to it.
      let _ = this.update_in(cx, |ws, window, cx| {
        code_view.update(cx, |v, cx| {
          v.set_language_override("markdown", cx);
          v.open_file(reply_path, window, cx);
        });
        ws.set_active_pane_view(PaneContentKind::CodeViewer, window, cx);
      });
    })
    .detach();
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

    // PromptSubmit just sets busy — it's not a problem/notification.
    if matches!(event.kind, HookEventKind::PromptSubmit) {
      let session_uuid = &event.session_id;
      if !session_uuid.is_empty() {
        let mut found = false;
        for project in &mut self.projects {
          if let Some(session) =
            project.sessions.values_mut().find(|s| s.uuid.as_deref() == Some(session_uuid))
          {
            session.busy = true;
            session.has_ever_been_busy = true;
            session.pending_events.remove(&PendingEvent::ClaudePermission);
            found = true;
            break;
          }
        }
        // If no UUID match, assign to a pending (uuid=None) session in the matching project.
        if !found {
          for project in &mut self.projects {
            if event.project_path.as_deref() != Some(project.path.as_path()) {
              continue;
            }
            if let Some(session) = project.sessions.values_mut().find(|s| s.uuid.is_none()) {
              session.uuid = Some(session_uuid.clone());
              session.busy = true;
              session.has_ever_been_busy = true;
              session.pending_events.remove(&PendingEvent::ClaudePermission);
              // Update TODO.md with the new UUID.
              let label = session.label.clone();
              project.todo_view.update(cx, |tv, cx| {
                tv.update_session_uuid(&label, session_uuid, &mut *window, cx);
                tv.save(cx);
              });
              break;
            }
          }
        }
      }
      cx.notify();
      return;
    }

    // Determine the pending event (if any) and whether this clears busy.
    let (pending, clears_busy) = match event.kind {
      HookEventKind::Stop => (None, true),
      HookEventKind::StopFailure => (Some(PendingEvent::ClaudeStopFailure), true),
      HookEventKind::PermissionPrompt => (Some(PendingEvent::ClaudePermission), true),
      HookEventKind::IdlePrompt => (None, true),
      HookEventKind::PromptSubmit | HookEventKind::SessionClear { .. } => unreachable!(),
    };

    // Match by UUID (session_id from hook) across all projects.
    let mut matched_project: Option<String> = None;
    let mut matched_label: Option<String> = None;

    let session_uuid = &event.session_id;
    if !session_uuid.is_empty() {
      let mut found = false;
      // First pass: find an existing session with this UUID.
      for project in &mut self.projects {
        if let Some(session) =
          project.sessions.values_mut().find(|s| s.uuid.as_deref() == Some(session_uuid))
        {
          if clears_busy {
            session.busy = false;
            // Claude has progressed past any permission prompt.
            session.pending_events.remove(&PendingEvent::ClaudePermission);
          }
          if let Some(ref pe) = pending {
            session.pending_events.insert(pe.clone());
          }
          matched_project = Some(project.name.clone());
          matched_label = Some(session.label.clone());
          found = true;
          break;
        }
      }
      // Fallback: assign UUID to a pending (uuid=None) session in the matching project.
      if !found {
        for project in &mut self.projects {
          if event.project_path.as_deref() != Some(project.path.as_path()) {
            continue;
          }
          if let Some(session) = project.sessions.values_mut().find(|s| s.uuid.is_none()) {
            session.uuid = Some(session_uuid.clone());
            if clears_busy {
              session.busy = false;
              session.pending_events.remove(&PendingEvent::ClaudePermission);
            }
            if let Some(ref pe) = pending {
              session.pending_events.insert(pe.clone());
            }
            // Update TODO.md with the new UUID.
            let label = session.label.clone();
            project.todo_view.update(cx, |tv, cx| {
              tv.update_session_uuid(&label, session_uuid, &mut *window, cx);
              tv.save(cx);
            });
            matched_project = Some(project.name.clone());
            matched_label = Some(label);
            break;
          }
        }
      }
    }

    // Notify when the window is not active (user is in another app).
    if let (Some(project_name), Some(session_label)) = (matched_project, matched_label)
      && !self.window_active
    {
      let critical =
        matches!(event.kind, HookEventKind::PermissionPrompt | HookEventKind::StopFailure);
      let title = format!("{project_name} > {session_label}");
      let message = match event.kind {
        HookEventKind::Stop => "Claude finished",
        HookEventKind::StopFailure => "API error",
        HookEventKind::PermissionPrompt => "Permission needed",
        HookEventKind::IdlePrompt => "Claude is idle",
        HookEventKind::PromptSubmit | HookEventKind::SessionClear { .. } => unreachable!(),
      };
      let notify_id =
        if event.session_id.is_empty() { None } else { Some(event.session_id.as_str()) };
      crate::notify::notify(&title, message, critical, notify_id);
    }

    cx.notify();
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
    let Some(project) = self.projects.iter_mut().find(|p| p.path == *project_path) else { return };
    let Some(session) =
      project.sessions.values_mut().find(|s| s.uuid.as_deref() == Some(old_session_id))
    else {
      eprintln!("hook: session-clear for unknown uuid {old_session_id}");
      return;
    };

    eprintln!("hook: session cleared, uuid {old_session_id} -> {new_session_id}");
    let label = session.label.clone();
    session.uuid = Some(new_session_id.to_string());

    // Update TODO.md: change `> uuid=OLD` to `> uuid=NEW`.
    let todo_view = project.todo_view.clone();
    todo_view.update(cx, |tv, cx| {
      tv.update_session_uuid(&label, new_session_id, window, cx);
      tv.save(cx);
    });
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
    KeyBinding::new("cmd-shift-e", OpenInExternalEditor, Some("Workspace")),
    KeyBinding::new("cmd-p", crate::views::picker::ShowSessionPicker, Some("Workspace")),
    KeyBinding::new("cmd-k", OpenCommentPanel, Some("Workspace")),
    KeyBinding::new("cmd-s", SaveFile, Some("Workspace")),
    KeyBinding::new("cmd-enter", SendToTerminal, Some("Workspace")),
    KeyBinding::new("cmd-;", NextProblem, Some("Workspace")),
    KeyBinding::new("cmd-.", JumpToWait, Some("Workspace")),
    KeyBinding::new("cmd-shift-k", crate::views::picker::ShowSnippetPicker, Some("Workspace")),
    KeyBinding::new("cmd-shift-p", crate::views::picker::ProjectActionsPicker, Some("Workspace")),
    KeyBinding::new("cmd-shift-c", CopyReply, Some("Workspace")),
    KeyBinding::new("cmd-`", RotateNextProject, Some("Workspace")),
    KeyBinding::new("cmd-?", ShowKeybindingHelp, Some("Workspace")),
    KeyBinding::new("cmd-alt-up", ScrollOtherUp, Some("Workspace")),
    KeyBinding::new("cmd-alt-down", ScrollOtherDown, Some("Workspace")),
    KeyBinding::new("cmd-alt-pageup", ScrollOtherPageUp, Some("Workspace")),
    KeyBinding::new("cmd-alt-pagedown", ScrollOtherPageDown, Some("Workspace")),
    KeyBinding::new("cmd-d", ToggleCodeDiff, Some("Workspace")),
  ]);

  cx.bind_keys([
    KeyBinding::new("cmd-[", FocusPrevPane, Some("Input")),
    KeyBinding::new("cmd-]", FocusNextPane, Some("Input")),
    KeyBinding::new("cmd-k", OpenCommentPanel, Some("Input")),
    KeyBinding::new("cmd-s", SaveFile, Some("Input")),
    KeyBinding::new("cmd-enter", SendToTerminal, Some("Input")),
    KeyBinding::new("cmd-shift-k", crate::views::picker::ShowSnippetPicker, Some("Input")),
    KeyBinding::new("cmd-.", JumpToWait, Some("Input")),
    KeyBinding::new("cmd-`", RotateNextProject, Some("Input")),
    KeyBinding::new("cmd-alt-up", ScrollOtherUp, Some("Input")),
    KeyBinding::new("cmd-alt-down", ScrollOtherDown, Some("Input")),
    KeyBinding::new("cmd-alt-pageup", ScrollOtherPageUp, Some("Input")),
    KeyBinding::new("cmd-alt-pagedown", ScrollOtherPageDown, Some("Input")),
    KeyBinding::new("cmd-d", ToggleCodeDiff, Some("Input")),
  ]);
}
