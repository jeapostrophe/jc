use serde::{Deserialize, Serialize};

const DARK_THEME_TOML: &str = include_str!("../../../data/dark_theme.toml");
const LIGHT_THEME_TOML: &str = include_str!("../../../data/light_theme.toml");

/// Which appearance variant is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Appearance {
  #[default]
  Dark,
  Light,
}

impl Appearance {
  pub fn is_dark(self) -> bool {
    matches!(self, Self::Dark)
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
  pub palette: PaletteColors,
  #[serde(default)]
  pub ui: UiColors,
  #[serde(default)]
  pub editor: EditorColors,
  #[serde(default)]
  pub syntax: SyntaxColors,
}

impl Default for ThemeConfig {
  fn default() -> Self {
    Self::dark()
  }
}

impl ThemeConfig {
  /// The built-in dark theme (Tomorrow Night).
  pub fn dark() -> Self {
    toml::from_str(DARK_THEME_TOML).expect("embedded dark theme TOML must be valid")
  }

  /// The built-in light theme (Tomorrow).
  pub fn light() -> Self {
    toml::from_str(LIGHT_THEME_TOML).expect("embedded light theme TOML must be valid")
  }

  /// Return the theme for the given appearance.
  pub fn for_appearance(appearance: Appearance) -> Self {
    match appearance {
      Appearance::Dark => Self::dark(),
      Appearance::Light => Self::light(),
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaletteColors {
  pub foreground: String,
  pub background: String,
  pub cursor: String,
  pub black: String,
  pub red: String,
  pub green: String,
  pub yellow: String,
  pub blue: String,
  pub magenta: String,
  pub cyan: String,
  pub white: String,
  pub bright_black: String,
  pub bright_red: String,
  pub bright_green: String,
  pub bright_yellow: String,
  pub bright_blue: String,
  pub bright_magenta: String,
  pub bright_cyan: String,
  pub bright_white: String,
}

impl Default for PaletteColors {
  fn default() -> Self {
    ThemeConfig::default().palette
  }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiColors {
  pub border: Option<String>,
  pub muted: Option<String>,
  pub accent: Option<String>,
  pub selection: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EditorColors {
  pub background: Option<String>,
  pub foreground: Option<String>,
  pub active_line: Option<String>,
  pub line_number: Option<String>,
  pub active_line_number: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyntaxColors {
  pub keyword: Option<String>,
  pub string: Option<String>,
  pub comment: Option<String>,
  pub function: Option<String>,
  pub number: Option<String>,
  #[serde(rename = "type")]
  pub type_: Option<String>,
  pub constant: Option<String>,
  pub boolean: Option<String>,
  pub variable: Option<String>,
  pub property: Option<String>,
  pub operator: Option<String>,
  pub tag: Option<String>,
  pub attribute: Option<String>,
  pub punctuation: Option<String>,
  pub title: Option<String>,
  pub constructor: Option<String>,
}
