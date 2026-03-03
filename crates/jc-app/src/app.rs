use std::borrow::Cow;

use crate::views::code_view;
use crate::views::picker;
use crate::views::todo_view;
use crate::views::workspace::{self, Workspace};
use gpui::*;
use gpui_component::TitleBar;
use jc_core::config::{AppConfig, AppState};
use jc_core::theme::ThemeConfig;

pub fn run(state: AppState, config: AppConfig, theme: ThemeConfig) {
  let app = Application::new().with_assets(gpui_component_assets::Assets);

  app.run(move |cx| {
    gpui_component::init(cx);
    jc_terminal::init(cx);
    workspace::init(cx);
    picker::init(cx);
    code_view::init(cx);
    todo_view::init(cx);

    // Register the bundled Lilex font family.
    cx.text_system()
      .add_fonts(vec![
        Cow::Borrowed(include_bytes!("../../../data/fonts/Lilex-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../../../data/fonts/Lilex-Bold.ttf")),
        Cow::Borrowed(include_bytes!("../../../data/fonts/Lilex-Italic.ttf")),
        Cow::Borrowed(include_bytes!("../../../data/fonts/Lilex-BoldItalic.ttf")),
      ])
      .expect("failed to register Lilex fonts");

    cx.on_window_closed(|cx| {
      if cx.windows().is_empty() {
        cx.quit();
      }
    })
    .detach();

    let opts = WindowOptions {
      window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
        None,
        size(px(1200.0), px(800.0)),
        cx,
      ))),
      titlebar: Some(TitleBar::title_bar_options()),
      ..Default::default()
    };

    cx.open_window(opts, |window, cx| {
      window.activate_window();
      let view = cx.new(|cx| Workspace::new(state, config, theme, window, cx));
      cx.new(|cx| gpui_component::Root::new(view, window, cx))
    })
    .expect("failed to open window");

    cx.activate(true);
  });
}
