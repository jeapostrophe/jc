mod own;
mod pickers;
mod render;
mod sessions;
mod state_file;

use crate::views::close_confirm::{CloseConfirm, CloseConfirmEvent, Closing};
use crate::views::keybinding_help::{DismissHelpEvent, KeybindingHelp};
use crate::views::pane::{Pane, PaneContent, PaneContentKind, default_kinds};
use crate::views::project_state::{ProjectState, SavedPaneLayout};
use crate::views::session_state::{SavedViewState, SessionId, SessionState};
use crate::views::todo_view::ScheduledFire;
use chrono::NaiveDateTime;
use gpui::*;
use gpui_component::input::InputState;
use gpui_component::theme::Theme;
use jc_core::background::Claude;
use jc_core::config::{AppConfig, AppState};
use jc_core::flavor::Flavor;
use jc_core::hooks::{HookEvent, HookEventKind, HookServer};
use jc_core::hooks_settings::HookJob;
use jc_core::theme::Appearance;
use jc_core::todo::{SessionKey, TodoDocument};
use jc_terminal::{Palette, TerminalView};
use std::collections::VecDeque;
use std::ops::DerefMut;
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;
use std::time::SystemTime;

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
    ToggleScratch,
    OpenCiraPage,
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
  /// What the pending close confirmation closes.
  closing: Closing,
  /// Watches `state.toml` (Cira PLAN TMUX-10).
  _state_watcher: Option<notify::RecommendedWatcher>,
  /// The newest `state.toml` read; older ones are not applied.
  state_reads: jc_core::config::Reads,
  /// Bumped at each change of `state.toml`, which is read once it holds
  /// still for `STATE_SETTLE`.
  state_changes: u64,
  /// Which app this is (Cira PLAN TMUX-07): `jc` runs each session's
  /// `claude` itself (`own.rs`); `jcc` is Cira's client (`sessions.rs`).
  flavor: Flavor,
  /// jcc: `claude attach` and the read-only `claude agents --json`, all jcc
  /// runs of `claude` itself. Everything else about a session is asked of Cira.
  claude: Claude,
  /// jcc: the `cira` CLI every lifecycle request goes through. `None` in jc,
  /// which never asks Cira for anything.
  cira_client: Option<jc_core::cira::Cira>,
  /// Cira as last read, and what jc remembers of its sessions.
  cira: sessions::CiraView,
  /// jcc: asks the Cira poll thread for a read now.
  cira_kick: Option<flume::Sender<()>>,
  /// jcc: keeps `sessions.toml`, on a thread of its own.
  records_tx: Option<flume::Sender<jc_core::lifecycle::Records>>,
  /// Hooks from a known project that matched no session, kept until the next
  /// Cira read teaches jc the uuid they carry.
  unmatched_hooks: VecDeque<HookEvent>,
  /// jcc: receives each `cira status --json` read by the Cira poll thread.
  _cira_poll_task: Option<Task<()>>,
  /// The last project whose hooks could not be installed, and why: shown
  /// until jc restarts, since its sessions send this app nothing.
  hook_warning: Option<String>,
  _hook_failure_task: Option<Task<()>>,
  /// Installs and removes projects' hooks, in order, on its own thread; with
  /// the hook server.
  hook_jobs: Option<(flume::Sender<HookJob>, std::thread::JoinHandle<()>)>,
  /// Why the last action was refused, shown for a few seconds
  /// (`Workspace::refuse`), with a number so a later refusal's timer is not
  /// cleared by an earlier one's.
  refusal: Option<(u64, String)>,
  /// jcc: per session (project index, session), what the right pane showed
  /// before Cmd-\ put Scratch there (Cira PLAN TMUX-24).
  scratch_returns: crate::views::pane::ScratchReturns<(usize, Option<SessionId>)>,
  /// jcc: the Dock badge as last set.
  dock_badge: Option<String>,
}

/// What a pane shows as `kind` for `project`'s active session; `None` for a
/// session's view when it has none.
fn pane_content(
  kind: PaneContentKind,
  project: &ProjectState,
  global_todo_view: &Entity<crate::views::code_view::CodeView>,
  cx: &App,
) -> Option<PaneContent> {
  let (view, focus): (AnyView, FocusHandle) = match kind {
    PaneContentKind::ClaudeTerminal => project
      .active_session()
      .map(|s| (s.claude_terminal.clone().into(), s.claude_terminal.read(cx).focus_handle(cx)))?,
    PaneContentKind::GeneralTerminal => project
      .active_session()
      .map(|s| (s.general_terminal.clone().into(), s.general_terminal.read(cx).focus_handle(cx)))?,
    PaneContentKind::CodeViewer => project
      .active_session()
      .map(|s| (s.code_view.clone().into(), s.code_view.read(cx).focus_handle(cx)))?,
    PaneContentKind::TodoEditor => {
      (project.todo_view.clone().into(), project.todo_view.read(cx).focus_handle(cx))
    }
    PaneContentKind::GlobalTodo => {
      (global_todo_view.clone().into(), global_todo_view.read(cx).focus_handle(cx))
    }
    PaneContentKind::Scratch => {
      (project.scratch_view.clone().into(), project.scratch_view.read(cx).focus_handle(cx))
    }
  };
  Some(PaneContent { kind, view, focus })
}

