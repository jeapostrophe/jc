use std::rc::Rc;
use std::{cell::RefCell, ops::Range};

use gpui::{App, SharedString};
use ropey::Rope;
use tree_sitter::InputEdit;

use super::text_wrapper::TextWrapper;
use crate::highlighter::DiagnosticSet;
use crate::highlighter::SyntaxHighlighter;
use crate::input::{RopeExt as _, TabSize};

#[derive(Clone)]
pub(crate) enum InputMode {
  /// A plain text input mode.
  PlainText { multi_line: bool, tab: TabSize, rows: usize },
  /// An auto grow input mode.
  AutoGrow { rows: usize, min_rows: usize, max_rows: usize },
  /// A code editor input mode.
  CodeEditor {
    multi_line: bool,
    tab: TabSize,
    rows: usize,
    /// Show line number
    line_number: bool,
    language: SharedString,
    indent_guides: bool,
    highlighter: Rc<RefCell<Option<SyntaxHighlighter>>>,
    diagnostics: DiagnosticSet,
  },
}

impl Default for InputMode {
  fn default() -> Self {
    InputMode::plain_text()
  }
}

#[allow(unused)]
impl InputMode {
  /// Create a plain input mode with default settings.
  pub(super) fn plain_text() -> Self {
    InputMode::PlainText { multi_line: false, tab: TabSize::default(), rows: 1 }
  }

  /// Create a code editor input mode with default settings.
  pub(super) fn code_editor(language: impl Into<SharedString>) -> Self {
    InputMode::CodeEditor {
      rows: 2,
      multi_line: true,
      tab: TabSize::default(),
      language: language.into(),
      highlighter: Rc::new(RefCell::new(None)),
      line_number: true,
      indent_guides: true,
      diagnostics: DiagnosticSet::new(&Rope::new()),
    }
  }

  /// Create an auto grow input mode with given min and max rows.
  pub(super) fn auto_grow(min_rows: usize, max_rows: usize) -> Self {
    InputMode::AutoGrow { rows: min_rows, min_rows, max_rows }
  }

  pub(super) fn multi_line(mut self, multi_line: bool) -> Self {
    match &mut self {
      InputMode::PlainText { multi_line: ml, .. } => *ml = multi_line,
      InputMode::CodeEditor { multi_line: ml, .. } => *ml = multi_line,
      InputMode::AutoGrow { .. } => {}
    }
    self
  }

  #[inline]
  pub(super) fn is_single_line(&self) -> bool {
    !self.is_multi_line()
  }

  #[inline]
  pub(super) fn is_code_editor(&self) -> bool {
    matches!(self, InputMode::CodeEditor { .. })
  }

  #[inline]
  pub(super) fn is_auto_grow(&self) -> bool {
    matches!(self, InputMode::AutoGrow { .. })
  }

  #[inline]
  pub(super) fn is_multi_line(&self) -> bool {
    match self {
      InputMode::PlainText { multi_line, .. } => *multi_line,
      InputMode::CodeEditor { multi_line, .. } => *multi_line,
      InputMode::AutoGrow { max_rows, .. } => *max_rows > 1,
    }
  }

  pub(super) fn set_rows(&mut self, new_rows: usize) {
    match self {
      InputMode::PlainText { rows, .. } => {
        *rows = new_rows;
      }
      InputMode::CodeEditor { rows, .. } => {
        *rows = new_rows;
      }
      InputMode::AutoGrow { rows, min_rows, max_rows } => {
        *rows = new_rows.clamp(*min_rows, *max_rows);
      }
    }
  }

  pub(super) fn update_auto_grow(&mut self, text_wrapper: &TextWrapper) {
    if self.is_single_line() {
      return;
    }

    let wrapped_lines = text_wrapper.len();
    self.set_rows(wrapped_lines);
  }

  /// At least 1 row be return.
  pub(super) fn rows(&self) -> usize {
    if !self.is_multi_line() {
      return 1;
    }

    match self {
      InputMode::PlainText { rows, .. } => *rows,
      InputMode::CodeEditor { rows, .. } => *rows,
      InputMode::AutoGrow { rows, .. } => *rows,
    }
    .max(1)
  }

  /// At least 1 row be return.
  #[allow(unused)]
  pub(super) fn min_rows(&self) -> usize {
    match self {
      InputMode::AutoGrow { min_rows, .. } => *min_rows,
      _ => 1,
    }
    .max(1)
  }

