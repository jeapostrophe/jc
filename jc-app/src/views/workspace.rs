use crate::views::comment_panel::{CommentPanel, CommentPanelEvent};
use crate::views::diff_view::DiffViewEvent;
use crate::views::pane::{Pane, PaneContent, PaneContentKind};
use crate::views::picker::{
  CodeSymbolPickerDelegate, DiffFilePickerDelegate, FilePickerDelegate, GitLogPickerDelegate,
  LineSearchPickerDelegate, OpenContextPicker, OpenFilePicker, PickerEvent, PickerState,
  ReplyHeadingPickerDelegate, ReplyTurnPickerDelegate, SearchLines, SessionPickerDelegate,
  ShowSessionPicker, ShowSlugPicker, SlugAction, SlugPickerDelegate, TodoHeaderPickerDelegate,
};
use crate::views::project_state::ProjectState;
use crate::views::reply_view::gc_stale_replies;
use crate::views::session_state::SessionState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::TitleBar;
use gpui_component::resizable::{h_resizable, resizable_panel};
use gpui_component::theme::Theme;
use gpui_component::tooltip::Tooltip;
use jc_core::config::{AppConfig, AppState};
use jc_core::hooks::{HookEvent, HookServer};
use jc_core::theme::Appearance;
use jc_core::usage::{FullUsageReport, ParStatus};
use jc_terminal::Palette;
use std::ops::DerefMut;
use std::path::PathBuf;
use std::time::Duration as StdDuration;

