use serde::{Deserialize, Serialize};

const DEFAULT_THEME_TOML: &str = include_str!("../../../data/default_theme.toml");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
  pub terminal: TerminalTheme,
}

impl Default for ThemeConfig {
  fn default() -> Self {
    toml::from_str(DEFAULT_THEME_TOML).expect("embedded default_theme.toml must be valid")
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalTheme {
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

impl Default for TerminalTheme {
  fn default() -> Self {
    ThemeConfig::default().terminal
  }
}