  #[allow(unused)]
  pub(super) fn max_rows(&self) -> usize {
    if !self.is_multi_line() {
      return 1;
    }

    match self {
      InputMode::AutoGrow { max_rows, .. } => *max_rows,
      _ => usize::MAX,
    }
  }

  /// Return false if the mode is not [`InputMode::CodeEditor`].
  #[allow(unused)]
  #[inline]
  pub(super) fn line_number(&self) -> bool {
    match self {
      InputMode::CodeEditor { line_number, multi_line, .. } => *line_number && *multi_line,
      _ => false,
    }
  }

  /// Reparse `text` after `replaced_range` (a byte range in `old_text`) was
  /// replaced by `new_text`.
  ///
  /// Does nothing outside [`InputMode::CodeEditor`].
  pub(super) fn update_highlighter(
    &mut self,
    replaced_range: &Range<usize>,
    old_text: &Rope,
    text: &Rope,
    new_text: &str,
    cx: &mut App,
  ) {
    let edit = input_edit(replaced_range, old_text, text, new_text);
    self.parse_highlighter(Some(edit), text, cx);
  }

  /// Build the highlighter and parse `text` if there is no highlighter yet.
  ///
  /// Does nothing outside [`InputMode::CodeEditor`], or if one already exists --
  /// use [`InputMode::update_highlighter`] to reparse after an edit.
  pub(super) fn ensure_highlighter(&mut self, text: &Rope, cx: &mut App) {
    let InputMode::CodeEditor { highlighter, .. } = &self else {
      return;
    };
    if highlighter.borrow().is_some() {
      return;
    }

    self.parse_highlighter(None, text, cx);
  }

  fn parse_highlighter(&mut self, edit: Option<InputEdit>, text: &Rope, _cx: &mut App) {
    let InputMode::CodeEditor { language, highlighter, .. } = &self else {
      return;
    };

    let mut highlighter = highlighter.borrow_mut();
    if highlighter.is_none() {
      highlighter.replace(SyntaxHighlighter::new(language));
    }

    // A highlighter built just now has parsed nothing, so `edit` describes a
    // change to text it never saw; `SyntaxHighlighter::update` rejects an edit
    // that does not fit what it last parsed and reparses in full instead.
    if let Some(highlighter) = highlighter.as_mut() {
      highlighter.update(edit, text);
    }
  }

  #[allow(unused)]
  pub(super) fn diagnostics(&self) -> Option<&DiagnosticSet> {
    match self {
      InputMode::CodeEditor { diagnostics, .. } => Some(diagnostics),
      _ => None,
    }
  }

  pub(super) fn diagnostics_mut(&mut self) -> Option<&mut DiagnosticSet> {
    match self {
      InputMode::CodeEditor { diagnostics, .. } => Some(diagnostics),
      _ => None,
    }
  }
}

/// Build the [`InputEdit`] describing a replacement of `replaced_range` (a byte
/// range in `old_text`) by `new_text`, which produced `text`.
///
/// `replaced_range` is expressed in the *pre-edit* coordinate space, so its
/// endpoints and their `Point`s must be resolved against `old_text`. Clamping
/// them against the post-edit `text` (which is shorter after a deletion) reports
/// a shorter deletion than actually happened, leaving tree-sitter holding stale
/// node offsets that it then resolves against the new text -- where they can
/// land in the middle of a multi-byte character.
fn input_edit(
  replaced_range: &Range<usize>,
  old_text: &Rope,
  text: &Rope,
  new_text: &str,
) -> InputEdit {
  // Clip through the same helper `RopeExt::replace` applied the range with, so
  // the byte offsets, the `Point`s and the bytes the rope actually removed all
  // agree.
  let Range { start, end: old_end } = old_text.clip_range(replaced_range.clone());
  // `text` is normally `old_text` spliced, which cannot be shorter than
  // `start + new_text.len()` -- but `InputState::replace_text_in_range` can
  // substitute a masked rope, so keep the edit inside the text it describes.
  let new_end = (start + new_text.len()).min(text.len());

  InputEdit {
    start_byte: start,
    old_end_byte: old_end,
    new_end_byte: new_end,
    start_position: old_text.offset_to_point(start),
    old_end_position: old_text.offset_to_point(old_end),
    new_end_position: text.offset_to_point(new_end),
  }
}