actions!(
  workspace,
  [
    CloseWindow,
    MinimizeWindow,
    Quit,
    FocusLeftPane,
    FocusRightPane,
    ShowClaudeTerminal,
    ShowGeneralTerminal,
    ShowGitDiff,
    ShowCodeViewer,
    ShowTodoEditor,
    OpenInExternalEditor,
    EvenSplit,
    OpenGitLogPicker,
    ShowReplyViewer,
    OpenCommentPanel,
    SaveFile,
    SendToTerminal,
  ]
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePane {
  Left,
  Right,
}

/// Map a GPUI WindowAppearance to our Appearance enum.
fn appearance_from_window(appearance: WindowAppearance) -> Appearance {
  match appearance {
    WindowAppearance::Dark | WindowAppearance::VibrantDark => Appearance::Dark,
    WindowAppearance::Light | WindowAppearance::VibrantLight => Appearance::Light,
  }
}

pub struct Workspace {
  left_pane: Entity<Pane>,
  right_pane: Entity<Pane>,
  active_pane: ActivePane,
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
  usage_report: Option<FullUsageReport>,
  _usage_poll_task: Option<Task<()>>,
  _appearance_subscription: Subscription,
  _diff_view_subscription: Option<Subscription>,
  _left_focus_in: Subscription,
  _right_focus_in: Subscription,
  _hook_server: Option<HookServer>,
  _hook_poll_task: Option<Task<()>>,
}

impl Workspace {
  pub fn new(
    state: AppState,
    config: AppConfig,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    // Detect the current system appearance and pick the right terminal palette.
    let appearance = appearance_from_window(window.appearance());
    let palette = Palette::for_appearance(appearance);

    // Build a ProjectState per registered project.
    let mut projects = Vec::new();
    for project in &state.projects {
      projects.push(ProjectState::create(
        project.path.clone(),
        project.name.clone(),
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
    let (left_content, right_content) = Self::initial_pane_contents(&projects[0], cx);

    let left_pane = cx.new(|cx| Pane::with_content(left_content, cx));
    let right_pane = cx.new(|cx| Pane::with_content(right_content, cx));

    left_pane.read(cx).focus_content(window);

    // Observe system appearance changes and update themes accordingly.
    let appearance_subscription =
      cx.observe_window_appearance(window, |this: &mut Self, window, cx| {
        this.apply_appearance(appearance_from_window(window.appearance()), window, cx);
      });

    let working_hours = config.working_hours.clone();
    let usage_poll_task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
      loop {
        let wh = working_hours.clone();
        let result = std::thread::spawn(move || -> Option<FullUsageReport> {
          let token = jc_core::claude_api::load_oauth_token().ok()?;
          let api = jc_core::claude_api::fetch_usage(&token).ok()?;
          Some(FullUsageReport::from_api(&api, &wh))
        })
        .join()
        .ok()
        .flatten();

        let Ok(should_continue) = cx.update(|cx: &mut App| {
          if let Some(entity) = this.upgrade() {
            entity.update(cx, |view, cx| {
              view.usage_report = result;
              cx.notify();
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

        Timer::after(StdDuration::from_secs(60)).await;
      }
    });

    let left_focus = left_pane.read(cx).focus_handle(cx);
    let right_focus = right_pane.read(cx).focus_handle(cx);

    let left_focus_in = cx.on_focus_in(&left_focus, window, |this, _window, cx| {
      if this.active_pane != ActivePane::Left {
        this.active_pane = ActivePane::Left;
        cx.notify();
      }
    });
    let right_focus_in = cx.on_focus_in(&right_focus, window, |this, _window, cx| {
      if this.active_pane != ActivePane::Right {
        this.active_pane = ActivePane::Right;
        cx.notify();
      }
    });

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

    let mut ws = Self {
      left_pane,
      right_pane,
      active_pane: ActivePane::Left,
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
      usage_report: None,
      _usage_poll_task: Some(usage_poll_task),
      _appearance_subscription: appearance_subscription,
      _diff_view_subscription: None,
      _left_focus_in: left_focus_in,
      _right_focus_in: right_focus_in,
      _hook_server: hook_server,
      _hook_poll_task: hook_poll_task,
    };

    ws.subscribe_active_project(window, cx);
    ws
  }

  /// Build initial PaneContent for left and right panes from a project.
  fn initial_pane_contents(project: &ProjectState, cx: &App) -> (PaneContent, PaneContent) {
    let left = if let Some(session) = project.active_session() {
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

    let right = if let Some(session) = project.active_session() {
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

    (left, right)
  }

  /// Subscribe to the active project's diff_view and todo_view events.
  fn subscribe_active_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let project = &self.projects[self.active_project_index];

    let diff_view = project.diff_view.clone();
    self._diff_view_subscription = Some(cx.subscribe_in(
      &diff_view,
      window,
      |this: &mut Self, _, event: &DiffViewEvent, window, cx| match event {
        DiffViewEvent::Reviewed => {
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

  fn project_path(&self) -> PathBuf {
    self.active_project().path.clone()
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

  fn focus_left_pane(&mut self, _: &FocusLeftPane, window: &mut Window, cx: &mut Context<Self>) {
    self.active_pane = ActivePane::Left;
    self.left_pane.read(cx).focus_content(window);
    cx.notify();
  }

  fn focus_right_pane(&mut self, _: &FocusRightPane, window: &mut Window, cx: &mut Context<Self>) {
    self.active_pane = ActivePane::Right;
    self.right_pane.read(cx).focus_content(window);
    cx.notify();
  }

  fn active_pane_entity(&self) -> &Entity<Pane> {
    match self.active_pane {
      ActivePane::Left => &self.left_pane,
      ActivePane::Right => &self.right_pane,
    }
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
        project.diff_view.update(cx, |v, cx| v.refresh(window, cx));
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
        s.reply_view.update(cx, |v, cx| v.refresh(window, cx));
        let focus = s.reply_view.read(cx).focus_handle(cx);
        (s.reply_view.clone().into(), focus)
      }),
    };

    if let Some((view, focus)) = result {
      let pane = self.active_pane_entity().clone();
      pane.update(cx, |p, cx| {
        p.set_content(PaneContent { kind, view, focus: focus.clone() }, cx);
      });
      focus.focus(window);

      // When switching to the TODO editor, auto-scroll to the end of the active session's WAIT body.
      if kind == PaneContentKind::TodoEditor {
        let project = &self.projects[self.active_project_index];
        let tv = project.todo_view.read(cx);
        if let Some(slug) = project.active_slug() {
          let text = tv.editor_text(cx);
          if let Some(wait_line) = tv.document().wait_body_end_line(slug, &text) {
            let wait_line_0 = wait_line.saturating_sub(1);
            drop(tv);
            project.todo_view.update(cx, |tv, cx| tv.scroll_to_line(wait_line_0, window, cx));
          }
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

  fn even_split(&mut self, _: &EvenSplit, _window: &mut Window, cx: &mut Context<Self>) {
    self.split_generation += 1;
    cx.notify();
  }

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
    self.active_project_index = project_idx;
    self.projects[project_idx].active_session_index = Some(session_idx);

    // Update the TODO view's active session highlight.
    {
      let slug = self.projects[project_idx].sessions.get(session_idx).map(|s| s.slug.clone());
      let todo_view = self.projects[project_idx].todo_view.clone();
      todo_view.update(cx, |tv, cx| tv.set_active_slug(slug.as_deref(), cx));
    }

    if project_changed {
      self.subscribe_active_project(window, cx);
    }

    // Rebind panes to the new session's views.
    let project = &self.projects[self.active_project_index];
    if let Some(session) = project.active_session() {
      // Set left pane to claude terminal.
      let focus = session.claude_terminal.read(cx).focus_handle(cx);
      self.left_pane.update(cx, |p, cx| {
        p.set_content(
          PaneContent {
            kind: PaneContentKind::ClaudeTerminal,
            view: session.claude_terminal.clone().into(),
            focus: focus.clone(),
          },
          cx,
        );
      });

      // Set right pane to general terminal.
      let focus = session.general_terminal.read(cx).focus_handle(cx);
      self.right_pane.update(cx, |p, cx| {
        p.set_content(
          PaneContent {
            kind: PaneContentKind::GeneralTerminal,
            view: session.general_terminal.clone().into(),
            focus: focus.clone(),
          },
          cx,
        );
      });

      self.left_pane.read(cx).focus_content(window);
      self.active_pane = ActivePane::Left;
    }

    cx.notify();
  }

  fn open_session_picker(
    &mut self,
    _: &ShowSessionPicker,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.active_picker.is_some() {
      return;
    }

    let delegate = SessionPickerDelegate::new(&self.projects, self.active_project_index);
    let picker = cx.new(|cx| PickerState::new(delegate, window, cx));
    self.pre_picker_focus = window.focused(cx);

    let subscription =
      cx.subscribe_in(&picker, window, move |this: &mut Self, picker_entity, event, window, cx| {
        match event {
          PickerEvent::Confirmed => {
            let (project_idx, session_idx) = picker_entity.read(cx).delegate().confirmed_entry();
            // switch_to_session sets focus to the left pane; drop stale pre_picker_focus.
            this.pre_picker_focus.take();
            this.switch_to_session(project_idx, session_idx, window, cx);
            this.dismiss_picker();
            cx.notify();
          }
          PickerEvent::Dismissed => {
            if let Some(focus) = this.pre_picker_focus.take() {
              focus.focus(window);
            }
            this.dismiss_picker();
            cx.notify();
          }
        }
      });

    self.active_picker = Some(picker.clone().into());
    self._picker_subscription = Some(subscription);

    picker.read(cx).input_focus_handle(cx).focus(window);
    cx.notify();
  }

  fn open_slug_picker(&mut self, _: &ShowSlugPicker, window: &mut Window, cx: &mut Context<Self>) {
    if self.active_picker.is_some() {
      return;
    }

    let project = &self.projects[self.active_project_index];
    let delegate = SlugPickerDelegate::new(project);
    let picker = cx.new(|cx| PickerState::new(delegate, window, cx));
    self.pre_picker_focus = window.focused(cx);

    let project_idx = self.active_project_index;
    let subscription =
      cx.subscribe_in(&picker, window, move |this: &mut Self, picker_entity, event, window, cx| {
        match event {
          PickerEvent::Confirmed => {
            let action =
              picker_entity.read(cx).delegate().confirmed_entry().map(|e| e.action.clone());
            match action {
              Some(SlugAction::Switch(session_idx)) => {
                this.pre_picker_focus.take();
                this.switch_to_session(project_idx, session_idx, window, cx);
              }
              Some(SlugAction::Attach(slug, label)) => {
                this.pre_picker_focus.take();
                this.adopt_slug(project_idx, &slug, &label, window, cx);
              }
              Some(SlugAction::New) => {
                this.pre_picker_focus.take();
                this.create_new_session(project_idx, window, cx);
              }
              None => {
                if let Some(focus) = this.pre_picker_focus.take() {
                  focus.focus(window);
                }
              }
            }
            this.dismiss_picker();
            cx.notify();
          }
          PickerEvent::Dismissed => {
            if let Some(focus) = this.pre_picker_focus.take() {
              focus.focus(window);
            }
            this.dismiss_picker();
            cx.notify();
          }
        }
      });

    self.active_picker = Some(picker.clone().into());
    self._picker_subscription = Some(subscription);

    picker.read(cx).input_focus_handle(cx).focus(window);
    cx.notify();
  }

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
    let appearance = appearance_from_window(window.appearance());
    let palette = Palette::for_appearance(appearance);
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
    let appearance = appearance_from_window(window.appearance());
    let palette = Palette::for_appearance(appearance);
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

  // ---------------------------------------------------------------------------
  // Pickers
  // ---------------------------------------------------------------------------

  fn open_file_picker(&mut self, _: &OpenFilePicker, window: &mut Window, cx: &mut Context<Self>) {
    if self.active_picker.is_some() {
      return;
    }

    let project = self.active_project();
    let delegate = FilePickerDelegate::new(
      project.path.clone(),
      project.code_view.clone(),
      self.recent_files.clone(),
    );
    self.show_picker_with_confirm(delegate, Some(PaneContentKind::CodeViewer), window, cx);
  }

  fn open_context_picker(
    &mut self,
    _: &OpenContextPicker,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.active_picker.is_some() {
      return;
    }

    let pane = self.active_pane_entity().clone();
    let kind = pane.read(cx).content_kind();
    let project = self.active_project();

    match kind {
      Some(PaneContentKind::GitDiff) => {
        self.open_diff_picker(window, cx);
      }
      Some(PaneContentKind::TodoEditor) => {
        let delegate = TodoHeaderPickerDelegate::new(project.todo_view.clone(), cx);
        self.show_picker(delegate, window, cx);
      }
      Some(PaneContentKind::CodeViewer) => {
        let delegate = CodeSymbolPickerDelegate::new(project.code_view.clone(), cx);
        self.show_picker(delegate, window, cx);
      }
      Some(PaneContentKind::ReplyViewer) => {
        if let Some(session) = project.active_session() {
          let delegate = ReplyHeadingPickerDelegate::new(session.reply_view.clone(), cx);
          self.show_picker(delegate, window, cx);
        }
      }
      _ => {}
    }
  }

  fn open_diff_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.active_picker.is_some() {
      return;
    }
    let delegate = DiffFilePickerDelegate::new(self.active_project().diff_view.clone(), cx);
    self.show_picker(delegate, window, cx);
  }

  fn open_git_log_picker(
    &mut self,
    _: &OpenGitLogPicker,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.active_picker.is_some() {
      return;
    }

    let pane = self.active_pane_entity().clone();
    let kind = pane.read(cx).content_kind();
    let project = self.active_project();

    if kind == Some(PaneContentKind::ReplyViewer) {
      if let Some(session) = project.active_session() {
        let delegate = ReplyTurnPickerDelegate::new(session.reply_view.clone(), cx);
        self.show_picker(delegate, window, cx);
      }
    } else {
      let delegate = GitLogPickerDelegate::new(project.diff_view.clone(), cx);
      self.show_picker_with_confirm(delegate, Some(PaneContentKind::GitDiff), window, cx);
    }
  }

  fn search_lines(&mut self, _: &SearchLines, window: &mut Window, cx: &mut Context<Self>) {
    if self.active_picker.is_some() {
      return;
    }

    let pane = self.active_pane_entity().clone();
    let kind = pane.read(cx).content_kind();
    let project = self.active_project();

    match kind {
      Some(PaneContentKind::CodeViewer) => {
        let delegate = LineSearchPickerDelegate::for_code_view(&project.code_view, cx);
        self.show_picker(delegate, window, cx);
      }
      Some(PaneContentKind::TodoEditor) => {
        let delegate = LineSearchPickerDelegate::for_todo_view(&project.todo_view, cx);
        self.show_picker(delegate, window, cx);
      }
      Some(PaneContentKind::GitDiff) => {
        let delegate = LineSearchPickerDelegate::for_diff_view(&project.diff_view, cx);
        self.show_picker(delegate, window, cx);
      }
      Some(PaneContentKind::ReplyViewer) => {
        if let Some(session) = project.active_session() {
          let delegate = LineSearchPickerDelegate::for_reply_view(&session.reply_view, cx);
          self.show_picker(delegate, window, cx);
        }
      }
      _ => {}
    }
  }

  fn show_picker_with_confirm<D: crate::views::picker::PickerDelegate>(
    &mut self,
    delegate: D,
    switch_pane: Option<PaneContentKind>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let picker = cx.new(|cx| PickerState::new(delegate, window, cx));
    self.pre_picker_focus = window.focused(cx);

    let subscription =
      cx.subscribe_in(&picker, window, move |this: &mut Self, _, event, window, cx| match event {
        PickerEvent::Confirmed => {
          if let Some(path) = this.active_project().code_view.read(cx).file_path() {
            let path = path.to_path_buf();
            this.recent_files.retain(|p| p != &path);
            this.recent_files.insert(0, path);
            this.recent_files.truncate(50);
          }
          if let Some(kind) = switch_pane {
            // set_active_pane_view focuses the new view; don't override with pre_picker_focus.
            this.set_active_pane_view(kind, window, cx);
          } else if let Some(focus) = this.pre_picker_focus.take() {
            focus.focus(window);
          }
          this.dismiss_picker();
          cx.notify();
        }
        PickerEvent::Dismissed => {
          if let Some(focus) = this.pre_picker_focus.take() {
            focus.focus(window);
          }
          this.dismiss_picker();
          cx.notify();
        }
      });

    self.active_picker = Some(picker.clone().into());
    self._picker_subscription = Some(subscription);

    picker.read(cx).input_focus_handle(cx).focus(window);
    cx.notify();
  }

  fn show_picker<D: crate::views::picker::PickerDelegate>(
    &mut self,
    delegate: D,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.show_picker_with_confirm(delegate, None, window, cx);
  }

  fn dismiss_picker(&mut self) {
    self.active_picker = None;
    self._picker_subscription = None;
  }

  // ---------------------------------------------------------------------------
  // Comment panel
  // ---------------------------------------------------------------------------

  fn open_comment_panel(
    &mut self,
    _: &OpenCommentPanel,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.active_comment_panel.is_some() || self.active_picker.is_some() {
      return;
    }

    let pane = self.active_pane_entity().clone();
    let kind = pane.read(cx).content_kind();
    let project = self.active_project();

    let context = match kind {
      Some(PaneContentKind::CodeViewer) => {
        project.code_view.read(cx).comment_context(&project.path, cx)
      }
      Some(PaneContentKind::GitDiff) => project.diff_view.read(cx).comment_context(cx),
      Some(PaneContentKind::ReplyViewer) => {
        project.active_session().and_then(|s| s.reply_view.read(cx).comment_context(cx))
      }
      _ => None,
    };

    let Some(context) = context else { return };

    // Save focus before creating the panel — CommentPanel::new calls
    // set_cursor_position which steals focus to the panel's input.
    self.pre_comment_focus = window.focused(cx);
    let panel = cx.new(|cx| CommentPanel::new(context, window, cx));

    let subscription = cx.subscribe_in(&panel, window, |this: &mut Self, _, event, window, cx| {
      if let CommentPanelEvent::Confirmed(text) = event {
        // Insert comment into active session's WAIT section.
        let project = &this.projects[this.active_project_index];
        if let Some(session) = project.active_session() {
          let comment = format!("{text}\n");
          project.todo_view.update(cx, |tv, cx| {
            tv.insert_comment(&session.slug, &comment, window, cx);
            tv.save(cx);
          });
        }
      }
      if let Some(focus) = this.pre_comment_focus.take() {
        focus.focus(window);
      }
      this.dismiss_comment_panel();
      cx.notify();
    });

    self.active_comment_panel = Some(panel.clone().into());
    self._comment_subscription = Some(subscription);

    panel.read(cx).input_focus_handle(cx).focus(window);
    cx.notify();
  }

  fn dismiss_comment_panel(&mut self) {
    self.active_comment_panel = None;
    self._comment_subscription = None;
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
    let bracketed = claude_terminal.read(cx).bracketed_paste_mode();
    let pty = claude_terminal.read(cx).pty_handle();

    if bracketed {
      let mut buf = Vec::with_capacity(message_text.len() + 12);
      buf.extend_from_slice(b"\x1b[200~");
      buf.extend_from_slice(message_text.as_bytes());
      buf.extend_from_slice(b"\x1b[201~");
      let _ = pty.write_all(&buf);
    } else {
      let _ = pty.write_all(message_text.as_bytes());
    }

    // Send Enter (\r) from a background thread after a delay so the
    // application has time to process the pasted content.
    std::thread::spawn(move || {
      std::thread::sleep(StdDuration::from_millis(200));
      let _ = pty.write_all(b"\r");
    });

    // Switch left pane to Claude terminal so the user can see it working.
    self.active_pane = ActivePane::Left;
    self.set_active_pane_view(PaneContentKind::ClaudeTerminal, window, cx);
  }

  // ---------------------------------------------------------------------------
  // Labels
  // ---------------------------------------------------------------------------

  fn pane_header_label(&self, pane: &Entity<Pane>, cx: &App) -> String {
    let project = self.active_project();
    match pane.read(cx).content_kind() {
      Some(PaneContentKind::CodeViewer) => {
        if let Some(path) = project.code_view.read(cx).file_path() {
          let relative = path.strip_prefix(&project.path).ok().unwrap_or(path);
          format!("Code: {}", relative.display())
        } else {
          "Code".to_string()
        }
      }
      Some(PaneContentKind::GitDiff) => {
        let dv = project.diff_view.read(cx);
        let reviewed = dv.reviewed_count();
        let total = dv.file_count();
        let source_label = dv.source().label();
        if let Some(name) = dv.current_file_name() {
          format!("Diff [{source_label}]: {name} ({reviewed}/{total})")
        } else {
          format!("Diff [{source_label}] ({reviewed}/{total})")
        }
      }
      Some(PaneContentKind::ReplyViewer) => {
        if let Some(session) = project.active_session() {
          let label = session.reply_view.read(cx).current_turn_label();
          format!("Reply: {label}")
        } else {
          "Reply: No session".to_string()
        }
      }
      Some(kind) => kind.label().to_string(),
      None => "Empty".to_string(),
    }
  }

  fn render_usage_label(&self, theme: &gpui_component::theme::ThemeColor) -> AnyElement {
    match &self.usage_report {
      None => div().text_sm().text_color(theme.muted_foreground).child("...").into_any_element(),
      Some(full) => {
        let color = match full.par_status() {
          ParStatus::Under => theme.success,
          ParStatus::On => theme.warning,
          ParStatus::Over => theme.danger,
        };
        let label = full.title_label();
        let full = full.clone();
        div()
          .id("usage-label")
          .text_sm()
          .text_color(color)
          .child(label)
          .tooltip(move |window, cx| {
            let f = full.clone();
            Tooltip::element(move |_window, cx| {
              let f = &f;
              let theme = cx.theme();
              let dim = theme.muted_foreground;
              let fg = theme.foreground;
              div()
                .font_family("Lilex")
                .flex()
                .flex_col()
                .gap_1()
                .text_xs()
                .child(
                  div().flex().gap_2().child(
                    div().text_color(fg).child(format!(
                      "5h: {:.0}%  (resets {})",
                      f.five_hour_pct, f.five_hour_reset
                    )),
                  ),
                )
                .child(div().flex().gap_2().child(div().text_color(fg).child(format!(
                  "7d: {:.0}%  (resets {})",
                  f.report.limit_pct, f.seven_day_reset
                ))))
                .child(
                  div().text_color(dim).child(format!("Work time: {:.0}%", f.report.working_pct)),
                )
                .child(div().text_color(fg).child(f.title_label()))
                .child(div().text_color(dim).child(format!(
                  "Pace: {}  Remaining: {}",
                  f.pace_label(),
                  f.remaining_hours_label()
                )))
                .when_some(f.extra.as_ref(), |el, extra| {
                  el.child(div().text_color(dim).child(format!(
                    "Extra: ${:.0} / ${:.0} ({:.1}%)",
                    extra.used_credits, extra.monthly_limit, extra.utilization,
                  )))
                })
            })
            .build(window, cx)
          })
          .into_any_element()
      }
    }
  }

  fn render_title_bar(&self, cx: &mut Context<Self>) -> TitleBar {
    let theme = cx.theme();
    let project = self.active_project();

    let mut title = project.name.clone();
    if let Some(session) = project.active_session() {
      title = format!("{} > {}", title, session.slug);
    }

    // Count problems across other sessions (not the active one).
    let other_problems: usize = self
      .projects
      .iter()
      .enumerate()
      .flat_map(|(pi, p)| {
        p.sessions.iter().enumerate().filter_map(move |(si, s)| {
          let is_active = pi == self.active_project_index && Some(si) == p.active_session_index;
          if is_active || s.problems.is_empty() { None } else { Some(s.problems.len()) }
        })
      })
      .sum();

    let current_has_problems = project.active_session().is_some_and(|s| !s.problems.is_empty());

    let title_el = {
      let el = div().text_sm().text_color(theme.foreground).child(title);
      if current_has_problems {
        el.child(div().ml_1().text_xs().text_color(gpui::hsla(0., 0.8, 0.5, 1.0)).child("!"))
      } else {
        el
      }
    };

    let right_el = {
      let mut el = div().flex().items_center().ml_auto().gap_2();
      if other_problems > 0 {
        el = el.child(
          div()
            .text_xs()
            .text_color(gpui::hsla(30. / 360., 0.8, 0.5, 1.0))
            .child(format!("{other_problems} problems")),
        );
      }
      el.child(self.render_usage_label(theme)).mr_2()
    };

    TitleBar::new()
      .font_family("Lilex")
      .child(div().flex().items_center().gap_1().mr_auto().child(title_el))
      .child(right_el)
  }

  fn handle_hook_event(&mut self, event: HookEvent, cx: &mut Context<Self>) {
    eprintln!("hook: {:?} session={} slug={:?}", event.kind, event.session_id, event.slug);
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

impl Render for Workspace {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();
    let active_border = theme.accent;

    let left_active = self.active_pane == ActivePane::Left;
    let right_active = self.active_pane == ActivePane::Right;

    let left_label = self.pane_header_label(&self.left_pane, cx);
    let right_label = self.pane_header_label(&self.right_pane, cx);

    let pane_header = |label: String, active: bool| {
      div()
        .px_2()
        .py_1()
        .text_sm()
        .text_color(if active { theme.foreground } else { theme.muted_foreground })
        .when(active, |d| d.font_weight(FontWeight::SEMIBOLD))
        .border_b_1()
        .border_color(theme.border)
        .truncate()
        .child(label)
    };

    let left_wrapper = div()
      .size_full()
      .flex()
      .flex_col()
      .border_l_2()
      .border_color(if left_active { active_border } else { gpui::transparent_black() })
      .child(pane_header(left_label, left_active))
      .child(div().flex_1().min_h_0().overflow_hidden().child(self.left_pane.clone()));

    let right_wrapper = div()
      .size_full()
      .flex()
      .flex_col()
      .border_l_2()
      .border_color(if right_active { active_border } else { gpui::transparent_black() })
      .child(pane_header(right_label, right_active))
      .child(div().flex_1().min_h_0().overflow_hidden().child(self.right_pane.clone()));

    div()
      .id("workspace")
      .key_context("Workspace")
      .track_focus(&self.focus)
      .size_full()
      .bg(theme.background)
      .on_action(cx.listener(Self::close_window))
      .on_action(cx.listener(Self::minimize_window))
      .on_action(cx.listener(Self::quit))
      .on_action(cx.listener(Self::focus_left_pane))
      .on_action(cx.listener(Self::focus_right_pane))
      .on_action(cx.listener(Self::show_claude_terminal))
      .on_action(cx.listener(Self::show_general_terminal))
      .on_action(cx.listener(Self::show_git_diff))
      .on_action(cx.listener(Self::show_code_viewer))
      .on_action(cx.listener(Self::show_todo_editor))
      .on_action(cx.listener(Self::show_reply_viewer))
      .on_action(cx.listener(Self::open_in_external_editor))
      .on_action(cx.listener(Self::open_file_picker))
      .on_action(cx.listener(Self::open_context_picker))
      .on_action(cx.listener(Self::even_split))
      .on_action(cx.listener(Self::open_git_log_picker))
      .on_action(cx.listener(Self::open_session_picker))
      .on_action(cx.listener(Self::open_slug_picker))
      .on_action(cx.listener(Self::search_lines))
      .on_action(cx.listener(Self::open_comment_panel))
      .on_action(cx.listener(Self::save_file))
      .on_action(cx.listener(Self::send_to_terminal))
      .child(self.render_title_bar(cx))
      .child(
        h_resizable(("main-split", self.split_generation))
          .child(resizable_panel().size(px(600.0)).child(left_wrapper))
          .child(resizable_panel().size(px(600.0)).child(right_wrapper)),
      )
      .when_some(self.active_picker.as_ref(), |el, v| el.child(modal_overlay(v)))
      .when_some(self.active_comment_panel.as_ref(), |el, v| el.child(modal_overlay(v)))
  }
}

fn modal_overlay(content: &AnyView) -> Deferred {
  deferred(
    div()
      .absolute()
      .size_full()
      .top_0()
      .left_0()
      .flex()
      .justify_center()
      .pt(px(80.0))
      .bg(hsla(0., 0., 0., 0.3))
      .on_mouse_down(MouseButton::Left, |_, _, _cx| {})
      .child(content.clone()),
  )
  .with_priority(1)
}

pub fn init(cx: &mut App) {
  cx.bind_keys([
    KeyBinding::new("cmd-w", CloseWindow, Some("Workspace")),
    KeyBinding::new("cmd-m", MinimizeWindow, Some("Workspace")),
    KeyBinding::new("cmd-q", Quit, Some("Workspace")),
    KeyBinding::new("cmd-[", FocusLeftPane, Some("Workspace")),
    KeyBinding::new("cmd-]", FocusRightPane, Some("Workspace")),
    KeyBinding::new("cmd-1", ShowClaudeTerminal, Some("Workspace")),
    KeyBinding::new("cmd-2", ShowGeneralTerminal, Some("Workspace")),
    KeyBinding::new("cmd-3", ShowGitDiff, Some("Workspace")),
    KeyBinding::new("cmd-4", ShowCodeViewer, Some("Workspace")),
    KeyBinding::new("cmd-5", ShowTodoEditor, Some("Workspace")),
    KeyBinding::new("cmd-6", ShowReplyViewer, Some("Workspace")),
    KeyBinding::new("cmd-shift-e", OpenInExternalEditor, Some("Workspace")),
    KeyBinding::new("cmd-|", EvenSplit, Some("Workspace")),
    KeyBinding::new("cmd-shift-o", OpenGitLogPicker, Some("Workspace")),
    KeyBinding::new("cmd-p", ShowSessionPicker, Some("Workspace")),
    KeyBinding::new("cmd-shift-p", ShowSlugPicker, Some("Workspace")),
    KeyBinding::new("cmd-k", OpenCommentPanel, Some("Workspace")),
    KeyBinding::new("cmd-s", SaveFile, Some("Workspace")),
    KeyBinding::new("cmd-enter", SendToTerminal, Some("Workspace")),
  ]);

  cx.bind_keys([
    KeyBinding::new("cmd-[", FocusLeftPane, Some("Input")),
    KeyBinding::new("cmd-]", FocusRightPane, Some("Input")),
    KeyBinding::new("cmd-k", OpenCommentPanel, Some("Input")),
    KeyBinding::new("cmd-s", SaveFile, Some("Input")),
    KeyBinding::new("cmd-enter", SendToTerminal, Some("Input")),
  ]);
}
