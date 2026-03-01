use gpui::{AppContext, Application, Bounds, WindowBounds, WindowOptions, px, size};
use jc_terminal::TerminalView;

fn main() {
  let app = Application::new();
  app.run(|cx| {
    let opts = WindowOptions {
      window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
        None,
        size(px(900.0), px(600.0)),
        cx,
      ))),
      ..Default::default()
    };

    cx.open_window(opts, |window, cx| {
      cx.new(|cx| TerminalView::new(Default::default(), None, window, cx))
    })
    .expect("failed to open window");
  });
}
