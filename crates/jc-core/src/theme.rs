use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
  pub terminal: TerminalTheme,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
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
    Self {
      foreground: "#C5C8C6".to_string(),
      background: "#1D1F21".to_string(),
      cursor: "#C5C8C6".to_string(),
      black: "#1D1F21".to_string(),
      red: "#CC6666".to_string(),
      green: "#B5BD68".to_string(),
      yellow: "#F0C674".to_string(),
      blue: "#81A2BE".to_string(),
      magenta: "#B294BB".to_string(),
      cyan: "#8ABEB7".to_string(),
      white: "#C5C8C6".to_string(),
      bright_black: "#969896".to_string(),
      bright_red: "#DE935F".to_string(),
      bright_green: "#B5BD68".to_string(),
      bright_yellow: "#F0C674".to_string(),
      bright_blue: "#81A2BE".to_string(),
      bright_magenta: "#B294BB".to_string(),
      bright_cyan: "#8ABEB7".to_string(),
      bright_white: "#FFFFFF".to_string(),
    }
  }
}