#[cfg(test)]
mod tests {
  use ropey::Rope;

  use crate::{
    highlighter::DiagnosticSet,
    input::{
      RopeExt as _, TabSize,
      mode::{InputMode, input_edit},
    },
  };

  /// Apply `new_text` over `range` the way `InputState::replace_text_in_range`
  /// does, and return the resulting edit alongside the post-edit text.
  ///
  /// Every edit is checked against the rope mutation it claims to describe, so
  /// the expectation is derived from what actually happened rather than
  /// recomputed from the same formula the function under test uses.
  fn apply(
    old: &Rope,
    range: std::ops::Range<usize>,
    new_text: &str,
  ) -> (Rope, tree_sitter::InputEdit) {
    let mut text = old.clone();
    text.replace(range.clone(), new_text);
    let edit = input_edit(&range, old, &text, new_text);
    assert_describes_replacement(&edit, old, &text, new_text);
    (text, edit)
  }

  /// Splicing `new_text` into `old` at the edit's own byte offsets must
  /// reproduce `text` exactly, and each `Point` must agree with the byte offset
  /// beside it. This is the whole contract tree-sitter relies on: an edit that
  /// understates the replaced span leaves it holding nodes for bytes that no
  /// longer exist.
  fn assert_describes_replacement(
    edit: &tree_sitter::InputEdit,
    old: &Rope,
    text: &Rope,
    new_text: &str,
  ) {
    let old_source = old.to_string();
    let spliced =
      format!("{}{}{}", &old_source[..edit.start_byte], new_text, &old_source[edit.old_end_byte..]);
    assert_eq!(spliced, text.to_string(), "edit does not describe the replacement: {edit:?}");

    assert_eq!(edit.start_position, old.offset_to_point(edit.start_byte));
    assert_eq!(edit.old_end_position, old.offset_to_point(edit.old_end_byte));
    assert_eq!(edit.new_end_position, text.offset_to_point(edit.new_end_byte));
    assert_eq!(edit.new_end_byte, edit.start_byte + new_text.len());
  }

  #[test]
  fn test_code_editor() {
    let mode = InputMode::code_editor("rust");
    assert_eq!(mode.is_code_editor(), true);
    assert_eq!(mode.is_multi_line(), true);
    assert_eq!(mode.is_single_line(), false);
    assert_eq!(mode.line_number(), true);
    assert_eq!(mode.has_indent_guides(), true);
    assert_eq!(mode.max_rows(), usize::MAX);
    assert_eq!(mode.min_rows(), 1);

    let mode = InputMode::CodeEditor {
      multi_line: false,
      line_number: true,
      indent_guides: true,
      rows: 0,
      tab: Default::default(),
      language: "rust".into(),
      highlighter: Default::default(),
      diagnostics: DiagnosticSet::new(&Rope::new()),
    };
    assert_eq!(mode.is_code_editor(), true);
    assert_eq!(mode.is_multi_line(), false);
    assert_eq!(mode.is_single_line(), true);
    assert_eq!(mode.line_number(), false);
    assert_eq!(mode.has_indent_guides(), false);
    assert_eq!(mode.max_rows(), 1);
    assert_eq!(mode.min_rows(), 1);
  }

  #[test]
  fn test_plain() {
    let mode = InputMode::PlainText { multi_line: true, tab: TabSize::default(), rows: 5 };
    assert_eq!(mode.is_code_editor(), false);
    assert_eq!(mode.is_multi_line(), true);
    assert_eq!(mode.is_single_line(), false);
    assert_eq!(mode.line_number(), false);
    assert_eq!(mode.rows(), 5);
    assert_eq!(mode.max_rows(), usize::MAX);
    assert_eq!(mode.min_rows(), 1);

    let mode = InputMode::plain_text();
    assert_eq!(mode.is_code_editor(), false);
    assert_eq!(mode.is_multi_line(), false);
    assert_eq!(mode.is_single_line(), true);
    assert_eq!(mode.line_number(), false);
    assert_eq!(mode.max_rows(), 1);
    assert_eq!(mode.min_rows(), 1);
  }

