use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::{Input, InputState};

const SAMPLE_CODE: &str = r#"fn main() {
    let greeting = "Hello, world!";
    println!("{greeting}");

    for i in 0..10 {
        if i % 2 == 0 {
            println!("{i} is even");
        }
    }
}
"#;

struct CodeEditorDemo {
  editor: Entity<InputState>,
}

impl CodeEditorDemo {
  fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
    let editor = cx.new(|cx| InputState::new(window, cx).code_editor("rust").soft_wrap(false));
    editor.update(cx, |state, cx| {
      state.set_value(SAMPLE_CODE, window, cx);
    });
    Self { editor }
  }
}

impl Render for CodeEditorDemo {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();
    div()
      .size_full()
      .bg(theme.background)
      .child(Input::new(&self.editor).h_full().appearance(false).bordered(false))
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
      titlebar: Some(TitlebarOptions {
        title: Some("Code Editor Demo".into()),
        ..Default::default()
      }),
      ..Default::default()
    };

    cx.open_window(opts, |window, cx| {
      let view = cx.new(|cx| CodeEditorDemo::new(window, cx));
      cx.new(|cx| gpui_component::Root::new(view, window, cx))
    })
    .expect("failed to open window");
  });
}
