use alacritty_terminal::vte::ansi::{Color, NamedColor};
use gpui::Hsla;

/// Convert r, g, b (0-255) to GPUI Hsla.
fn rgb_to_hsla(r: u8, g: u8, b: u8) -> Hsla {
  gpui::rgba(((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | 0xFF).into()
}

/// Standard dark terminal palette.
pub struct Palette {
  pub foreground: Hsla,
  pub background: Hsla,
  pub cursor: Hsla,
  ansi: [Hsla; 256],
}

impl Default for Palette {
  fn default() -> Self {
    let mut ansi = [rgb_to_hsla(0, 0, 0); 256];

    // Standard 16 ANSI colors (dark theme)
    ansi[NamedColor::Black as usize] = rgb_to_hsla(0x1D, 0x1F, 0x21);
    ansi[NamedColor::Red as usize] = rgb_to_hsla(0xCC, 0x66, 0x66);
    ansi[NamedColor::Green as usize] = rgb_to_hsla(0xB5, 0xBD, 0x68);
    ansi[NamedColor::Yellow as usize] = rgb_to_hsla(0xF0, 0xC6, 0x74);
    ansi[NamedColor::Blue as usize] = rgb_to_hsla(0x81, 0xA2, 0xBE);
    ansi[NamedColor::Magenta as usize] = rgb_to_hsla(0xB2, 0x94, 0xBB);
    ansi[NamedColor::Cyan as usize] = rgb_to_hsla(0x8A, 0xBE, 0xB7);
    ansi[NamedColor::White as usize] = rgb_to_hsla(0xC5, 0xC8, 0xC6);
    // Bright
    ansi[NamedColor::BrightBlack as usize] = rgb_to_hsla(0x96, 0x98, 0x96);
    ansi[NamedColor::BrightRed as usize] = rgb_to_hsla(0xDE, 0x93, 0x5F);
    ansi[NamedColor::BrightGreen as usize] = rgb_to_hsla(0xB5, 0xBD, 0x68);
    ansi[NamedColor::BrightYellow as usize] = rgb_to_hsla(0xF0, 0xC6, 0x74);
    ansi[NamedColor::BrightBlue as usize] = rgb_to_hsla(0x81, 0xA2, 0xBE);
    ansi[NamedColor::BrightMagenta as usize] = rgb_to_hsla(0xB2, 0x94, 0xBB);
    ansi[NamedColor::BrightCyan as usize] = rgb_to_hsla(0x8A, 0xBE, 0xB7);
    ansi[NamedColor::BrightWhite as usize] = rgb_to_hsla(0xFF, 0xFF, 0xFF);

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
      foreground: rgb_to_hsla(0xC5, 0xC8, 0xC6),
      background: rgb_to_hsla(0x1D, 0x1F, 0x21),
      cursor: rgb_to_hsla(0xC5, 0xC8, 0xC6),
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
