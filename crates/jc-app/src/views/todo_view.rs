use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use std::path::PathBuf;

pub struct TodoView {
  editor: Entity<InputState>,
  file_path: PathBuf,
  dirty: bool,
  _subscription: Subscription,
}

impl TodoView {
  pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let editor = cx.new(|cx| InputState::new(window, cx).code_editor("markdown").soft_wrap(true));

    let subscription = cx.subscribe(&editor, |this: &mut Self, _, event: &InputEvent, _cx| {
      if matches!(event, InputEvent::Change) {
        this.dirty = true;
      }
    });

    let file_path = project_path.join("TODO.md");
    let mut view = Self { editor, file_path, dirty: false, _subscription: subscription };
    view.load(window, cx);
    view
  }

  pub fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let content = std::fs::read_to_string(&self.file_path).unwrap_or_default();
    self.editor.update(cx, |state, cx| {
      state.set_value(content, window, cx);
    });
    self.dirty = false;
  }

  pub fn save(&mut self, cx: &mut Context<Self>) {
    let content = self.editor.read(cx).value();
    if let Err(e) = std::fs::write(&self.file_path, content.as_ref()) {
      eprintln!("Failed to save TODO.md: {e}");
    }
    self.dirty = false;
  }
}

impl Render for TodoView {
  fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
    div().size_full().child(Input::new(&self.editor).h_full().appearance(false).bordered(false))
  }
}

impl Focusable for TodoView {
  fn focus_handle(&self, cx: &App) -> FocusHandle {
    self.editor.read(cx).focus_handle(cx)
  }
}
