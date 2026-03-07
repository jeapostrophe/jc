use alacritty_terminal::term::TermMode;
use gpui::Keystroke;

/// Convert a GPUI keystroke into terminal byte sequences.
pub fn keystroke_to_bytes(keystroke: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
  // Handle Ctrl+key combos.  Use `keystroke.key` (the base key name, always
  // populated) rather than `key_char` which GPUI sets to None when Ctrl is
  // held on macOS.
  if keystroke.modifiers.control
    && let Some(ch) = keystroke.key.chars().next()
  {
    let byte = match ch {
      'a'..='z' => Some(ch as u8 - b'a' + 1),
      '@' => Some(0),
      '[' => Some(0x1B),
      '\\' => Some(0x1C),
      ']' => Some(0x1D),
      '^' => Some(0x1E),
      '_' => Some(0x1F),
      _ => None,
    };
    if let Some(b) = byte {
      return Some(vec![b]);
    }
  }

  let key = keystroke.key_char.as_deref().unwrap_or("");

  // Handle special named keys
  let app_cursor = mode.contains(TermMode::APP_CURSOR);
  match keystroke.key.as_str() {
    "enter" => return Some(b"\r".to_vec()),
    "escape" => return Some(b"\x1b".to_vec()),
    "tab" => return Some(b"\t".to_vec()),
    "backspace" => return Some(vec![0x7f]),
    "delete" => return Some(b"\x1b[3~".to_vec()),
    "up" => return Some(if app_cursor { b"\x1bOA".to_vec() } else { b"\x1b[A".to_vec() }),
    "down" => return Some(if app_cursor { b"\x1bOB".to_vec() } else { b"\x1b[B".to_vec() }),
    "right" => return Some(if app_cursor { b"\x1bOC".to_vec() } else { b"\x1b[C".to_vec() }),
    "left" => return Some(if app_cursor { b"\x1bOD".to_vec() } else { b"\x1b[D".to_vec() }),
    "home" => return Some(if app_cursor { b"\x1bOH".to_vec() } else { b"\x1b[H".to_vec() }),
    "end" => return Some(if app_cursor { b"\x1bOF".to_vec() } else { b"\x1b[F".to_vec() }),
    "pageup" => return Some(b"\x1b[5~".to_vec()),
    "pagedown" => return Some(b"\x1b[6~".to_vec()),
    "insert" => return Some(b"\x1b[2~".to_vec()),
    "f1" => return Some(b"\x1bOP".to_vec()),
    "f2" => return Some(b"\x1bOQ".to_vec()),
    "f3" => return Some(b"\x1bOR".to_vec()),
    "f4" => return Some(b"\x1bOS".to_vec()),
    "f5" => return Some(b"\x1b[15~".to_vec()),
    "f6" => return Some(b"\x1b[17~".to_vec()),
    "f7" => return Some(b"\x1b[18~".to_vec()),
    "f8" => return Some(b"\x1b[19~".to_vec()),
    "f9" => return Some(b"\x1b[20~".to_vec()),
    "f10" => return Some(b"\x1b[21~".to_vec()),
    "f11" => return Some(b"\x1b[23~".to_vec()),
    "f12" => return Some(b"\x1b[24~".to_vec()),
    "space" => return Some(b" ".to_vec()),
    _ => {}
  }

  // Regular character input
  if !keystroke.modifiers.control && !key.is_empty() {
    let mut bytes = Vec::new();
    // Alt prefix
    if keystroke.modifiers.alt {
      bytes.push(0x1b);
    }
    bytes.extend_from_slice(key.as_bytes());
    return Some(bytes);
  }

  None
}
