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
use gpui_component::theme::Theme;
use jc_core::config::{AppConfig, AppState};
use jc_core::hooks::{HookEvent, HookEventKind, HookServer};
use jc_core::problem::ProblemTarget;
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
  ]
);

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

/// Read the current macOS clipboard contents via `pbpaste`.
fn clipboard_contents() -> Option<String> {
  std::process::Command::new("pbpaste")
    .output()
    .ok()
    .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
}

pub struct Workspace {
  panes: Vec<Entity<Pane>>,
  active_pane_index: usize,
  layout: PaneLayout,
  projects: Vec<ProjectState>,
  active_project_index: usize,
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
  _problems_poll_task: Option<Task<()>>,
  last_jumped_target: Option<ProblemTarget>,
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
        let task = cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
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
    let problems_poll_task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
      loop {
        let Ok(should_continue) = cx.update(|cx: &mut App| {
          if let Some(entity) = this.upgrade() {
            entity.update(cx, |view, cx| {
              let mut changed = false;
              // Refresh stale diff views so problem counts reflect git state.
              for project in &mut view.projects {
                let stale = project.diff_view.read(cx).is_stale();
                if stale {
                  project.diff_view.update(cx, |dv, _cx| dv.refresh_data());
                  changed = true;
                }
              }
              for project in &mut view.projects {
                changed |= project.refresh_problems(cx);
              }
              if changed {
                // Keep TodoView active label in sync (e.g. after a heading rename).
                {
                  let pi = view.active_project_index;
                  let label = view.projects[pi].active_label().map(|s| s.to_string());
                  let todo_view = view.projects[pi].todo_view.clone();
                  todo_view.update(cx, |tv, cx| tv.set_active_label(label.as_deref(), cx));
                }
                cx.notify();
              }
            });
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

    // Initialize notification system and poll for notification action responses.
    let notification_action_rx = crate::notify::action_receiver();
    crate::notify::init();
    let notification_poll_task =
      cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
        while let Ok(slug) = notification_action_rx.recv_async().await {
          let Ok(should_continue) = cx.update(|window, cx| {
            if let Some(entity) = this.upgrade() {
              entity.update(cx, |ws, cx| ws.switch_to_slug(&slug, window, cx));
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
      _problems_poll_task: Some(problems_poll_task),
      last_jumped_target: None,
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
          {
            session.pending_events.insert(PendingEvent::TerminalBell);
          }
          cx.notify();
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
    let active = self.active_session_count();
    self.show_close_confirm(active, false, window, cx);
  }

  fn minimize_window(&mut self, _: &MinimizeWindow, window: &mut Window, _cx: &mut Context<Self>) {
    window.minimize_window();
  }

  fn quit(&mut self, _: &Quit, window: &mut Window, cx: &mut Context<Self>) {
    if self.close_confirm.is_some() {
      return;
    }
    let active = self.active_session_count();
    self.show_close_confirm(active, true, window, cx);
  }

  /// Count sessions that are actively working (not idle/stopped).
  fn active_session_count(&self) -> usize {
    self
      .projects
      .iter()
      .flat_map(|p| p.sessions.values())
      .filter(|s| s.busy)
      .count()
  }

  fn show_close_confirm(
    &mut self,
    session_count: usize,
    is_quit: bool,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.close_confirm_is_quit = is_quit;
    self.pre_close_confirm_focus = window.focused(cx);
    let view = cx.new(|cx| CloseConfirm::new(session_count, is_quit, cx));
    let sub = cx.subscribe_in(&view, window, |this: &mut Self, _, event, window, cx| {
      match event {
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
    let pane_idx = self.active_pane_index;
    self.set_pane_view(pane_idx, kind, cx);

    self.panes[pane_idx].read(cx).focus_content(window);

    // When switching to the TODO editor, auto-scroll to the end of the active session's WAIT body.
    if kind == PaneContentKind::TodoEditor {
      let project = &self.projects[self.active_project_index];
      let tv = project.todo_view.read(cx);
      if let Some(label) = project.active_label() {
        let text = tv.editor_text(cx);
        if let Some(wait_line) = tv.document().wait_body_end_line(label, &text) {
          let wait_line_0 = wait_line.saturating_sub(1);
          let _ = tv;
          project.todo_view.update(cx, |tv, cx| tv.scroll_to_line(wait_line_0, window, cx));
        }
      }
    }
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
        let cv = project.code_view.read(cx);
        let path = cv.file_path().map(|p| p.to_path_buf());
        let line = cv.editor().read(cx).cursor_position().line;
        (path, line)
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
    let project_changed = project_idx != self.active_project_index;

    // Save the current project's pane layout before switching away.
    if project_changed {
      let saved = SavedPaneLayout {
        pane_kinds: std::array::from_fn(|i| self.panes[i].read(cx).content_kind()),
        active_pane_index: self.active_pane_index,
        layout: self.layout,
      };
      self.projects[self.active_project_index].saved_layout = Some(saved);
    }

    self.active_project_index = project_idx;
    self.projects[project_idx].active_session = session_id;

    // Acknowledge pending events for this session (user is switching to it).
    if let Some(session) = self.projects[project_idx].active_session_mut() {
      session.acknowledge();
    }

    // Update the TODO view's active session highlight.
    {
      let label = self.projects[project_idx].active_label().map(|s| s.to_string());
      let todo_view = self.projects[project_idx].todo_view.clone();
      todo_view.update(cx, |tv, cx| tv.set_active_label(label.as_deref(), cx));
    }

    if project_changed {
      self.subscribe_active_project(window, cx);
    }

    // Refresh problems after acknowledge.
    self.projects[project_idx].refresh_problems(cx);

    if project_changed {
      self.restore_or_default_panes(window, cx);
    } else {
      self.rebind_session_panes(window, cx);
    }

    cx.notify();
  }

  fn rotate_next_project(
    &mut self,
    _: &RotateNextProject,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    // Build a flat list of (project_index, session_id) in deterministic order.
    let mut slots: Vec<(usize, Option<SessionId>)> = Vec::new();
    for (pi, project) in self.projects.iter().enumerate() {
      if project.sessions.is_empty() {
        slots.push((pi, None));
      } else {
        let mut ids: Vec<SessionId> = project.sessions.keys().copied().collect();
        ids.sort();
        for id in ids {
          slots.push((pi, Some(id)));
        }
      }
    }
    if slots.len() <= 1 {
      return;
    }

    let current = (self.active_project_index, self.projects[self.active_project_index].active_session);
    let pos = slots.iter().position(|s| *s == current).unwrap_or(0);
    let (next_pi, next_sid) = slots[(pos + 1) % slots.len()];
    self.switch_to_session(next_pi, next_sid, window, cx);
  }

  /// Find a session by slug/label across all projects and switch to it.
  /// This is used by notification actions which still pass slug strings.
  fn switch_to_slug(&mut self, slug: &str, window: &mut Window, cx: &mut Context<Self>) {
    for (pi, project) in self.projects.iter().enumerate() {
      // Try to match by label.
      if let Some((id, _)) = project.session_by_label(slug) {
        self.switch_to_session(pi, Some(id), window, cx);
        return;
      }
      // Try to match by UUID.
      if let Some((id, _)) = project.session_by_uuid(slug) {
        self.switch_to_session(pi, Some(id), window, cx);
        return;
      }
    }
  }

  /// Restore saved pane layout for the active project, or use defaults.
  fn restore_or_default_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    // Copy saved layout data to avoid borrow conflict with set_pane_view.
    let saved = self.projects[self.active_project_index]
      .saved_layout
      .as_ref()
      .map(|s| (s.pane_kinds, s.active_pane_index, s.layout));

    if let Some((kinds, active, layout)) = saved {
      self.layout = layout;
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

  /// When switching sessions within the same project, swap session-bound views
  /// in-place (Claude terminal, general terminal, reply viewer) without
  /// disturbing the pane layout or non-session views.
  fn rebind_session_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.projects[self.active_project_index].active_session().is_none() {
      return;
    }

    // Collect which panes need rebinding (avoids borrow conflict with set_pane_view).
    let mut to_rebind: Vec<(usize, PaneContentKind)> = Vec::new();
    for i in 0..self.panes.len() {
      if let Some(kind) = self.panes[i].read(cx).content_kind() {
        match kind {
          PaneContentKind::ClaudeTerminal
          | PaneContentKind::GeneralTerminal => to_rebind.push((i, kind)),
          _ => {}
        }
      }
    }

    if to_rebind.is_empty() {
      // No pane shows a session view; put claude terminal in active pane.
      self.set_pane_view(self.active_pane_index, PaneContentKind::ClaudeTerminal, cx);
    } else {
      for (i, kind) in to_rebind {
        self.set_pane_view(i, kind, cx);
      }
    }

    self.panes[self.active_pane_index].read(cx).focus_content(window);
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
      PaneContentKind::CodeViewer => {
        let focus = project.code_view.read(cx).focus_handle(cx);
        Some((project.code_view.clone().into(), focus))
      }
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
    let project_path = self.projects[project_idx].path.clone();
    let palette = palette_from_window(window);

    let project = &mut self.projects[project_idx];
    let id = project.next_session_id;
    project.next_session_id += 1;

    let uuid_opt = if uuid.is_empty() { None } else { Some(uuid.to_string()) };
    let session = SessionState::create(
      id,
      uuid_opt,
      label.to_string(),
      &project_path,
      &palette,
      window,
      cx,
    );

    project.sessions.insert(id, session);
    self.subscribe_session_bell(project_idx, id, cx);
    self.switch_to_session(project_idx, Some(id), window, cx);
  }

  /// Collect TodoDocument references from each project's todo_view.
  fn todo_documents<'a>(&'a self, cx: &'a App) -> Vec<&'a jc_core::todo::TodoDocument> {
    self.projects.iter().map(|p| p.todo_view.read(cx).document()).collect()
  }

  // ---------------------------------------------------------------------------
  // Session removal
  // ---------------------------------------------------------------------------

  fn remove_session(
    &mut self,
    project_idx: usize,
    session_id: SessionId,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let project = &mut self.projects[project_idx];

    // Get label before removing, for marking deleted in TODO.
    let label = project.sessions.get(&session_id).map(|s| s.label.clone());

    // Remove the session state (drops terminals).
    project.sessions.remove(&session_id);

    // Mark the session as deleted in TODO.md so it won't reappear on restart.
    if let Some(label) = &label {
      let todo_view = project.todo_view.clone();
      todo_view.update(cx, |tv, cx| {
        tv.mark_session_deleted(label, window, cx);
        tv.save(cx);
      });
    }

    // If the removed session was active, pick another one.
    if project.active_session == Some(session_id) {
      let next_id = project.sessions.keys().next().copied();
      project.active_session = next_id;
    }

    // Rebuild bell subscriptions since a session was removed.
    self._bell_subscriptions = Self::subscribe_bells(&self.projects, cx);

    // Switch to the new active session (or clear panes if none).
    let active = self.projects[project_idx].active_session;
    self.switch_to_session(project_idx, active, window, cx);
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
        project.code_view.update(cx, |v, cx| v.save(cx));
      }
      Some(PaneContentKind::TodoEditor) => {
        project.todo_view.update(cx, |v, cx| v.save(cx));
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
    todo_view.update(cx, |tv, cx| { tv.ensure_wait(&label, window, cx); });

    let Some((message_text, _)) =
      todo_view.update(cx, |tv, cx| tv.send_selection(&label, window, cx))
    else {
      return;
    };

    // Mark session as busy — we're about to submit work to Claude.
    if let Some(session) = self.projects[self.active_project_index].active_session_mut() {
      session.busy = true;
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

    // Show Claude terminal in the "other" pane so the user can see it working.
    let target = if self.active_pane_index == 0 { 1.min(self.panes.len() - 1) } else { 0 };
    self.active_pane_index = target;
    self.set_active_pane_view(PaneContentKind::ClaudeTerminal, window, cx);
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
    todo_view.update(cx, |tv, cx| { tv.ensure_wait(&label, window, cx); });

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
    let existing = (0..visible).find(|&i| {
      self.panes[i].read(cx).content_kind() == Some(PaneContentKind::TodoEditor)
    });
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
    let filename = session
      .uuid
      .as_deref()
      .filter(|u| !u.is_empty())
      .unwrap_or(&session.label);
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
    let code_view = project.code_view.clone();
    cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
      // Read initial clipboard.
      let initial = clipboard_contents().unwrap_or_default();

      // Poll for clipboard change (up to 3s, every 200ms).
      let mut new_content = None;
      for _ in 0..15 {
        Timer::after(StdDuration::from_millis(200)).await;
        if let Some(current) = clipboard_contents() {
          if current != initial && !current.is_empty() {
            new_content = Some(current);
            break;
          }
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

  fn handle_hook_event(
    &mut self,
    event: HookEvent,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
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

    let pending = match event.kind {
      HookEventKind::Stop => PendingEvent::ClaudeStop,
      HookEventKind::PermissionPrompt => PendingEvent::ClaudePermission,
      HookEventKind::IdlePrompt => PendingEvent::ClaudeIdle,
      HookEventKind::SessionClear { .. } => unreachable!(),
    };

    // Match by UUID (session_id from hook) across all projects.
    let mut matched_project: Option<String> = None;
    let mut matched_label: Option<String> = None;

    let session_uuid = &event.session_id;
    if !session_uuid.is_empty() {
      for project in &mut self.projects {
        let found = project
          .sessions
          .values_mut()
          .find(|s| s.uuid.as_deref() == Some(session_uuid));
        if let Some(session) = found {
          session.ever_active = true;
          session.busy = !matches!(pending, PendingEvent::ClaudeIdle | PendingEvent::ClaudeStop);
          let is_new = session.pending_events.insert(pending.clone());
          if is_new {
            matched_project = Some(project.name.clone());
            matched_label = Some(session.label.clone());
          }
          break;
        }
        // If no UUID match, try to assign to a pending (uuid=None) session.
        if let Some(session) = project.sessions.values_mut().find(|s| s.uuid.is_none()) {
          session.uuid = Some(session_uuid.clone());
          session.ever_active = true;
          session.busy = !matches!(pending, PendingEvent::ClaudeIdle | PendingEvent::ClaudeStop);
          let is_new = session.pending_events.insert(pending.clone());
          // Update TODO.md with the new UUID.
          let label = session.label.clone();
          project.todo_view.update(cx, |tv, cx| {
            tv.update_session_uuid(&label, session_uuid, &mut *window, cx);
            tv.save(cx);
          });
          if is_new {
            matched_project = Some(project.name.clone());
            matched_label = Some(label);
          }
          break;
        }
      }
    }

    // Notify when the window is not active (user is in another app).
    if let (Some(project_name), Some(session_label)) = (matched_project, matched_label)
      && !self.window_active
    {
      let critical = matches!(event.kind, HookEventKind::PermissionPrompt);
      let title = format!("{project_name} > {session_label}");
      let message = match event.kind {
        HookEventKind::Stop => "Claude finished",
        HookEventKind::PermissionPrompt => "Permission needed",
        HookEventKind::IdlePrompt => "Claude is idle",
        HookEventKind::SessionClear { .. } => unreachable!(),
      };
      // Pass session_uuid as notification identifier (replaces old slug).
      let notify_id = if event.session_id.is_empty() { None } else { Some(event.session_id.as_str()) };
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
    let Some(session) = project.sessions.values_mut().find(|s| s.uuid.as_deref() == Some(old_session_id)) else {
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
  ]);
}