  /// A deletion's `old_end_byte` lives in the *pre-edit* coordinate space.
  /// Clamping it to the (shorter) post-edit length understates how much was
  /// removed, so tree-sitter keeps nodes whose offsets no longer exist.
  #[test]
  fn test_input_edit_large_deletion() {
    let old = Rope::from("# Title\n\n- \u{2705} done\n- \u{23f3} pending\n- tail\n");
    let tail = "- tail\n";
    let deleted = 0..(old.len() - tail.len());

    let (text, edit) = apply(&old, deleted.clone(), "");

    assert_eq!(text.to_string(), tail);
    assert_eq!(edit.start_byte, deleted.start);
    assert_eq!(edit.old_end_byte, deleted.end);
    assert_eq!(edit.new_end_byte, deleted.start);
  }

  #[test]
  fn test_input_edit_backspace_and_insert() {
    let old = Rope::from("let x = \u{2705};\n");

    let backspaced = old.len() - 2..old.len() - 1;
    let (text, edit) = apply(&old, backspaced.clone(), "");
    assert_eq!(edit.start_byte, backspaced.start);
    assert_eq!(edit.old_end_byte, backspaced.end);
    assert_eq!(edit.new_end_byte, backspaced.start);
    assert_eq!(text.len(), old.len() - 1);

    let (_, edit) = apply(&old, 4..5, "yz");
    assert_eq!(edit.start_byte, 4);
    assert_eq!(edit.old_end_byte, 5);
    assert_eq!(edit.new_end_byte, 6);
  }

  /// The whole document replaced at once: the pre-edit range covers all of the
  /// old text even though the new text is far shorter.
  #[test]
  fn test_input_edit_full_replace() {
    let old = Rope::from("\u{2705}\u{2705}\u{2705}\u{2705}\u{2705}\n");
    let (text, edit) = apply(&old, 0..old.len(), "x");

    assert_eq!(edit.start_byte, 0);
    assert_eq!(edit.old_end_byte, old.len());
    assert_eq!(edit.new_end_byte, 1);
    assert_eq!(text.to_string(), "x");
  }

  /// A range that already exceeds the old text is clamped to it, not silently
  /// turned into an inverted range.
  #[test]
  fn test_input_edit_out_of_bounds_range() {
    let old = Rope::from("abc");
    let edit = input_edit(&(10..20), &old, &old, "");

    assert_eq!(edit.start_byte, old.len());
    assert_eq!(edit.old_end_byte, old.len());
    assert_eq!(edit.new_end_byte, old.len());
  }

  /// A range whose endpoints split a character is clipped by `RopeExt::replace`
  /// -- start left, end right -- so the edit must be clipped the same way.
  /// Clipping the end left (as `offset_to_point` does) or not at all reports a
  /// span that is not the one the rope removed.
  #[test]
  fn test_input_edit_clips_like_the_rope() {
    let old = Rope::from("a\u{2705}b\u{2014}c\u{23f3}d\n");
    let source = old.to_string();

    let multi: Vec<usize> =
      source.char_indices().filter(|(_, c)| c.len_utf8() > 1).map(|(i, _)| i).collect();
    assert!(multi.len() >= 2);

    // Both endpoints land inside a multi-byte character.
    let split = multi[0] + 1..multi[1] + 1;
    assert!(!source.is_char_boundary(split.start) && !source.is_char_boundary(split.end));

    let (_, edit) = apply(&old, split.clone(), "");

    assert_eq!(edit.start_byte, multi[0], "start must clip left, off {}", split.start);
    assert_eq!(
      edit.old_end_byte,
      multi[1] + source[multi[1]..].chars().next().unwrap().len_utf8(),
      "end must clip right, off {}",
      split.end
    );
  }

  #[test]
  fn test_auto_grow() {
    let mut mode = InputMode::auto_grow(2, 5);
    assert_eq!(mode.is_code_editor(), false);
    assert_eq!(mode.is_multi_line(), true);
    assert_eq!(mode.is_single_line(), false);
    assert_eq!(mode.line_number(), false);
    assert_eq!(mode.rows(), 2);
    assert_eq!(mode.max_rows(), 5);
    assert_eq!(mode.min_rows(), 2);

    mode.set_rows(4);
    assert_eq!(mode.rows(), 4);

    mode.set_rows(1);
    assert_eq!(mode.rows(), 2);

    mode.set_rows(10);
    assert_eq!(mode.rows(), 5);
  }
}
