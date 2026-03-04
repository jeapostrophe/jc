use std::borrow::Cow;
use std::rc::Rc;
use std::sync::Arc;

use crate::views::code_view;
use crate::views::comment_panel;
use crate::views::diff_view;
use crate::views::picker;
use crate::views::reply_view;
use crate::views::workspace::{self, Workspace};
use gpui::*;
use gpui_component::TitleBar;
use gpui_component::highlighter::{LanguageConfig, LanguageRegistry};
use gpui_component::theme::{Theme, ThemeMode};
use jc_core::config::{AppConfig, AppState};
use jc_core::theme::ThemeConfig;
use jc_terminal::colors::hex_to_hsla;

/// Build a gpui-component `ThemeConfig` from a `jc_core::ThemeConfig`.
fn build_gpui_theme(unified: &ThemeConfig, mode: ThemeMode) -> gpui_component::theme::ThemeConfig {
  let p = &unified.palette;

  // Build ThemeConfigColors via serde_json since some fields (base colors) are private.
  let mut colors_json = serde_json::Map::new();
  colors_json.insert("background".into(), serde_json::Value::String(p.background.clone()));
  colors_json.insert("foreground".into(), serde_json::Value::String(p.foreground.clone()));
  colors_json.insert(
    "border".into(),
    serde_json::Value::String(unified.ui.border.clone().unwrap_or_else(|| p.bright_black.clone())),
  );
  colors_json.insert(
    "muted.background".into(),
    serde_json::Value::String(unified.ui.muted.clone().unwrap_or_else(|| p.bright_black.clone())),
  );
  colors_json.insert("muted.foreground".into(), serde_json::Value::String(p.bright_black.clone()));
  colors_json.insert(
    "accent.background".into(),
    serde_json::Value::String(unified.ui.accent.clone().unwrap_or_else(|| p.bright_black.clone())),
  );
  colors_json.insert(
    "selection.background".into(),
    serde_json::Value::String(
      unified.ui.selection.clone().unwrap_or_else(|| p.bright_black.clone()),
    ),
  );
  // Base ANSI colors
  colors_json.insert("base.red".into(), serde_json::Value::String(p.red.clone()));
  colors_json.insert("base.green".into(), serde_json::Value::String(p.green.clone()));
  colors_json.insert("base.blue".into(), serde_json::Value::String(p.blue.clone()));
  colors_json.insert("base.yellow".into(), serde_json::Value::String(p.yellow.clone()));
  colors_json.insert("base.magenta".into(), serde_json::Value::String(p.magenta.clone()));
  colors_json.insert("base.cyan".into(), serde_json::Value::String(p.cyan.clone()));
  // Primary = blue from palette
  colors_json.insert("primary.background".into(), serde_json::Value::String(p.blue.clone()));
  colors_json.insert("caret".into(), serde_json::Value::String(p.cursor.clone()));

  let colors: gpui_component::theme::ThemeConfigColors =
    serde_json::from_value(serde_json::Value::Object(colors_json))
      .expect("unified theme colors JSON must be valid");

  // Build syntax highlight style from the unified theme.
  let s = &unified.syntax;
  let e = &unified.editor;
  let syntax_style = build_syntax_colors(s);

  let editor_bg = e.background.as_deref().unwrap_or(&p.background);
  let editor_fg = e.foreground.as_deref().unwrap_or(&p.foreground);

  let highlight = gpui_component::highlighter::HighlightThemeStyle {
    editor_background: Some(hex_to_hsla(editor_bg)),
    editor_foreground: Some(hex_to_hsla(editor_fg)),
    editor_active_line: e.active_line.as_deref().map(hex_to_hsla),
    editor_line_number: e.line_number.as_deref().map(hex_to_hsla),
    editor_active_line_number: e.active_line_number.as_deref().map(hex_to_hsla),
    syntax: syntax_style,
    ..Default::default()
  };

  let name: SharedString = if mode.is_dark() { "JC Dark".into() } else { "JC Light".into() };

  gpui_component::theme::ThemeConfig {
    is_default: true,
    name,
    mode,
    colors,
    highlight: Some(highlight),
    ..Default::default()
  }
}

