pub mod code_view;
pub mod comment_panel;
pub mod diff_view;
pub mod pane;
pub mod picker;
pub mod project_state;
pub mod project_view;
pub mod reply_view;
pub mod session_state;
pub mod todo_view;
pub mod workspace;

use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::InputState;

/// Reads the full text content from an editor widget.
pub fn editor_text(editor: &Entity<InputState>, cx: &App) -> String {
  editor.read(cx).value().as_ref().to_string()
}

/// Returns the 1-based (start_line, end_line) of the current selection in an editor.
pub fn selection_line_range(editor: &Entity<InputState>, cx: &App) -> (u32, u32) {
  let (start, end) = editor.read(cx).selection_positions();
  (start.line + 1, end.line + 1)
}

/// Formats a line range as "N" (single line) or "N-M" (multi-line).
pub fn format_line_range(start: u32, end: u32) -> String {
  if start == end { format!("{start}") } else { format!("{start}-{end}") }
}

/// Scrolls an editor widget so the given 0-based `line` is approximately centered.
pub fn scroll_editor_to_line<V: 'static>(
  editor: &Entity<InputState>,
  line: u32,
  window: &mut Window,
  cx: &mut Context<V>,
) {
  let line_height = window.line_height();
  let viewport_height = window.viewport_size().height;
  let half_viewport_lines =
    if line_height > px(0.) { (viewport_height / line_height / 2.0).floor() as u32 } else { 15 };

  let pre_line = line.saturating_sub(half_viewport_lines);
  editor.update(cx, |editor, cx| {
    editor.set_cursor_position(gpui_component::input::Position::new(pre_line, 0), window, cx);
  });

  let editor = editor.clone();
  cx.spawn_in(window, async move |_this: WeakEntity<V>, cx: &mut AsyncWindowContext| {
    let _ = editor.update_in(cx, |editor, window, cx| {
      editor.set_cursor_position(gpui_component::input::Position::new(line, 0), window, cx);
    });
  })
  .detach();
}

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
