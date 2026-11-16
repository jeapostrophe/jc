use gpui::*;
use gpui_component::ActiveTheme;
use jc_core::flavor::Flavor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneContentKind {
  ClaudeTerminal,
  GeneralTerminal,
  CodeViewer,
  TodoEditor,
  GlobalTodo,
  /// jcc: the Cira project's scratch pad (Cira PLAN TMUX-11).
  Scratch,
}

impl PaneContentKind {
  pub const ALL: [PaneContentKind; 6] = [
    Self::ClaudeTerminal,
    Self::GeneralTerminal,
    Self::CodeViewer,
    Self::TodoEditor,
    Self::GlobalTodo,
    Self::Scratch,
  ];

  /// The views `flavor` offers: jc has no scratch pad, which is Cira's.
  pub fn offered(flavor: Flavor) -> Vec<Self> {
    Self::ALL.into_iter().filter(|k| flavor == Flavor::Jcc || *k != Self::Scratch).collect()
  }

  pub fn label(self) -> &'static str {
    match self {
      Self::ClaudeTerminal => "Claude",
      Self::GeneralTerminal => "Terminal",
      Self::CodeViewer => "Code",
      Self::TodoEditor => "TODO",
      Self::GlobalTodo => "Global TODO",
      Self::Scratch => "Scratch",
    }
  }
}

/// What the three panes show for a session not laid out yet, and at start
/// (jcc's right panel: Cira PLAN TMUX-12).
pub fn default_kinds(flavor: Flavor, has_session: bool) -> [PaneContentKind; 3] {
  use PaneContentKind::*;
  match (has_session, flavor) {
    (false, _) => [TodoEditor, TodoEditor, GlobalTodo],
    (true, Flavor::Jc) => [ClaudeTerminal, TodoEditor, GlobalTodo],
    (true, Flavor::Jcc) => [ClaudeTerminal, TodoEditor, GeneralTerminal],
  }
}

/// jcc's Cmd-\ on a pane showing `shown` (Cira PLAN TMUX-24): what it shows
/// next, and what to remember it showed. Scratch goes back to `before`, else
/// `fallback`; anything else, or nothing, gives way to Scratch.
pub fn scratch_toggle(
  shown: Option<PaneContentKind>,
  before: Option<PaneContentKind>,
  fallback: PaneContentKind,
) -> (PaneContentKind, Option<PaneContentKind>) {
  match shown {
    Some(PaneContentKind::Scratch) => (before.unwrap_or(fallback), None),
    shown => (PaneContentKind::Scratch, shown),
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

#[cfg(test)]
mod tests {
  use super::{Flavor, PaneContentKind, default_kinds, scratch_toggle};

  /// Cira PLAN TMUX-24: Cmd-\ twice is where it started.
  #[test]
  fn cmd_backslash_goes_to_scratch_and_back() {
    use PaneContentKind::*;
    let (to, before) = scratch_toggle(Some(GeneralTerminal), None, GeneralTerminal);
    assert_eq!((to, before), (Scratch, Some(GeneralTerminal)));
    assert_eq!(scratch_toggle(Some(to), before, GeneralTerminal), (GeneralTerminal, None));
    let (to, before) = scratch_toggle(Some(CodeViewer), None, GeneralTerminal);
    assert_eq!(scratch_toggle(Some(to), before, GeneralTerminal).0, CodeViewer, "not the default");
    // Scratch put there another way (Cmd-O) goes back to the pane's default.
    assert_eq!(scratch_toggle(Some(Scratch), None, GeneralTerminal), (GeneralTerminal, None));
    assert_eq!(scratch_toggle(None, None, GeneralTerminal), (Scratch, None));
  }

  /// Cira PLAN TMUX-12: jcc's right panel starts on the session's Terminal;
  /// jc's stays on the Global TODO.
  #[test]
  fn jccs_right_panel_defaults_to_the_terminal() {
    use PaneContentKind::*;
    assert_eq!(default_kinds(Flavor::Jcc, true), [ClaudeTerminal, TodoEditor, GeneralTerminal]);
    assert_eq!(default_kinds(Flavor::Jc, true), [ClaudeTerminal, TodoEditor, GlobalTodo]);
    // With no session there is no terminal to show.
    assert_eq!(default_kinds(Flavor::Jcc, false), [TodoEditor, TodoEditor, GlobalTodo]);
  }
}
