mod pickers;
mod problems;
mod render;

use crate::views::diff_view::DiffViewEvent;
use crate::views::keybinding_help::{DismissHelpEvent, KeybindingHelp};
use crate::views::pane::{Pane, PaneContent, PaneContentKind};
use crate::views::project_state::{ProjectState, SavedPaneLayout};
use crate::views::reply_view::gc_stale_replies;
use crate::views::session_state::{PendingEvent, SessionState};
use gpui::*;
use gpui_component::theme::Theme;
use jc_core::config::{AppConfig, AppState};
use jc_core::hooks::{HookEvent, HookEventKind, HookServer};
use jc_core::problem::ProblemTarget;
use jc_core::snippets::{self, SnippetDocument};
use jc_core::theme::Appearance;
use jc_terminal::{Palette, TerminalView, TerminalViewEvent};
use std::ops::DerefMut;
use std::path::PathBuf;
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
    OpenGitLogPicker,
    ShowReplyViewer,
    OpenCommentPanel,
    SaveFile,
    SendToTerminal,
    NextProblem,
    ShowKeybindingHelp,
  ]
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneLayout {
  One,
  #[default]
  Two,
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
      let path = project.path.clone();
      std::thread::spawn(move || gc_stale_replies(&path));
    }

    // If no projects registered, create a default one from cwd.
    if projects.is_empty() {
      let path = std::env::current_dir().unwrap_or_default();
      let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".into());
      let gc_path = path.clone();
      std::thread::spawn(move || gc_stale_replies(&gc_path));
      projects.push(ProjectState::create(path, name, &palette, window, cx));
    }

    // Determine initial pane content from first project's first session.
    let initial_contents = Self::initial_pane_contents(&projects[0], cx);
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
        let task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
          while let Ok(event) = rx.recv_async().await {
            let Ok(should_continue) = cx.update(|cx: &mut App| {
              if let Some(entity) = this.upgrade() {
                entity.update(cx, |view, cx| view.handle_hook_event(event, cx));
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
              for project in &mut view.projects {
                changed |= project.refresh_problems(cx);
              }
              if changed {
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
    };

    ws.subscribe_active_project(window, cx);
    ws
  }

  /// Build initial PaneContent for all 3 panes from a project.
  fn initial_pane_contents(project: &ProjectState, cx: &App) -> Vec<PaneContent> {
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

    let second = if let Some(session) = project.active_session() {
      let focus = session.general_terminal.read(cx).focus_handle(cx);
      PaneContent {
        kind: PaneContentKind::GeneralTerminal,
        view: session.general_terminal.clone().into(),
        focus,
      }
    } else {
      let focus = project.diff_view.read(cx).focus_handle(cx);
      PaneContent { kind: PaneContentKind::GitDiff, view: project.diff_view.clone().into(), focus }
    };

    let third = {
      let focus = project.diff_view.read(cx).focus_handle(cx);
      PaneContent { kind: PaneContentKind::GitDiff, view: project.diff_view.clone().into(), focus }
    };

    vec![first, second, third]
  }

  /// Subscribe to bell events from all sessions' claude terminals.
  fn subscribe_bells(projects: &[ProjectState], cx: &mut Context<Self>) -> Vec<Subscription> {
    let mut subs = Vec::new();
    for (pi, project) in projects.iter().enumerate() {
      for (si, session) in project.sessions.iter().enumerate() {
        subs.push(Self::make_bell_subscription(&session.claude_terminal, pi, si, cx));
      }
    }
    subs
  }

  /// Subscribe to bell events for a single newly-created session.
  fn subscribe_session_bell(&mut self, pi: usize, si: usize, cx: &mut Context<Self>) {
    let terminal = &self.projects[pi].sessions[si].claude_terminal;
    let sub = Self::make_bell_subscription(terminal, pi, si, cx);
    self._bell_subscriptions.push(sub);
  }

  fn make_bell_subscription(
    terminal: &Entity<TerminalView>,
    pi: usize,
    si: usize,
    cx: &mut Context<Self>,
  ) -> Subscription {
    cx.subscribe(
      terminal,
      move |this: &mut Self, _, event: &TerminalViewEvent, cx: &mut Context<Self>| match event {
        TerminalViewEvent::Bell => {
          if let Some(session) = this.projects.get_mut(pi).and_then(|p| p.sessions.get_mut(si)) {
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
          this.open_diff_picker(window, cx);
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
      for session in &project.sessions {
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

  fn close_window(&mut self, _: &CloseWindow, window: &mut Window, _cx: &mut Context<Self>) {
    window.remove_window();
  }

  fn minimize_window(&mut self, _: &MinimizeWindow, window: &mut Window, _cx: &mut Context<Self>) {
    window.minimize_window();
  }

  fn quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
    cx.quit();
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
    let count = self.visible_pane_count();
    self.active_pane_index = (self.active_pane_index + count - 1) % count;
    self.panes[self.active_pane_index].read(cx).focus_content(window);
    cx.notify();
  }

  fn focus_next_pane(&mut self, _: &FocusNextPane, window: &mut Window, cx: &mut Context<Self>) {
    let count = self.visible_pane_count();
    self.active_pane_index = (self.active_pane_index + 1) % count;
    self.panes[self.active_pane_index].read(cx).focus_content(window);
    cx.notify();
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
    if kind == PaneContentKind::ClaudeTerminal {
      let si = self.projects[self.active_project_index].active_session_index;
      if let Some(si) = si
        && let Some(session) = self.projects[self.active_project_index].sessions.get_mut(si)
      {
        session.pending_events.remove(&PendingEvent::TerminalBell);
      }
    }

    // Refresh views that need it when actively switched to.
    if kind == PaneContentKind::GitDiff {
      self.projects[self.active_project_index].diff_view.update(cx, |v, cx| v.refresh(window, cx));
    }
    if kind == PaneContentKind::ReplyViewer
      && let Some(s) = self.projects[self.active_project_index].active_session()
    {
      s.reply_view.update(cx, |v, cx| v.refresh(window, cx));
    }

    let pane_idx = self.active_pane_index;
    self.set_pane_view(pane_idx, kind, cx);

    self.panes[pane_idx].read(cx).focus_content(window);

    // When switching to the TODO editor, auto-scroll to the end of the active session's WAIT body.
    if kind == PaneContentKind::TodoEditor {
      let project = &self.projects[self.active_project_index];
      let tv = project.todo_view.read(cx);
      if let Some(slug) = project.active_slug() {
        let text = tv.editor_text(cx);
        if let Some(wait_line) = tv.document().wait_body_end_line(slug, &text) {
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

  fn show_reply_viewer(
    &mut self,
    _: &ShowReplyViewer,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.set_active_pane_view(PaneContentKind::ReplyViewer, window, cx);
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
    let file_path = match kind {
      Some(PaneContentKind::CodeViewer) => {
        project.code_view.read(cx).file_path().map(|p| p.to_path_buf())
      }
      Some(PaneContentKind::TodoEditor) => {
        Some(project.todo_view.read(cx).file_path().to_path_buf())
      }
      _ => None,
    };
    if let Some(path) = file_path {
      let editor =
        if self.config.editor.is_empty() { "open".to_string() } else { self.config.editor.clone() };
      let _ = std::process::Command::new(&editor).arg(path).spawn();
    }
  }

  // ---------------------------------------------------------------------------
  // Project opening (from IPC)
  // ---------------------------------------------------------------------------

  pub fn open_project(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
    let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());

    // If the project is already loaded, just switch to it.
    if let Some(idx) = self.projects.iter().position(|p| p.path == canonical) {
      let session_idx = self.projects[idx].active_session_index.unwrap_or(0);
      self.switch_to_session(idx, session_idx, window, cx);
      return;
    }

    // Create a new ProjectState and switch to it.
    let palette = palette_from_window(window);
    let name = canonical
      .file_name()
      .map(|n| n.to_string_lossy().into_owned())
      .unwrap_or_else(|| "unknown".into());

    let gc_path = canonical.clone();
    std::thread::spawn(move || gc_stale_replies(&gc_path));

    let project = ProjectState::create(canonical.clone(), name, &palette, window, cx);
    self.projects.push(project);

    let project_idx = self.projects.len() - 1;
    let session_idx = self.projects[project_idx].active_session_index.unwrap_or(0);
    self.switch_to_session(project_idx, session_idx, window, cx);

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
    session_idx: usize,
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
    self.projects[project_idx].active_session_index = Some(session_idx);

    // Acknowledge pending events for this session (user is switching to it).
    if let Some(session) = self.projects[project_idx].sessions.get_mut(session_idx) {
      session.acknowledge();
    }

    // Update the TODO view's active session highlight.
    {
      let slug = self.projects[project_idx].sessions.get(session_idx).map(|s| s.slug.clone());
      let todo_view = self.projects[project_idx].todo_view.clone();
      todo_view.update(cx, |tv, cx| tv.set_active_slug(slug.as_deref(), cx));
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

  /// Find a session by slug across all projects and switch to it.
  fn switch_to_slug(&mut self, slug: &str, window: &mut Window, cx: &mut Context<Self>) {
    for (pi, project) in self.projects.iter().enumerate() {
      for (si, session) in project.sessions.iter().enumerate() {
        if session.slug == slug {
          self.switch_to_session(pi, si, window, cx);
          return;
        }
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
      self.set_pane_view(1, PaneContentKind::GeneralTerminal, cx);
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
          | PaneContentKind::GeneralTerminal
          | PaneContentKind::ReplyViewer => to_rebind.push((i, kind)),
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
      PaneContentKind::ReplyViewer => project.active_session().map(|s| {
        let focus = s.reply_view.read(cx).focus_handle(cx);
        (s.reply_view.clone().into(), focus)
      }),
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

  fn adopt_slug(
    &mut self,
    project_idx: usize,
    slug: &str,
    label: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let todo_view = self.projects[project_idx].todo_view.clone();
    let project_path = self.projects[project_idx].path.clone();

    // Insert heading in TODO.md.
    todo_view.update(cx, |tv, cx| {
      tv.insert_session_heading(slug, label, window, cx);
      tv.save(cx);
    });

    // Build palette and create session state.
    let palette = palette_from_window(window);
    let session = SessionState::create(
      slug.to_string(),
      label.to_string(),
      &project_path,
      &palette,
      window,
      cx,
    );

    let project = &mut self.projects[project_idx];
    project.sessions.push(session);
    let new_idx = project.sessions.len() - 1;
    self.subscribe_session_bell(project_idx, new_idx, cx);
    self.switch_to_session(project_idx, new_idx, window, cx);
  }

  /// Launch a brand new Claude session (no --resume), detect the slug once
  /// the JSONL file appears, and adopt it into TODO.md.
  fn create_new_session(
    &mut self,
    project_idx: usize,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    use jc_core::session::discover_session_groups;
    use std::collections::HashSet;

    let project_path = self.projects[project_idx].path.clone();

    // Snapshot existing slugs so we can detect the new one.
    let existing_slugs: HashSet<String> =
      discover_session_groups(&project_path).into_iter().map(|g| g.slug).collect();

    // Create a session that runs plain `claude` (no resume).
    let palette = palette_from_window(window);
    let session = SessionState::create(
      String::new(), // empty slug -> falls back to plain `claude`
      String::new(),
      &project_path,
      &palette,
      window,
      cx,
    );

    let project = &mut self.projects[project_idx];
    project.sessions.push(session);
    let new_idx = project.sessions.len() - 1;
    self.subscribe_session_bell(project_idx, new_idx, cx);
    self.switch_to_session(project_idx, new_idx, window, cx);

    // Poll for the new JSONL file in the background. Once a new slug appears,
    // update the session and insert a TODO heading.
    cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
      for _ in 0..120 {
        Timer::after(StdDuration::from_millis(500)).await;

        let path = project_path.clone();
        let slugs = existing_slugs.clone();
        let new_slug = std::thread::spawn(move || {
          discover_session_groups(&path)
            .into_iter()
            .find(|g| !slugs.contains(&g.slug))
            .map(|g| g.slug)
        })
        .join()
        .ok()
        .flatten();

        if let Some(slug) = new_slug {
          let _ = this.update_in(cx, |workspace, window, cx| {
            let project = &mut workspace.projects[project_idx];
            project.sessions[new_idx].slug = slug.clone();
            project.sessions[new_idx].label = slug.clone();

            // Update the reply view to track the new slug.
            project.sessions[new_idx].reply_view.update(cx, |rv, cx| {
              rv.set_session_slug(Some(slug.clone()), window, cx);
            });

            // Insert TODO heading.
            let todo_view = project.todo_view.clone();
            todo_view.update(cx, |tv, cx| {
              tv.insert_session_heading(&slug, &slug, window, cx);
              tv.save(cx);
            });

            cx.notify();
          });
          return;
        }
      }
    })
    .detach();
  }

  /// Activate an empty project: discover existing JSONL sessions and adopt the
  /// most recent one, or create a brand new Claude session if none exist.
  fn init_empty_project(
    &mut self,
    project_idx: usize,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    use jc_core::session::discover_latest_session_group;

    let project_path = self.projects[project_idx].path.clone();

    if let Some(group) = discover_latest_session_group(&project_path) {
      // Existing JSONL files found — adopt the most recent slug.
      let slug = group.slug.clone();
      let label = group.summary().unwrap_or_else(|| slug.clone());
      self.adopt_slug(project_idx, &slug, &label, window, cx);
    } else {
      // No JSONL files — launch a fresh Claude session.
      self.create_new_session(project_idx, window, cx);
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
    let project = &self.projects[self.active_project_index];
    let Some(slug) = project.active_slug().map(str::to_string) else {
      return;
    };
    let Some(session) = project.active_session() else {
      return;
    };
    let claude_terminal = session.claude_terminal.clone();

    let Some((message_text, _)) =
      project.todo_view.update(cx, |tv, cx| tv.send_selection(&slug, window, cx))
    else {
      return;
    };

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
  // Hook events
  // ---------------------------------------------------------------------------

  fn handle_hook_event(&mut self, event: HookEvent, cx: &mut Context<Self>) {
    eprintln!("hook: {:?} session={} slug={:?}", event.kind, event.session_id, event.slug);

    let pending = match event.kind {
      HookEventKind::Stop => PendingEvent::ClaudeStop,
      HookEventKind::PermissionPrompt => PendingEvent::ClaudePermission,
      HookEventKind::IdlePrompt => PendingEvent::ClaudeIdle,
    };

    // Match the slug to a session across all projects.
    let mut matched_project: Option<String> = None;
    let mut matched_label: Option<String> = None;
    if let Some(slug) = &event.slug {
      for project in &mut self.projects {
        for session in &mut project.sessions {
          if &session.slug == slug {
            let is_new = session.pending_events.insert(pending.clone());
            if is_new {
              matched_project = Some(project.name.clone());
              matched_label = Some(session.label.clone());
            }
            break;
          }
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
      };
      crate::notify::notify(&title, message, critical, event.slug.as_deref());
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
    KeyBinding::new("cmd-.", crate::views::picker::ShowViewPicker, Some("Workspace")),
    KeyBinding::new("cmd-shift-e", OpenInExternalEditor, Some("Workspace")),
    KeyBinding::new("cmd-shift-o", OpenGitLogPicker, Some("Workspace")),
    KeyBinding::new("cmd-p", crate::views::picker::ShowSessionPicker, Some("Workspace")),
    KeyBinding::new("cmd-shift-p", crate::views::picker::ShowSlugPicker, Some("Workspace")),
    KeyBinding::new("cmd-k", OpenCommentPanel, Some("Workspace")),
    KeyBinding::new("cmd-s", SaveFile, Some("Workspace")),
    KeyBinding::new("cmd-enter", SendToTerminal, Some("Workspace")),
    KeyBinding::new("cmd-;", NextProblem, Some("Workspace")),
    KeyBinding::new("cmd-:", crate::views::picker::ShowProblemPicker, Some("Workspace")),
    KeyBinding::new("cmd-shift-k", crate::views::picker::ShowSnippetPicker, Some("Workspace")),
    KeyBinding::new("cmd-?", ShowKeybindingHelp, Some("Workspace")),
  ]);

  cx.bind_keys([
    KeyBinding::new("cmd-.", crate::views::picker::ShowViewPicker, Some("Input")),
    KeyBinding::new("cmd-[", FocusPrevPane, Some("Input")),
    KeyBinding::new("cmd-]", FocusNextPane, Some("Input")),
    KeyBinding::new("cmd-k", OpenCommentPanel, Some("Input")),
    KeyBinding::new("cmd-s", SaveFile, Some("Input")),
    KeyBinding::new("cmd-enter", SendToTerminal, Some("Input")),
    KeyBinding::new("cmd-shift-k", crate::views::picker::ShowSnippetPicker, Some("Input")),
  ]);
}
