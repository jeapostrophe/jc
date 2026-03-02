use crate::views::code_view::CodeView;
use crate::views::diff_view::DiffView;
use crate::views::pane::{Pane, PaneContent, PaneContentKind};
use crate::views::todo_view::TodoView;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::TitleBar;
use gpui_component::resizable::{h_resizable, resizable_panel};
use jc_core::config::{AppConfig, AppState};
use jc_core::theme::ThemeConfig;
use jc_terminal::{Palette, TerminalConfig, TerminalView};

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
  ]
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePane {
  Left,
  Right,
}

pub struct Workspace {
  left_pane: Entity<Pane>,
  right_pane: Entity<Pane>,
  active_pane: ActivePane,
  claude_terminal: Entity<TerminalView>,
  general_terminal: Entity<TerminalView>,
  diff_view: Entity<DiffView>,
  code_view: Entity<CodeView>,
  todo_view: Entity<TodoView>,
  state: AppState,
  config: AppConfig,
  focus: FocusHandle,
}

impl Workspace {
  pub fn new(
    state: AppState,
    config: AppConfig,
    theme: ThemeConfig,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let project_path = state
      .projects
      .first()
      .map(|p| p.path.clone())
      .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let palette = Palette::from(&theme.terminal);
    let terminal_config =
      |palette: &Palette| TerminalConfig { palette: Some(palette.clone()), ..Default::default() };

    let claude_terminal =
      cx.new(|cx| TerminalView::new(terminal_config(&palette), None, window, cx));
    let general_terminal =
      cx.new(|cx| TerminalView::new(terminal_config(&palette), None, window, cx));
    let diff_view = cx.new(|cx| DiffView::new(project_path.clone(), window, cx));
    let code_view = cx.new(|cx| CodeView::new(window, cx));
    let todo_view = cx.new(|cx| TodoView::new(project_path, window, cx));

    let claude_focus = claude_terminal.read(cx).focus_handle(cx);
    let general_focus = general_terminal.read(cx).focus_handle(cx);

    let left_pane = cx.new(|cx| {
      Pane::with_content(
        PaneContent {
          kind: PaneContentKind::ClaudeTerminal,
          view: claude_terminal.clone().into(),
          focus: claude_focus,
        },
        cx,
      )
    });

    let right_pane = cx.new(|cx| {
      Pane::with_content(
        PaneContent {
          kind: PaneContentKind::GeneralTerminal,
          view: general_terminal.clone().into(),
          focus: general_focus,
        },
        cx,
      )
    });

    left_pane.read(cx).focus_content(window);

    Self {
      left_pane,
      right_pane,
      active_pane: ActivePane::Left,
      claude_terminal,
      general_terminal,
      diff_view,
      code_view,
      todo_view,
      state,
      config,
      focus: cx.focus_handle(),
    }
  }

  fn close_window(&mut self, _: &CloseWindow, window: &mut Window, _cx: &mut Context<Self>) {
    window.remove_window();
  }

  fn minimize_window(&mut self, _: &MinimizeWindow, window: &mut Window, _cx: &mut Context<Self>) {
    window.minimize_window();
  }