/// Unmatched hooks kept for re-dispatch; older ones are dropped.
const UNMATCHED_HOOKS_MAX: usize = 32;
/// How long a refusal's reason stays on screen.
const REFUSAL_SHOWN: StdDuration = StdDuration::from_secs(8);
/// The pause between two reads of `cira status --json`.
const CIRA_POLL: StdDuration = StdDuration::from_secs(2);

impl Workspace {
  pub fn new(
    state: AppState,
    config: AppConfig,
    ipc_rx: flume::Receiver<PathBuf>,
    added: Option<PathBuf>,
    home: Option<PathBuf>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let palette = palette_from_window(window);
    let flavor = Flavor::current();
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
    for project in &mut projects {
      project.added = added.as_ref() == Some(&project.path);
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
    let initial_contents = Self::initial_pane_contents(&projects[0], &global_todo_view, flavor, cx);
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
      cx.observe_window_activation(window, move |this: &mut Self, window, cx| {
        this.window_active = window.is_window_active();
        // The daemon announces no change to a pad: read it again on coming back.
        if this.window_active {
          let scratch = this.active_project().scratch_view.clone();
          scratch.update(cx, |v, cx| v.refresh(window, cx));
        }
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

    let (hook_failures_tx, hook_failures) = flume::unbounded::<String>();
    // Start hook server for Claude Code integration.
    let project_paths: Vec<PathBuf> = projects.iter().map(|p| p.path.clone()).collect();
    let mut hook_jobs = None;
    let (hook_server, hook_poll_task) = match HookServer::start(flavor, project_paths.clone()) {
      Ok(server) => {
        // Install hooks into each project's settings, off the main thread, in
        // order with later installs and removals (`hook_worker`). A failure
        // (a settings.local.json that does not parse is left alone) means no
        // markers or notifications from that project: the window says so
        // (`hook_warning`).
        let failed = hook_failures_tx.clone();
        let (jobs, worker) =
          jc_core::hooks_settings::hook_worker(flavor, server.port, move |why: String| {
            eprintln!("{why}");
            let _ = failed.send(why);
          });
        for path in &project_paths {
          let _ = jobs.send(HookJob::Install(path.clone()));
        }
        hook_jobs = Some((jobs, worker));
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
              if Flavor::current().ipc_switches() {
                entity.update(cx, |ws, cx| ws.open_project(path, window, cx));
                window.activate_window();
              } else {
                entity.update(cx, |ws, cx| ws.list_project(path, window, cx));
              }
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

    // jcc only: Cira's counts, session states and lifecycle (Cira PLAN
    // TMUX-01/05/06), read on the poll's own thread, each read stamped with
    // when it began; only applying a read runs here. jc has no client, so no
    // poll: it never runs `cira`.
    let cira_client = (flavor == Flavor::Jcc).then(jc_core::cira::Cira::default);
    let (cira_kick, cira_poll_task) = match cira_client.clone() {
      Some(reader) => {
        let (cira_rx, cira_kick) =
          jc_core::cira::poll(CIRA_POLL, move || (SystemTime::now(), reader.status()));
        let task =
          cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            while let Ok((read_at, status)) = cira_rx.recv_async().await {
              let applied =
                this.update_in(cx, |ws, window, cx| ws.on_cira(read_at, status, window, cx));
              if applied.is_err() {
                break; // workspace dropped; dropping the receiver ends the poll thread
              }
            }
          });
        (Some(cira_kick), Some(task))
      }
      None => (None, None),
    };

    // jcc: `sessions.toml` is read and written off the main thread: the saver
    // writes the latest records it is sent. jc keeps no records, so has no saver.
    let records_tx = (flavor == Flavor::Jcc).then(|| {
      let (records_tx, records_rx) = flume::unbounded::<jc_core::lifecycle::Records>();
      std::thread::spawn(move || {
        while let Ok(mut records) = records_rx.recv() {
          while let Ok(newer) = records_rx.try_recv() {
            records = newer;
          }
          if let Err(e) = jc_core::config::save_sessions(&records) {
            eprintln!("sessions.toml: {e:#}");
          }
        }
      });
      records_tx
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
      closing: Closing::Window,
      _state_watcher: None,
      state_reads: Default::default(),
      state_changes: 0,
      flavor,
      claude,
      cira_client,
      cira: sessions::CiraView { home, ..Default::default() },
      cira_kick,
      records_tx,
      unmatched_hooks: VecDeque::new(),
      _cira_poll_task: cira_poll_task,
      scratch_returns: Default::default(),
      dock_badge: None,
      hook_warning: None,
      _hook_failure_task: None,
      hook_jobs,
      refusal: None,
    };

    if flavor == Flavor::Jcc {
      ws.start_cira_client(window, cx);
    }
    ws._hook_failure_task =
      Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
        while let Ok(failure) = hook_failures.recv_async().await {
          let shown = this.update(cx, |ws, cx| {
            ws.hook_warning = Some(failure);
            cx.notify();
          });
          if shown.is_err() {
            break;
          }
        }
      }));
    ws._state_watcher = ws.watch_state_file(window, cx);
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
    flavor: Flavor,
    cx: &App,
  ) -> Vec<PaneContent> {
    let kinds = default_kinds(flavor, project.active_session().is_some());
    kinds
      .into_iter()
      .map(|kind| {
        pane_content(kind, project, global_todo_view, cx)
          .expect("default_kinds names a session's view only when there is a session")
      })
      .collect()
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
  /// are buffers with merge conflicts that can't be saved, or, in jc, active
  /// sessions, which quitting terminates. In jcc working sessions are no
  /// reason to ask: closing only detaches from background sessions, which
  /// keep running.
  fn try_close(&mut self, is_quit: bool, window: &mut Window, cx: &mut Context<Self>) {
    let conflicts = self.save_all_dirty(window, cx);
    let active = self.active_session_count();

    if conflicts.is_empty() && active == 0 {
      // Nothing to warn about — just close.
      if is_quit {
        cx.quit();
      } else {
        window.remove_window();
      }
    } else {
      let closing = if is_quit { Closing::Quit } else { Closing::Window };
      self.show_close_confirm(active, conflicts, closing, window, cx);
    }
  }

  /// jc: count sessions that are actively working (not idle/stopped). Always
  /// 0 in jcc, where quitting stops nothing.
  fn active_session_count(&self) -> usize {
    if self.flavor != Flavor::Jc {
      return 0;
    }
    self.projects.iter().flat_map(|p| p.sessions.values()).filter(|s| s.busy).count()
  }

  /// Refuse an action: the beep, and why, in a banner for a few seconds.
  pub(super) fn refuse(&mut self, why: impl Into<String>, cx: &mut Context<Self>) {
    crate::notify::beep();
    self.notice(why, cx);
  }

  /// Say something in the window's banner for a few seconds.
  pub(super) fn notice(&mut self, why: impl Into<String>, cx: &mut Context<Self>) {
    let n = self.refusal.as_ref().map_or(0, |(n, _)| n + 1);
    self.refusal = Some((n, why.into()));
    cx.notify();
    cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
      Timer::after(REFUSAL_SHOWN).await;
      let _ = this.update(cx, |ws, cx| {
        if ws.refusal.as_ref().is_some_and(|(m, _)| *m == n) {
          ws.refusal = None;
          cx.notify();
        }
      });
    })
    .detach();
  }

