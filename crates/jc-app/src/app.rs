use crate::views::project_view::ProjectView;
use gpui::*;
use jc_core::config::AppState;

pub fn run(state: AppState) {
  let app = Application::new().with_assets(gpui_component_assets::Assets);

  app.run(move |cx| {
    gpui_component::init(cx);

    let opts = WindowOptions {
      window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
        None,
        size(px(1200.0), px(800.0)),
        cx,
      ))),
      titlebar: Some(TitlebarOptions { title: Some("jc".into()), ..Default::default() }),
      ..Default::default()
    };

    cx.open_window(opts, |window, cx| {
      let view = cx.new(|_cx| ProjectView::with_state(state));
      cx.new(|cx| gpui_component::Root::new(view, window, cx))
    })
    .expect("failed to open window");
  });
}
