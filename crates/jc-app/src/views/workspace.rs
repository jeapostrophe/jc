use crate::views::pane::{Pane, PaneContent, PaneContentKind};
use crate::views::project_view::ProjectView;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::resizable::{h_resizable, resizable_panel};
use jc_core::config::AppState;
use jc_terminal::TerminalView;

actions!(
  workspace,
  [CloseWindow, MinimizeWindow, Quit, FocusLeftPane, FocusRightPane, CyclePaneFocus,]
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
  #[allow(dead_code)]
  state: AppState,
  focus: FocusHandle,
}

impl Workspace {
  pub fn new(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let project_view = cx.new(|cx| ProjectView::with_state(state.clone(), cx));
    let project_focus = project_view.read(cx).focus_handle(cx);

    let left_pane = cx.new(|cx| {
      Pane::with_content(
        PaneContent {
          kind: PaneContentKind::ProjectList,
          view: project_view.into(),
          focus: project_focus,
        },
        cx,
      )
    });

    let terminal_view = cx.new(|cx| TerminalView::new(Default::default(), None, window, cx));
    let terminal_focus = terminal_view.read(cx).focus_handle(cx);

    let right_pane = cx.new(|cx| {
      Pane::with_content(
        PaneContent {
          kind: PaneContentKind::Terminal,
          view: terminal_view.into(),
          focus: terminal_focus,
        },
        cx,
      )
    });

    Self { left_pane, right_pane, active_pane: ActivePane::Left, state, focus: cx.focus_handle() }
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

  fn cycle_pane_focus(&mut self, _: &CyclePaneFocus, window: &mut Window, cx: &mut Context<Self>) {
    match self.active_pane {
      ActivePane::Left => self.focus_right_pane(&FocusRightPane, window, cx),
      ActivePane::Right => self.focus_left_pane(&FocusLeftPane, window, cx),
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

    let left_wrapper = div()
      .size_full()
      .when(left_active, |d: Div| d.border_l_2().border_color(active_border))
      .child(self.left_pane.clone());

    let right_wrapper = div()
      .size_full()
      .when(right_active, |d: Div| d.border_l_2().border_color(active_border))
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
      .on_action(cx.listener(Self::cycle_pane_focus))
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
    KeyBinding::new("cmd-1", FocusLeftPane, Some("Workspace")),
    KeyBinding::new("cmd-2", FocusRightPane, Some("Workspace")),
    KeyBinding::new("ctrl-`", CyclePaneFocus, Some("Workspace")),
  ]);
}
