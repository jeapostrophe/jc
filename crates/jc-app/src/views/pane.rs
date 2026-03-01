use gpui::*;
use gpui_component::ActiveTheme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneContentKind {
  ClaudeTerminal,
  GeneralTerminal,
}

impl PaneContentKind {
  pub fn label(self) -> &'static str {
    match self {
      Self::ClaudeTerminal => "Claude",
      Self::GeneralTerminal => "Terminal",
    }
  }
}

pub struct PaneContent {
  pub kind: PaneContentKind,
  pub view: AnyView,
  pub focus: FocusHandle,
}

pub struct Pane {
  content: Option<PaneContent>,
  focus: FocusHandle,
}

impl Pane {
  pub fn with_content(content: PaneContent, cx: &mut Context<Self>) -> Self {
    Self { content: Some(content), focus: cx.focus_handle() }
  }

  pub fn set_content(&mut self, content: PaneContent, cx: &mut Context<Self>) {
    self.content = Some(content);
    cx.notify();
  }

  pub fn content_kind(&self) -> Option<PaneContentKind> {
    self.content.as_ref().map(|c| c.kind)
  }

  pub fn focus_content(&self, window: &mut Window) {
    if let Some(content) = &self.content {
      content.focus.focus(window);
    }
  }
}

impl Focusable for Pane {
  fn focus_handle(&self, _: &App) -> FocusHandle {
    self.focus.clone()
  }
}

impl Render for Pane {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();

    let content_element = if let Some(content) = &self.content {
      div().size_full().child(content.view.clone())
    } else {
      div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(div().text_color(theme.muted_foreground).child("Empty pane"))
    };

    div().id("pane").track_focus(&self.focus).size_full().overflow_hidden().child(content_element)
  }
}