  fn quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
    cx.quit();
  }

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

  fn set_active_pane_view(
    &mut self,
    kind: PaneContentKind,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let (view, focus): (AnyView, FocusHandle) = match kind {
      PaneContentKind::ClaudeTerminal => {
        let focus = self.claude_terminal.read(cx).focus_handle(cx);
        (self.claude_terminal.clone().into(), focus)
      }
      PaneContentKind::GeneralTerminal => {
        let focus = self.general_terminal.read(cx).focus_handle(cx);
        (self.general_terminal.clone().into(), focus)
      }
      PaneContentKind::GitDiff => {
        self.diff_view.update(cx, |v, cx| v.refresh(window, cx));
        let focus = self.diff_view.read(cx).focus_handle(cx);
        (self.diff_view.clone().into(), focus)
      }
      PaneContentKind::CodeViewer => {
        let focus = self.code_view.read(cx).focus_handle(cx);
        (self.code_view.clone().into(), focus)
      }
      PaneContentKind::TodoEditor => {
        let focus = self.todo_view.read(cx).focus_handle(cx);
        (self.todo_view.clone().into(), focus)
      }
    };

    let pane = self.active_pane_entity().clone();
    pane.update(cx, |p, cx| {
      p.set_content(PaneContent { kind, view, focus: focus.clone() }, cx);
    });
    focus.focus(window);
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

  fn open_in_external_editor(
    &mut self,
    _: &OpenInExternalEditor,
    _window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let pane = self.active_pane_entity().clone();
    let kind = pane.read(cx).content_kind();
    let file_path = match kind {
      Some(PaneContentKind::CodeViewer) => {
        self.code_view.read(cx).file_path().map(|p| p.to_path_buf())
      }
      Some(PaneContentKind::TodoEditor) => Some(self.todo_view.read(cx).file_path().to_path_buf()),
      _ => None,
    };
    if let Some(path) = file_path {
      let editor =
        if self.config.editor.is_empty() { "open".to_string() } else { self.config.editor.clone() };
      let _ = std::process::Command::new(&editor).arg(path).spawn();
    }
  }

  fn pane_label(&self, pane: &Entity<Pane>, cx: &App) -> &'static str {
    pane.read(cx).content_kind().map_or("Empty", PaneContentKind::label)
  }

  fn render_title_bar(&self, cx: &mut Context<Self>) -> TitleBar {
    let theme = cx.theme();

    let left_label = self.pane_label(&self.left_pane, cx);
    let right_label = self.pane_label(&self.right_pane, cx);
    let left_active = self.active_pane == ActivePane::Left;
    let right_active = self.active_pane == ActivePane::Right;

    let project_name =
      self.state.projects.first().map(|p| p.name.clone()).unwrap_or_else(|| "No project".into());

    let pane_tab = |label: &'static str, active: bool| {
      div()
        .px_3()
        .py_1()
        .text_sm()
        .when(active, |d| d.text_color(theme.foreground).font_weight(FontWeight::SEMIBOLD))
        .when(!active, |d| d.text_color(theme.muted_foreground))
        .child(label)
    };

    TitleBar::new()
      // left: project > task
      .child(
        div()
          .flex()
          .items_center()
          .gap_1()
          .mr_auto()
          .child(div().text_sm().text_color(theme.foreground).child(project_name)),
      )
      // center: pane labels
      .child(
        div()
          .flex()
          .items_center()
          .gap_1()
          .child(pane_tab(left_label, left_active))
          .child(div().text_sm().text_color(theme.muted_foreground).child("|"))
          .child(pane_tab(right_label, right_active)),
      )
      // right: usage placeholder
      .child(
        div()
          .flex()
          .items_center()
          .ml_auto()
          .child(div().text_sm().text_color(theme.muted_foreground).child("Usage")),
      )
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

    let left_wrapper = div()
      .size_full()
      .border_l_2()
      .border_color(if left_active { active_border } else { gpui::transparent_black() })
      .child(self.left_pane.clone());

    let right_wrapper = div()
      .size_full()
      .border_l_2()
      .border_color(if right_active { active_border } else { gpui::transparent_black() })
      .child(self.right_pane.clone());

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
      .on_action(cx.listener(Self::open_in_external_editor))
      .child(self.render_title_bar(cx))
      .child(
        h_resizable("main-split")
          .child(resizable_panel().size(px(600.0)).child(left_wrapper))
          .child(resizable_panel().size(px(600.0)).child(right_wrapper)),
      )
  }
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
    KeyBinding::new("cmd-shift-e", OpenInExternalEditor, Some("Workspace")),
  ]);

  cx.bind_keys([
    KeyBinding::new("cmd-[", FocusLeftPane, Some("Input")),
    KeyBinding::new("cmd-]", FocusRightPane, Some("Input")),
  ]);
}