  /// Save all dirty buffers (TODO views, code views, global TODO).
  /// Returns the files with unsaved edits that could not be saved: a merge
  /// conflict, which nothing saves over until it is reloaded, or a failed write.
  fn save_all_dirty(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<String> {
    let mut unsaved = Vec::new();
    for pi in 0..self.projects.len() {
      unsaved.extend(self.save_dirty_of(pi, window, cx));
    }
    if self.global_todo_view.read(cx).is_dirty(cx)
      && !self.global_todo_view.update(cx, |v, cx| v.save(window, cx))
    {
      unsaved.push("~/.claude/TODO.md".to_string());
    }
    unsaved
  }

  /// Save project `pi`'s dirty buffers (its TODO and code views); the files
  /// that could not be saved (a conflict, a failed write).
  fn save_dirty_of(&self, pi: usize, window: &mut Window, cx: &mut Context<Self>) -> Vec<String> {
    let project = &self.projects[pi];
    let named = |path: Option<&Path>| {
      path.map(|p| p.strip_prefix(&project.path).unwrap_or(p).display().to_string())
    };
    let mut unsaved = Vec::new();
    if project.todo_view.read(cx).is_dirty(cx)
      && !project.todo_view.update(cx, |tv, cx| tv.save(window, cx))
    {
      unsaved.extend(named(project.todo_view.read(cx).code_view().read(cx).file_path()));
    }
    if !project.scratch_view.update(cx, |v, cx| v.flush(cx)) {
      unsaved.push(format!("the scratch pad of {}", project.name));
    }
    for session in project.sessions.values() {
      if session.code_view.read(cx).is_dirty(cx)
        && !session.code_view.update(cx, |v, cx| v.save(window, cx))
      {
        unsaved.extend(named(session.code_view.read(cx).file_path()));
      }
    }
    unsaved
  }

  fn show_close_confirm(
    &mut self,
    session_count: usize,
    conflicts: Vec<String>,
    closing: Closing,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.closing = closing.clone();
    self.pre_close_confirm_focus = window.focused(cx);
    let view = cx.new(|cx| CloseConfirm::new(session_count, conflicts, closing, cx));
    let sub = cx.subscribe_in(&view, window, |this: &mut Self, _, event, window, cx| match event {
      CloseConfirmEvent::Confirmed => {
        this.close_confirm = None;
        match this.closing.clone() {
          Closing::Quit => cx.quit(),
          Closing::Window => window.remove_window(),
          Closing::Project(path) => {
            this.restore_close_confirm_focus(window);
            this.close_project(&path, window, cx);
            // Removals put off while the dialog was up are asked now.
            this.read_state_file(window, cx);
          }
        }
        this.pre_close_confirm_focus = None;
      }
      CloseConfirmEvent::Cancelled => {
        this.close_confirm = None;
        match this.closing.clone() {
          // Its own write of state.toml brings the next read.
          Closing::Project(path) => this.keep_project(path, window, cx),
          _ => this.read_state_file(window, cx),
        }
        this.restore_close_confirm_focus(window);
        cx.notify();
      }
    });
    view.read(cx).focus_handle(cx).focus(window);
    self.close_confirm = Some((view.into(), sub));
    cx.notify();
  }

  fn restore_close_confirm_focus(&mut self, window: &mut Window) {
    if let Some(focus) = self.pre_close_confirm_focus.take() {
      focus.focus(window);
    }
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
      PaneContentKind::Scratch => {
        Some(OtherPaneScrollable::Editor(project.scratch_view.read(cx).editor().clone()))
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

    if kind == PaneContentKind::Scratch {
      let scratch = self.projects[self.active_project_index].scratch_view.clone();
      scratch.update(cx, |v, cx| v.refresh(window, cx));
    }

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
    self.kick_cira(); // a shell pane shown for the first time asks for its shell
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
    let (idx, _) = self.name_project(path, window, cx);
    let active = self.projects[idx].active_session;
    self.switch_to_session(idx, active, window, cx);
  }

  /// jcc's `jcc DIR` while it runs (Cira PLAN TMUX-13): add the project to
  /// the list without showing it; Cmd-P reaches it.
  pub fn list_project(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
    let (idx, added) = self.name_project(path, window, cx);
    let name = self.projects[idx].path.display().to_string();
    match added {
      true => self.notice(format!("Added {name} to the project list (Cmd-P)"), cx),
      false => self.notice(format!("{name} is already in the project list (Cmd-P)"), cx),
    }
  }

  /// The project `path` names on the command line (`jc DIR`), added to the
  /// list and to `state.toml` when it is not there: its index, and whether it
  /// was added. Named again, a listed project may register a Cira project
  /// again.
  fn name_project(
    &mut self,
    path: PathBuf,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> (usize, bool) {
    let canonical = std::fs::canonicalize(&path).unwrap_or(path);
    if let Some(idx) = self.projects.iter().position(|p| p.path == canonical) {
      self.projects[idx].added = true;
      self.cira.registering.remove(&canonical);
      self.kick_cira();
      return (idx, false);
    }
    let idx = self.add_project(canonical.clone(), window, cx);
    if let Ok(mut state) = jc_core::config::load_state() {
      state.register_project(&canonical);
      let _ = jc_core::config::save_state(&state);
    }
    (idx, true)
  }

  /// Serve and install the hooks of a project opened while the app runs, as
  /// `Workspace::new` does for those open at start, off the main thread.
  fn install_project_hooks(&self, path: PathBuf) {
    let (Some(server), Some((jobs, _))) = (&self._hook_server, &self.hook_jobs) else { return };
    server.add_project(path.clone());
    let _ = jobs.send(HookJob::Install(path));
  }

  /// Stop serving a closed project's hooks and remove them, after any
  /// install still queued for it.
  pub(super) fn remove_project_hooks(&self, path: PathBuf) {
    if let Some(server) = &self._hook_server {
      server.remove_project(&path);
    }
    if let Some((jobs, _)) = &self.hook_jobs {
      let _ = jobs.send(HookJob::Uninstall(path));
    }
  }

  /// Open the project at `canonical` without switching to it; its index.
  fn add_project(
    &mut self,
    canonical: PathBuf,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> usize {
    let palette = palette_from_window(window);
    let name = canonical
      .file_name()
      .map(|n| n.to_string_lossy().into_owned())
      .unwrap_or_else(|| "unknown".into());
    self.install_project_hooks(canonical.clone());
    let mut project = ProjectState::create(canonical, name, &palette, window, cx);
    project.added = true;
    self.projects.push(project);
    let project_idx = self.projects.len() - 1;
    if self.flavor == Flavor::Jcc {
      self.observe_panes(project_idx, window, cx);
      self.kick_cira();
    }
    project_idx
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
      // Whatever this session printed (jc) or finished (jcc) while it was on
      // screen has been seen, so it must not come back marked in the Cmd-P
      // picker. Doing it on the way OUT rather than on the way in is what makes
      // that true regardless of how the switch was made (picker, Cmd-`, a
      // notification click).
      match self.flavor {
        Flavor::Jc => session.claude_terminal.read(cx).clear_output_seen(),
        Flavor::Jcc => session.seen = Some(SystemTime::now()),
      }
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

    if self.flavor == Flavor::Jcc {
      self.cira.focus_settled = true;
      self.note_focus();
    }
    cx.notify();
  }

  fn rotate_next_project(
    &mut self,
    _: &RotateNextProject,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.flavor == Flavor::Jcc && !self.cira_reachable(cx) {
      let why = crate::views::cira_banner(cx).unwrap_or_else(|| "Cira is unreachable".into());
      self.refuse(why, cx);
      return;
    }
    // Build a flat list of (project_index, session_id) in deterministic order.
    let mut slots: Vec<(usize, Option<SessionId>)> = Vec::new();
    for (pi, project) in self.projects.iter().enumerate() {
      if !rotates(self.flavor, project) {
        continue;
      }
      let mut ids: Vec<SessionId> = project.sessions.keys().copied().collect();
      ids.sort();
      for id in ids {
        slots.push((pi, Some(id)));
      }
    }
    let current =
      (self.active_project_index, self.projects[self.active_project_index].active_session);
    if let Some((next_pi, next_sid)) = next_slot(self.flavor, &slots, current) {
      self.switch_to_session(next_pi, next_sid, window, cx);
    }
  }

  /// jcc's Cmd-\ (Cira PLAN TMUX-24): the right pane shows Scratch, or, when it
  /// does, what it showed before (else its default). Focus stays where it is,
  /// unless it was in that pane.
  fn toggle_scratch(&mut self, _: &ToggleScratch, window: &mut Window, cx: &mut Context<Self>) {
    let idx = self.visible_pane_count() - 1;
    let shown = self.panes[idx].read(cx).content_kind();
    let session = self.active_project().active_session;
    let fallback = default_kinds(self.flavor, session.is_some())[idx];
    let to =
      self.scratch_returns.toggle((self.active_project_index, session), idx, shown, fallback);
    if self.focused_pane_index(window, cx) == Some(idx) {
      self.show_in_pane(idx, to, window, cx);
    } else {
      self.set_pane_view(idx, to, cx);
      if to == PaneContentKind::Scratch {
        let scratch = self.active_project().scratch_view.clone();
        scratch.update(cx, |v, cx| v.refresh(window, cx));
      }
      cx.notify();
    }
    if to == PaneContentKind::GeneralTerminal {
      self.kick_cira(); // a shell pane shown for the first time asks for its shell
    }
  }

  /// jcc's Cmd-, (Cira PLAN TMUX-23): Cira's page on this project, in Safari.
  fn open_cira_page(&mut self, _: &OpenCiraPage, window: &mut Window, cx: &mut Context<Self>) {
    if !self.cira_reachable(cx) {
      let why = crate::views::cira_banner(cx).unwrap_or_else(|| "Cira is unreachable".into());
      self.refuse(why, cx);
      return;
    }
    let path = &self.active_project().path;
    let Some(url) =
      self.cira.status.as_ref().and_then(|s| Some(s.page_url(&s.project_for(path)?.name)))
    else {
      self.refuse("Cira's page: no Cira project holds this one", cx);
      return;
    };
    self.run_blocking(
      window,
      cx,
      move || {
        let opened = std::process::Command::new("open")
          .args(["-a", "/Applications/Safari.app", &url])
          .stdin(std::process::Stdio::null())
          .output();
        match opened {
          Ok(out) if out.status.success() => None,
          Ok(out) => Some(format!("Safari: {}", String::from_utf8_lossy(&out.stderr).trim())),
          Err(e) => Some(format!("Safari: open: {e}")),
        }
      },
      |ws, failed, _, cx| {
        if let Some(why) = failed {
          ws.refuse(why, cx);
        }
      },
    );
  }

  /// The open blocks of every session jcc has attached, the title bar's first
  /// count (Cira PLAN TMUX-18).
  pub(super) fn attached_blocks(&self) -> u64 {
    self.projects.iter().flat_map(|p| p.sessions.values()).map(SessionState::open_blocks).sum()
  }

  /// Keep the Dock badge on Cira's last read, `status` (`None`: unreachable).
  pub(super) fn update_dock_badge(&mut self, status: Option<&jc_core::cira::Status>) {
    let badge = jc_core::cira::dock_badge(status, self.projects.iter().map(|p| p.path.as_path()));
    if self.dock_badge != badge {
      crate::notify::set_badge(badge.as_deref());
      self.dock_badge = badge;
    }
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
      let kinds = default_kinds(self.flavor, self.active_project().active_session().is_some());
      for (i, kind) in kinds.into_iter().enumerate() {
        self.set_pane_view(i, kind, cx);
      }
      if kinds.contains(&PaneContentKind::GeneralTerminal) {
        self.kick_cira(); // a shell pane shown for the first time asks for its shell
      }
      self.panes[0].read(cx).focus_content(window);
      self.active_pane_index = 0;
    }
  }

  /// Set a specific pane to show a view kind from the active project/session.
  fn set_pane_view(&mut self, pane_idx: usize, kind: PaneContentKind, cx: &mut App) {
    let project = &self.projects[self.active_project_index];
    if let Some(content) = pane_content(kind, project, &self.global_todo_view, cx) {
      self.panes[pane_idx].update(cx, |p, cx| p.set_content(content, cx));
    }
  }

  // ---------------------------------------------------------------------------
  // Session lifecycle: jc's own (`own.rs`) or Cira's (`sessions.rs`)
  // ---------------------------------------------------------------------------

  /// A brand new session: jc mints a uuid and runs `claude` on it; jcc asks
  /// Cira for one.
  fn create_new_session(&mut self, pi: usize, window: &mut Window, cx: &mut Context<Self>) {
    match self.flavor {
      Flavor::Jc => self.own_new_session(pi, window, cx),
      Flavor::Jcc => self.cira_new_session(pi, window, cx),
    }
  }

  /// Continue transcript `uuid`, which no heading names, under a new heading.
  fn adopt_transcript(
    &mut self,
    pi: usize,
    uuid: String,
    summary: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    match self.flavor {
      Flavor::Jc => self.own_adopt_transcript(pi, uuid, summary, window, cx),
      Flavor::Jcc => self.cira_adopt_transcript(pi, uuid, summary, window, cx),
    }
  }

  /// Give the heading `key` names, which has no pane, its session.
  fn adopt_session(
    &mut self,
    pi: usize,
    key: &SessionKey,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    match self.flavor {
      Flavor::Jc => self.own_adopt_session(pi, key, window, cx),
      Flavor::Jcc => self.cira_adopt_session(pi, key, window, cx),
    }
  }

  /// Toggle `[D]` on the heading `key` names.
  fn toggle_session_disabled(
    &mut self,
    pi: usize,
    key: &SessionKey,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    match self.flavor {
      Flavor::Jc => self.own_toggle_session_disabled(pi, key, window, cx),
      Flavor::Jcc => self.cira_toggle_session_disabled(pi, key, window, cx),
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
      // removed session's, and stamp it as seen.
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
          cv.update(cx, |v, cx| v.save(window, cx));
        }
      }
      Some(PaneContentKind::TodoEditor) => {
        project.todo_view.update(cx, |v, cx| v.save(window, cx));
      }
      Some(PaneContentKind::GlobalTodo) => {
        self.global_todo_view.update(cx, |v, cx| v.save(window, cx));
      }
      Some(PaneContentKind::Scratch) => {
        project.scratch_view.update(cx, |v, cx| v.save_now(window, cx));
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
    let ready = Self::ready_for_input(self.flavor, session, cx);

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
      // jc: mark busy (quitting asks first) and end the launch-settle window.
      if self.flavor == Flavor::Jc
        && let Some(session) = self.projects[self.active_project_index].active_session_mut()
      {
        session.mark_user_input(cx);
      }
      Self::deliver_to_terminal(&claude_terminal, &message_text, cx);
      cx.notify();
    }
  }

  /// Text typed into `session`'s Claude pane reaches the session. jc: always,
  /// its `claude` runs in the pane. jcc: the session process runs, and the
  /// attach client has drawn its first frame since it (re)started.
  fn ready_for_input(flavor: Flavor, session: &SessionState, cx: &App) -> bool {
    match flavor {
      Flavor::Jc => true,
      Flavor::Jcc => session.live && session.claude_terminal.read(cx).has_output_since_spawn(),
    }
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

  /// Arm a timer to deliver a scheduled message at `when`. In jcc a past-due
  /// one fires at once: only sessions ready for input are armed
  /// ([`Self::reconcile_schedules`]), so a catch-up send at startup waits for
  /// the session to come up. In jc a past-due one (a catch-up send re-armed at
  /// startup) fires after a short grace, so a freshly-spawned Claude terminal
  /// has time to reach its prompt. The timer re-reads the live TODO at fire
  /// time, so a cancelled or rescheduled marker is handled there rather than
  /// tracked here.
  fn arm_scheduled_send(
    &mut self,
    project_path: PathBuf,
    uuid: String,
    when: NaiveDateTime,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    /// jc: grace before firing a past-due send, so a just-launched terminal is ready.
    const CATCHUP_GRACE: StdDuration = StdDuration::from_secs(5);

    let now = chrono::Local::now().naive_local();
    let raw = (when - now).to_std().unwrap_or(StdDuration::ZERO);
    let delay = if raw.is_zero() && self.flavor == Flavor::Jc { CATCHUP_GRACE } else { raw };
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
    let flavor = self.flavor;
    let Some((id, claude_terminal)) = self.projects[project_idx]
      .session_by_key(uuid)
      .filter(|(_, s)| Self::ready_for_input(flavor, s, cx))
      .map(|(id, s)| (id, s.claude_terminal.clone()))
    else {
      return;
    };

    let todo_view = self.projects[project_idx].todo_view.clone();
    let now = chrono::Local::now().naive_local();

    match todo_view.update(cx, |tv, cx| tv.deliver_scheduled(uuid, now, window, cx)) {
      ScheduledFire::Deliver(body) => {
        Self::deliver_to_terminal(&claude_terminal, &body, cx);
        if flavor == Flavor::Jc
          && let Some(session) = self.projects[project_idx].sessions.get_mut(&id)
        {
          session.mark_user_input(cx);
        }
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
    let flavor = self.flavor;
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
            project.session_by_key(&s.id).is_some_and(|(_, r)| Self::ready_for_input(flavor, r, cx))
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
  /// waiting for the original, possibly later, timer to fire) and each running
  /// session's label (so a heading renamed in TODO.md takes effect). It does
  /// no I/O. What jc asks of Cira follows each Cira read (`Workspace::on_cira`).
  fn start_schedule_reconcile_loop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let task = cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| {
      loop {
        Timer::after(StdDuration::from_secs(2)).await;
        let stepped = this.update_in(cx, |ws, window, cx| {
          ws.reconcile_schedules(window, cx);
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
    // jc: a session start says nothing it needs (its panes run `claude`
    // themselves).
    if self.flavor == Flavor::Jc && matches!(event.kind, HookEventKind::SessionStart) {
      return;
    }
    eprintln!("hook: {:?} session={}", event.kind, event.session_id);

    // Handle session clear: follow the session onto its new uuid (jc also
    // rewrites the heading's `> uuid=`).
    if let HookEventKind::SessionClear { ref old_session_id, ref new_session_id } = event.kind {
      match self.flavor {
        Flavor::Jc => self.own_session_clear(old_session_id, new_session_id, window, cx),
        Flavor::Jcc => self.handle_session_clear(old_session_id, new_session_id),
      }
      cx.notify();
      return;
    }

    if self.flavor == Flavor::Jc {
      // Every remaining event is addressed to a session identified by its
      // uuid; one jc did not launch is not jc's.
      let Some((project_name, session_label)) = self.own_apply_hook(&event, cx) else {
        cx.notify();
        return;
      };
      self.notify_blocked(&event, &project_name, &session_label);
      cx.notify();
      return;
    }

    let Some((project_name, session_label)) = self.apply_hook_to_session(&event) else {
      // From a project jc has open but no session it knows: most likely one of
      // its own whose uuid or job changed while jc was not looking (a `/clear`
      // in another attach, a fork). Try again after the next Cira read.
      if event.project_path.as_ref().is_some_and(|p| self.project_index(p).is_some()) {
        if self.unmatched_hooks.len() == UNMATCHED_HOOKS_MAX {
          self.unmatched_hooks.pop_front();
        }
        self.unmatched_hooks.push_back(event);
      }
      return;
    };
    self.notify_blocked(&event, &project_name, &session_label);
    cx.notify();
  }

  /// Notify when the window is not active (user is in another app). Only a
  /// blocked session is worth interrupting for; Stop/IdlePrompt are ambient.
  fn notify_blocked(&self, event: &HookEvent, project_name: &str, session_label: &str) {
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
    session.note_hook(&event.kind, event.received);
    Some((project_name, session.label.clone()))
  }

  /// The `(project index, session)` a hook naming `hook_session` comes from.
  fn session_for_hook(&self, hook_session: &str) -> Option<(usize, SessionId)> {
    self.projects.iter().enumerate().find_map(|(pi, p)| {
      p.sessions.iter().find(|(_, s)| s.owns_hook(hook_session)).map(|(&sid, _)| (pi, sid))
    })
  }

  /// Dispatch again each hook that matched no session, now that a Cira read
  /// has taught jc the sessions' current uuids and jobs. Each is tried once
  /// more; one still unmatched is not jc's.
  fn retry_unmatched_hooks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    for event in std::mem::take(&mut self.unmatched_hooks) {
      if self.session_for_hook(&event.session_id).is_some() {
        self.handle_hook_event(event, window, cx);
      }
    }
  }

  /// Handle a `/clear` event: the session's conversation moved to a new uuid
  /// in the same Claude process. Only the in-memory uuid follows it: TODO.md
  /// names the session by its Cira id, which a `/clear` keeps.
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
    // After every queued job, so no install queued earlier runs after these.
    if let Some((jobs, worker)) = self.hook_jobs.take() {
      for project in &self.projects {
        let _ = jobs.send(HookJob::Uninstall(project.path.clone()));
      }
      drop(jobs);
      let _ = worker.join();
    } else {
      for project in &self.projects {
        let _ = jc_core::hooks_settings::uninstall_hooks(self.flavor, &project.path);
      }
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
  if Flavor::current() == Flavor::Jcc {
    for context in ["Workspace", "Input"] {
      cx.bind_keys([
        KeyBinding::new("cmd-\\", ToggleScratch, Some(context)),
        KeyBinding::new("cmd-,", OpenCiraPage, Some(context)),
      ]);
    }
  }
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

/// Whether Cmd-` stops at `project`'s sessions: any it has attached, and in
/// jcc only while Cira says blocks wait there (Cira PLAN TMUX-22).
fn rotates(flavor: Flavor, project: &ProjectState) -> bool {
  !project.sessions.is_empty() && (flavor == Flavor::Jc || project.open_blocks() > 0)
}

/// Cmd-`'s next stop after `current` among `slots`. jc: the one after it (the
/// second when `current` is not among them), none with fewer than two. jcc,
/// whose rotation leaves out projects with no open blocks: the first when
/// `current` is not among them, so a lone project with blocks is reached.
fn next_slot<T: Copy + PartialEq>(flavor: Flavor, slots: &[T], current: T) -> Option<T> {
  let pos = slots.iter().position(|s| *s == current);
  match (flavor, pos) {
    (Flavor::Jc, _) if slots.len() <= 1 => None,
    (Flavor::Jc, pos) => Some(slots[(pos.unwrap_or(0) + 1) % slots.len()]),
    (Flavor::Jcc, Some(_)) if slots.len() == 1 => None,
    (Flavor::Jcc, Some(pos)) => Some(slots[(pos + 1) % slots.len()]),
    (Flavor::Jcc, None) => slots.first().copied(),
  }
}

#[cfg(test)]
mod tests {
  use super::{Flavor, next_slot};

  /// Cira PLAN TMUX-22: from a project left out of the rotation (no open
  /// blocks), jcc's Cmd-` goes to the first that is in it, even when it is the
  /// only one.
  #[test]
  fn jccs_cmd_backtick_goes_to_the_next_slot() {
    let next = |slots: &[u8], current| next_slot(Flavor::Jcc, slots, current);
    assert_eq!(next(&[1, 2, 3], 2), Some(3));
    assert_eq!(next(&[1, 2, 3], 3), Some(1));
    assert_eq!(next(&[4], 9), Some(4), "the one project with blocks, from one without");
    assert_eq!(next(&[4], 4), None, "already there");
    assert_eq!(next(&[], 4), None, "no project has open blocks");
  }

  /// jc's Cmd-` is as it was before TMUX-22.
  #[test]
  fn jcs_cmd_backtick_is_unchanged() {
    let next = |slots: &[u8], current| next_slot(Flavor::Jc, slots, current);
    assert_eq!(next(&[1, 2, 3], 2), Some(3));
    assert_eq!(next(&[1, 2, 3], 3), Some(1));
    assert_eq!(next(&[1, 2, 3], 9), Some(2), "from no slot, the second");
    assert_eq!(next(&[4], 9), None, "one session: nowhere to go");
    assert_eq!(next(&[], 4), None);
  }
}
