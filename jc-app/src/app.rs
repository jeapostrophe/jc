use std::borrow::Cow;
use std::rc::Rc;
use std::sync::Arc;

use crate::views::close_confirm;
use crate::views::code_view;
use crate::views::keybinding_help;
use crate::views::picker;
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
  // Status fills (banners are `warning`), each with text that reads on it.
  for (status, fill) in
    [("warning", &p.yellow), ("danger", &p.red), ("info", &p.cyan), ("success", &p.green)]
  {
    let text = text_on(fill, &[&p.background, &p.foreground]);
    colors_json.insert(format!("{status}.background"), serde_json::Value::String(fill.clone()));
    colors_json.insert(format!("{status}.foreground"), serde_json::Value::String(text));
  }
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

/// Text for a fill of `fill`: whichever of the theme's own `colors` contrasts
/// most with it, if that reaches WCAG AA for body text (4.5:1), else black or
/// white, whichever contrasts more. gpui-component's fallback, the theme's
/// text colour, was light grey on yellow in the dark theme (Q-165).
fn text_on(fill: &str, colors: &[&str]) -> String {
  let luminance = |hex: &str| {
    let gpui::Rgba { r, g, b, .. } = hex_to_hsla(hex).to_rgb();
    let lin = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
  };
  let fill = luminance(fill);
  let ratio = |hex: &str| {
    let l = luminance(hex);
    (l.max(fill) + 0.05) / (l.min(fill) + 0.05)
  };
  let best = |candidates: &[&str]| {
    candidates.iter().copied().max_by(|a, b| ratio(a).total_cmp(&ratio(b))).unwrap().to_string()
  };
  let own = best(colors);
  if ratio(&own) >= 4.5 { own } else { best(&["#000000", "#FFFFFF"]) }
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
  theme.dark_theme = dark_config;
  theme.light_theme = light_config;

  // Apply the highlight theme for the current mode.
  let current_config = if theme.mode.is_dark() { &theme.dark_theme } else { &theme.light_theme };
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

pub fn run(state: AppState, config: AppConfig, ipc_rx: flume::Receiver<std::path::PathBuf>) {
  let app = Application::new().with_assets(gpui_component_assets::Assets);

  app.run(move |cx| {
    gpui_component::init(cx);
    apply_unified_themes(cx);
    register_todo_language();
    jc_terminal::init(cx);
    workspace::init(cx);
    picker::init(cx);
    code_view::init(cx);
    keybinding_help::init(cx);
    close_confirm::init(cx);

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
      window_bounds: Some(WindowBounds::Maximized(Bounds::centered(
        None,
        size(px(1200.0), px(800.0)),
        cx,
      ))),
      titlebar: Some(TitleBar::title_bar_options()),
      ..Default::default()
    };

    cx.open_window(opts, |window, cx| {
      window.activate_window();
      let view = cx.new(|cx| Workspace::new(state, config, ipc_rx, window, cx));
      cx.new(|cx| gpui_component::Root::new(view, window, cx))
    })
    .expect("failed to open window");

    cx.activate(true);
  });
}

#[cfg(test)]
mod tests {
  use super::build_gpui_theme;
  use gpui::{Hsla, Rgba};
  use gpui_component::theme::{Theme, ThemeColor, ThemeMode};
  use jc_core::theme::ThemeConfig;
  use std::rc::Rc;

  /// WCAG 2 relative luminance of an opaque colour.
  fn luminance(c: Hsla) -> f32 {
    let Rgba { r, g, b, .. } = c.to_rgb();
    let lin = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
  }

  fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
  }

  /// Q-165: jcc's banners were light grey on yellow in the dark theme
  /// (`warning.foreground` fell back to the text colour), and Jay could not
  /// read them. Every banner is `warning` text on `warning`; the other status
  /// colours are filled the same way by gpui-component. The colours are the
  /// ones gpui resolves from the themes jc ships, so a fallback counts.
  #[test]
  fn status_text_is_readable_on_its_fill_in_every_theme() {
    for (unified, mode) in
      [(ThemeConfig::dark(), ThemeMode::Dark), (ThemeConfig::light(), ThemeMode::Light)]
    {
      let mut theme = Theme::from(ThemeColor::dark().as_ref());
      theme.apply_config(&Rc::new(build_gpui_theme(&unified, mode)));
      for (what, fill, text) in [
        ("warning", theme.warning, theme.warning_foreground),
        ("danger", theme.danger, theme.danger_foreground),
        ("info", theme.info, theme.info_foreground),
        ("success", theme.success, theme.success_foreground),
      ] {
        assert_eq!(fill.a, 1.0, "{mode:?} {what}: an opaque fill");
        let ratio = contrast(fill, text);
        assert!(ratio >= 4.5, "{mode:?} {what}: contrast {ratio:.2} < 4.5 ({fill:?} on {text:?})");
      }
    }
  }
}
