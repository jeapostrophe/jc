use crate::views::pane::{Pane, PaneContent, PaneContentKind};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::resizable::{h_resizable, resizable_panel};
use jc_core::config::AppState;
use jc_terminal::TerminalView;

actions!(
  workspace,
  [
    CloseWindow,
    MinimizeWindow,
    Quit,
    FocusLeftPane,
    FocusRightPane,
    CyclePaneFocus,
    ShowClaudeTerminal,
    ShowGeneralTerminal,
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
  #[allow(dead_code)]
  state: AppState,
  focus: FocusHandle,
}

impl Workspace {
  pub fn new(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let claude_terminal = cx.new(|cx| TerminalView::new(Default::default(), None, window, cx));
    let general_terminal = cx.new(|cx| TerminalView::new(Default::default(), None, window, cx));

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

    Self {
      left_pane,
      right_pane,
      active_pane: ActivePane::Left,
      claude_terminal,
      general_terminal,
      state,
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

  fn cycle_pane_focus(&mut self, _: &CyclePaneFocus, window: &mut Window, cx: &mut Context<Self>) {
    match self.active_pane {
      ActivePane::Left => self.focus_right_pane(&FocusRightPane, window, cx),
      ActivePane::Right => self.focus_left_pane(&FocusLeftPane, window, cx),
    }
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
    let (view, focus) = match kind {
      PaneContentKind::ClaudeTerminal => {
        let focus = self.claude_terminal.read(cx).focus_handle(cx);
        (self.claude_terminal.clone().into(), focus)
      }
      PaneContentKind::GeneralTerminal => {
        let focus = self.general_terminal.read(cx).focus_handle(cx);
        (self.general_terminal.clone().into(), focus)
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
      .on_action(cx.listener(Self::show_claude_terminal))
      .on_action(cx.listener(Self::show_general_terminal))
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
    KeyBinding::new("ctrl-`", CyclePaneFocus, Some("Workspace")),
  ]);
}
