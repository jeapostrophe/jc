pub mod code_view;
pub mod diff_view;
pub mod pane;
pub mod picker;
pub mod project_view;
pub mod reply_view;
pub mod todo_view;
pub mod workspace;

use gpui::*;
use gpui_component::ActiveTheme;

/// Renders a warning banner when a file has been externally modified.
pub fn external_change_banner(externally_modified: bool, cx: &App) -> AnyElement {
  if externally_modified {
    let theme = cx.theme();
    div()
      .px_2()
      .py_1()
      .bg(theme.warning)
      .text_sm()
      .text_color(theme.warning_foreground)
      .child("File changed on disk \u{2014} press Cmd-R to reload")
      .into_any_element()
  } else {
    div().into_any_element()
  }
}
