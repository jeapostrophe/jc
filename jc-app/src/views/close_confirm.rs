use gpui::*;
use gpui_component::ActiveTheme;

actions!(close_confirm, [ConfirmClose, CancelClose]);

pub fn init(cx: &mut App) {
  cx.bind_keys([
    KeyBinding::new("enter", ConfirmClose, Some("CloseConfirm")),
    KeyBinding::new("escape", CancelClose, Some("CloseConfirm")),
  ]);
}

pub enum CloseConfirmEvent {
  Confirmed,
  Cancelled,
}

pub struct CloseConfirm {
  focus: FocusHandle,
  session_count: usize,
  conflicts: Vec<String>,
  is_quit: bool,
}

impl CloseConfirm {
  pub fn new(
    session_count: usize,
    conflicts: Vec<String>,
    is_quit: bool,
    cx: &mut Context<Self>,
  ) -> Self {
    Self { focus: cx.focus_handle(), session_count, conflicts, is_quit }
  }
}

impl EventEmitter<CloseConfirmEvent> for CloseConfirm {}

impl Focusable for CloseConfirm {
  fn focus_handle(&self, _: &App) -> FocusHandle {
    self.focus.clone()
  }
}

impl Render for CloseConfirm {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();

    let action = if self.is_quit { "Quit" } else { "Close" };

    let mut messages: Vec<String> = Vec::new();

    if self.session_count > 0 {
      let sessions = if self.session_count == 1 { "session" } else { "sessions" };
      messages
        .push(format!("{} active Claude {} will be terminated.", self.session_count, sessions));
    }

    if !self.conflicts.is_empty() {
      let files = self.conflicts.join(", ");
      messages.push(format!("Unsaved conflicts: {files}"));
    }

    if messages.is_empty() {
      messages.push("All sessions are idle. They will be terminated.".to_string());
    }

    div()
      .id("close-confirm")
      .key_context("CloseConfirm")
      .track_focus(&self.focus)
      .on_action(cx.listener(|_, _: &ConfirmClose, _, cx| cx.emit(CloseConfirmEvent::Confirmed)))
      .on_action(cx.listener(|_, _: &CancelClose, _, cx| cx.emit(CloseConfirmEvent::Cancelled)))
      .absolute()
      .inset_0()
      .flex()
      .items_center()
      .justify_center()
      .bg(theme.background.opacity(0.85))
      .child(
        div()
          .flex()
          .flex_col()
          .w(px(400.0))
          .p_6()
          .rounded_lg()
          .bg(theme.secondary)
          .border_1()
          .border_color(theme.border)
          .text_sm()
          .gap_4()
          .child(
            div()
              .text_base()
              .font_weight(FontWeight::BOLD)
              .text_color(theme.foreground)
              .child(format!("{action}?")),
          )
          .children(messages.into_iter().map(|msg| div().text_color(theme.foreground).child(msg)))
          .child(
            div()
              .flex()
              .flex_row()
              .justify_end()
              .gap_2()
              .text_xs()
              .text_color(theme.muted_foreground)
              .child("Esc to cancel")
              .child("·")
              .child(format!("Enter to {}", action.to_lowercase())),
          ),
      )
  }
}
