pub mod close_confirm;
pub mod code_view;
pub mod keybinding_help;
pub mod pane;
pub mod picker;
pub mod project_state;
pub mod session_state;
pub mod todo_view;
pub mod workspace;

use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::InputState;

use crate::language::Language;
use jc_core::cira::{Down, Reach, Status};

/// Trait for views that support line search via `LineSearchPickerDelegate`.
pub trait LineSearchable: Sized + 'static {
  fn editor_text(&self, cx: &App) -> String;
  fn language_name(&self) -> Language;
  fn scroll_to_line(&self, line: u32, window: &mut Window, cx: &mut Context<Self>);
}

/// Reads the full text content from an editor widget.
pub fn editor_text(editor: &Entity<InputState>, cx: &App) -> String {
  editor.read(cx).value().as_ref().to_string()
}

/// Scrolls an editor widget so the given 0-based `line` is approximately centered.
pub fn scroll_editor_to_line<V: 'static>(
  editor: &Entity<InputState>,
  line: u32,
  window: &mut Window,
  cx: &mut Context<V>,
) {
  editor.update(cx, |editor, cx| {
    editor.set_cursor_position(gpui_component::input::Position::new(line, 0), window, cx);
    editor.scroll_to_center_line(line, cx);
  });
}

/// jcc's one record of whether its last `cira status` reached Cira. Views
/// read it when they render, so one made during an outage knows. jc never
/// sets it.
#[derive(Default)]
pub struct CiraReach(Reach);

impl Global for CiraReach {}

/// Whether Cira was reached at the last read; `Reach::Unknown` in jc.
pub fn cira_reach(cx: &App) -> &Reach {
  static UNKNOWN: Reach = Reach::Unknown;
  cx.try_global::<CiraReach>().map_or(&UNKNOWN, |g| &g.0)
}

/// The window's Cira banner: why Cira is down, while it is.
pub fn cira_banner(cx: &App) -> Option<String> {
  cira_reach(cx).down().map(Down::to_string)
}

/// Take a read's outcome (`Reach::read`). When what is shown changed, say it
/// on stderr and re-render `todo_views`, whose banners depend on it; returns
/// whether it changed.
pub fn read_cira(
  read: &Result<Status, Down>,
  todo_views: impl IntoIterator<Item = Entity<todo_view::TodoView>>,
  cx: &mut App,
) -> bool {
  let reach = &mut cx.default_global::<CiraReach>().0;
  if !reach.read(read) {
    return false;
  }
  match reach.down() {
    Some(down) => eprintln!("cira: {down}"),
    None => eprintln!("cira: reachable"),
  }
  for tv in todo_views {
    tv.update(cx, |_, cx| cx.notify());
  }
  true
}

/// One warning line over an editor pane: a conflict, or a save that failed
/// (`SharedFile::banner`), else nothing.
pub fn file_banner(text: Option<String>, cx: &App) -> AnyElement {
  let Some(text) = text else { return div().into_any_element() };
  let theme = cx.theme();
  div()
    .px_2()
    .py_1()
    .bg(theme.warning)
    .text_sm()
    .text_color(theme.warning_foreground)
    .child(text)
    .into_any_element()
}
