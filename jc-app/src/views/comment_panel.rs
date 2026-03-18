use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::{Input, InputState, RopeExt as _};

actions!(comment_panel, [ConfirmComment, DismissComment]);

pub fn init(cx: &mut App) {
  cx.bind_keys([
    KeyBinding::new("cmd-enter", ConfirmComment, Some("CommentPanel")),
    // Also bind in "Input" context so Cmd-Enter works when the input has
    // focus — the action bubbles up to the CommentPanel handler.
    KeyBinding::new("cmd-enter", ConfirmComment, Some("Input")),
    KeyBinding::new("escape", DismissComment, Some("CommentPanel")),
    KeyBinding::new("cmd-w", DismissComment, Some("CommentPanel")),
  ]);
}

pub struct CommentContext {
  pub prefilled: String,
}

pub enum CommentPanelEvent {
  Confirmed(String),
  Dismissed,
}

impl EventEmitter<CommentPanelEvent> for CommentPanel {}

pub struct CommentPanel {
  input: Entity<InputState>,
  focus: FocusHandle,
}

impl CommentPanel {
  pub fn new(context: CommentContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let input = cx.new(|cx| {
      let mut state = InputState::new(window, cx).placeholder("Type comment...");
      state.set_value(&context.prefilled, window, cx);
      // Place cursor at the end of the prefilled text.
      let position = state.text().offset_to_position(context.prefilled.len());
      state.set_cursor_position(position, window, cx);
      state
    });

    Self { input, focus: cx.focus_handle() }
  }

  pub fn input_focus_handle(&self, cx: &App) -> FocusHandle {
    self.input.read(cx).focus_handle(cx)
  }

  fn confirm(&mut self, _: &ConfirmComment, _window: &mut Window, cx: &mut Context<Self>) {
    let text = self.input.read(cx).value().as_ref().to_string();
    cx.emit(CommentPanelEvent::Confirmed(text));
  }

  fn dismiss(&mut self, _: &DismissComment, _window: &mut Window, cx: &mut Context<Self>) {
    cx.emit(CommentPanelEvent::Dismissed);
  }
}

impl Focusable for CommentPanel {
  fn focus_handle(&self, _: &App) -> FocusHandle {
    self.focus.clone()
  }
}

impl Render for CommentPanel {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();

    div()
      .id("comment-panel")
      .key_context("CommentPanel")
      .track_focus(&self.focus)
      .on_action(cx.listener(Self::confirm))
      .on_action(cx.listener(Self::dismiss))
      .font_family("Lilex")
      .w(px(500.0))
      .bg(theme.background)
      .border_1()
      .border_color(theme.border)
      .rounded_md()
      .overflow_hidden()
      .flex()
      .flex_col()
      .shadow_lg()
      .child(
        div()
          .px_2()
          .py_1()
          .border_b_1()
          .border_color(theme.border)
          .text_sm()
          .text_color(theme.muted_foreground)
          .child("Comment (Cmd-Enter to confirm)"),
      )
      .child(
        div().p_2().child(Input::new(&self.input).appearance(false).cleanable(false).h(px(120.0))),
      )
  }
}