fn build_syntax_colors(
  s: &jc_core::theme::SyntaxColors,
) -> gpui_component::highlighter::SyntaxColors {
  fn style(hex: &Option<String>) -> Option<gpui_component::highlighter::ThemeStyle> {
    hex.as_deref().map(|h| {
      serde_json::from_value(serde_json::json!({ "color": h }))
        .expect("theme style JSON must be valid")
    })
  }

  gpui_component::highlighter::SyntaxColors {
    keyword: style(&s.keyword),
    string: style(&s.string),
    comment: style(&s.comment),
    function: style(&s.function),
    number: style(&s.number),
    type_: style(&s.type_),
    constant: style(&s.constant),
    boolean: style(&s.boolean),
    variable: style(&s.variable),
    property: style(&s.property),
    operator: style(&s.operator),
    tag: style(&s.tag),
    attribute: style(&s.attribute),
    punctuation: style(&s.punctuation),
    title: style(&s.title),
    constructor: style(&s.constructor),
    ..Default::default()
  }
}

/// Apply both dark and light unified themes to the gpui-component Theme global.
fn apply_unified_themes(cx: &mut App) {
  let dark_config = Rc::new(build_gpui_theme(&ThemeConfig::dark(), ThemeMode::Dark));
  let light_config = Rc::new(build_gpui_theme(&ThemeConfig::light(), ThemeMode::Light));

  let theme = Theme::global_mut(cx);
  theme.dark_theme = dark_config.clone();
  theme.light_theme = light_config.clone();

  // Apply the highlight theme for the current mode.
  let current_config = if theme.mode.is_dark() { &dark_config } else { &light_config };
  if let Some(style) = &current_config.highlight {
    theme.highlight_theme = Arc::new(gpui_component::highlighter::HighlightTheme {
      name: current_config.name.to_string(),
      appearance: current_config.mode,
      style: style.clone(),
    });
  }

  Theme::sync_system_appearance(None, cx);
}

/// Register a custom "todo-markdown" tree-sitter language that extends the
/// base markdown highlights with custom heading patterns for TODO sessions.
///
/// Uses heading level as a proxy for content type (h2 = Session, h3 = Message/WAIT).
/// `#match?` predicates are not evaluated by gpui-component's highlighter, so we
/// rely purely on structural patterns instead.
fn register_todo_language() {
  let registry = LanguageRegistry::singleton();
  let Some(md) = registry.language("markdown") else {
    eprintln!("todo-markdown: base 'markdown' language not found in registry");
    return;
  };
  // Use base markdown highlighting only — active session headings are
  // highlighted dynamically via extra_highlights in TodoView.
  let config = LanguageConfig::new(
    "todo-markdown",
    md.language.clone(),
    md.injection_languages.clone(),
    &md.highlights,
    &md.injections,
    &md.locals,
  );
  registry.register("todo-markdown", &config);
}

pub fn run(state: AppState, config: AppConfig) {
  let app = Application::new().with_assets(gpui_component_assets::Assets);

  app.run(move |cx| {
    gpui_component::init(cx);
    apply_unified_themes(cx);
    register_todo_language();
    jc_terminal::init(cx);
    workspace::init(cx);
    picker::init(cx);
    code_view::init(cx);
    diff_view::init(cx);
    reply_view::init(cx);
    comment_panel::init(cx);

    // Register the bundled Lilex font family.
    cx.text_system()
      .add_fonts(vec![
        Cow::Borrowed(include_bytes!("../../data/fonts/Lilex-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../../data/fonts/Lilex-Bold.ttf")),
        Cow::Borrowed(include_bytes!("../../data/fonts/Lilex-Italic.ttf")),
        Cow::Borrowed(include_bytes!("../../data/fonts/Lilex-BoldItalic.ttf")),
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
      let view = cx.new(|cx| Workspace::new(state, config, window, cx));
      cx.new(|cx| gpui_component::Root::new(view, window, cx))
    })
    .expect("failed to open window");

    cx.activate(true);
  });
}
