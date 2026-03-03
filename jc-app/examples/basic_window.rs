use gpui::*;
use gpui_component::ActiveTheme;

struct Hello;

impl Render for Hello {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();
    div()
      .flex()
      .size_full()
      .justify_center()
      .items_center()
      .bg(theme.background)
      .child(div().text_xl().text_color(theme.foreground).child("Hello from GPUI"))
  }
}

fn main() {
  let app = Application::new().with_assets(gpui_component_assets::Assets);
  app.run(|cx| {
    gpui_component::init(cx);

    let opts = WindowOptions {
      window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
        None,
        size(px(800.0), px(600.0)),
        cx,
      ))),
      titlebar: Some(TitlebarOptions { title: Some("basic_window".into()), ..Default::default() }),
      ..Default::default()
    };

    cx.open_window(opts, |window, cx| {
      let view = cx.new(|_cx| Hello);
      cx.new(|cx| gpui_component::Root::new(view, window, cx))
    })
    .expect("failed to open window");
  });
}
