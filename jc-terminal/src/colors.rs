use alacritty_terminal::vte::ansi::{Color, NamedColor};
use gpui::Hsla;
use jc_core::theme::{Appearance, PaletteColors, ThemeConfig};

/// Convert r, g, b (0-255) to GPUI Hsla.
fn rgb_to_hsla(r: u8, g: u8, b: u8) -> Hsla {
  gpui::rgba(((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | 0xFF).into()
}

/// Parse a hex color string (`#RRGGBB` or `#RRGGBBAA`) to GPUI Hsla.
pub fn hex_to_hsla(hex: &str) -> Hsla {
  let hex = hex.trim_start_matches('#');
  let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
  let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
  let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
  let a = if hex.len() >= 8 { u8::from_str_radix(&hex[6..8], 16).unwrap_or(0xFF) } else { 0xFF };
  gpui::rgba(((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | (a as u32)).into()
}

/// Standard dark terminal palette.
#[derive(Clone)]
pub struct Palette {
  pub foreground: Hsla,
  pub background: Hsla,
  pub cursor: Hsla,
  ansi: [Hsla; 256],
}

impl Default for Palette {
  fn default() -> Self {
    Palette::from(&PaletteColors::default())
  }
}

impl Palette {
  /// Build a palette for the given appearance.
  pub fn for_appearance(appearance: Appearance) -> Self {
    Palette::from(&ThemeConfig::for_appearance(appearance).palette)
  }
}

impl From<&PaletteColors> for Palette {
  fn from(palette: &PaletteColors) -> Self {
    let mut ansi = [rgb_to_hsla(0, 0, 0); 256];

    // Standard 16 ANSI colors from theme
    ansi[NamedColor::Black as usize] = hex_to_hsla(&palette.black);
    ansi[NamedColor::Red as usize] = hex_to_hsla(&palette.red);
    ansi[NamedColor::Green as usize] = hex_to_hsla(&palette.green);
    ansi[NamedColor::Yellow as usize] = hex_to_hsla(&palette.yellow);
    ansi[NamedColor::Blue as usize] = hex_to_hsla(&palette.blue);
    ansi[NamedColor::Magenta as usize] = hex_to_hsla(&palette.magenta);
    ansi[NamedColor::Cyan as usize] = hex_to_hsla(&palette.cyan);
    ansi[NamedColor::White as usize] = hex_to_hsla(&palette.white);
    // Bright
    ansi[NamedColor::BrightBlack as usize] = hex_to_hsla(&palette.bright_black);
    ansi[NamedColor::BrightRed as usize] = hex_to_hsla(&palette.bright_red);
    ansi[NamedColor::BrightGreen as usize] = hex_to_hsla(&palette.bright_green);
    ansi[NamedColor::BrightYellow as usize] = hex_to_hsla(&palette.bright_yellow);
    ansi[NamedColor::BrightBlue as usize] = hex_to_hsla(&palette.bright_blue);
    ansi[NamedColor::BrightMagenta as usize] = hex_to_hsla(&palette.bright_magenta);
    ansi[NamedColor::BrightCyan as usize] = hex_to_hsla(&palette.bright_cyan);
    ansi[NamedColor::BrightWhite as usize] = hex_to_hsla(&palette.bright_white);

    // 216-color cube (indices 16..232)
    for i in 0..216u8 {
      let r = if i / 36 > 0 { (i / 36) * 40 + 55 } else { 0 };
      let g = if (i / 6) % 6 > 0 { ((i / 6) % 6) * 40 + 55 } else { 0 };
      let b = if i % 6 > 0 { (i % 6) * 40 + 55 } else { 0 };
      ansi[16 + i as usize] = rgb_to_hsla(r, g, b);
    }

    // 24-step grayscale (indices 232..256)
    for i in 0..24u8 {
      let v = i * 10 + 8;
      ansi[232 + i as usize] = rgb_to_hsla(v, v, v);
    }

    Self {
      foreground: hex_to_hsla(&palette.foreground),
      background: hex_to_hsla(&palette.background),
      cursor: hex_to_hsla(&palette.cursor),
      ansi,
    }
  }
}

impl Palette {
  /// Resolve an alacritty Color to GPUI Hsla.
  pub fn resolve(&self, color: &Color) -> Hsla {
    match color {
      Color::Named(name) => self.ansi[*name as usize],
      Color::Spec(rgb) => rgb_to_hsla(rgb.r, rgb.g, rgb.b),
      Color::Indexed(idx) => self.ansi[*idx as usize],
    }
  }

  /// Resolve foreground color, applying default.
  pub fn resolve_fg(&self, color: &Color) -> Hsla {
    match color {
      Color::Named(NamedColor::Foreground) => self.foreground,
      other => self.resolve(other),
    }
  }

  /// Resolve background color, applying default.
  pub fn resolve_bg(&self, color: &Color) -> Hsla {
    match color {
      Color::Named(NamedColor::Background) => self.background,
      other => self.resolve(other),
    }
  }
}
