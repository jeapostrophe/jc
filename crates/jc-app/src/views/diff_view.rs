use std::path::{Path, PathBuf};

use git2::DiffFormat;
use gpui::*;
use gpui_component::{
  highlighter::{Diagnostic, DiagnosticSeverity},
  input::{Input, InputState, Position},
};
use similar::{ChangeTag, TextDiff};

pub struct DiffView {
  editor: Entity<InputState>,
  project_path: PathBuf,
}

struct DiffLine {
  origin: char,
  content: String,
}

/// A highlight region: (display_line, col_start, col_end, is_addition).
type Highlight = (usize, usize, usize, bool);

impl DiffView {
  pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let editor = cx.new(|cx| {
      InputState::new(window, cx).code_editor("diff").soft_wrap(false).line_number(false)
    });
    let mut view = Self { editor, project_path };
    view.refresh(window, cx);
    view
  }

  pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let (diff_text, highlights) = generate_diff(&self.project_path);

    self.editor.update(cx, |state, cx| {
      state.set_value(diff_text, window, cx);
    });

    // Apply diagnostics for word-level highlighting after set_value
    // (set_value resets diagnostics internally).
    self.editor.update(cx, |editor, _cx| {
      if let Some(diagnostics) = editor.diagnostics_mut() {
        diagnostics.clear();
        for &(line, col_start, col_end, is_addition) in &highlights {
          let severity =
            if is_addition { DiagnosticSeverity::Info } else { DiagnosticSeverity::Warning };
          diagnostics.push(
            Diagnostic::new(
              Position::new(line as u32, col_start as u32)
                ..Position::new(line as u32, col_end as u32),
              "",
            )
            .with_severity(severity),
          );
        }
      }
    });
  }
}

impl Focusable for DiffView {
  fn focus_handle(&self, cx: &App) -> FocusHandle {
    self.editor.read(cx).focus_handle(cx)
  }
}

impl Render for DiffView {
  fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
    div()
      .size_full()
      .font_family("Menlo")
      .child(Input::new(&self.editor).h_full().appearance(false).bordered(false).disabled(true))
  }
}

fn generate_diff(path: &Path) -> (String, Vec<Highlight>) {
  match generate_diff_inner(path) {
    Ok(result) => result,
    Err(e) => (format!("Error generating diff: {e}"), Vec::new()),
  }
}

fn generate_diff_inner(path: &Path) -> Result<(String, Vec<Highlight>), git2::Error> {
  let repo = git2::Repository::open(path)?;
  let head = repo.head()?;
  let tree = head.peel_to_tree()?;
  let diff = repo.diff_tree_to_workdir_with_index(Some(&tree), None)?;

  // Collect structured diff lines.
  let mut lines: Vec<DiffLine> = Vec::new();
  diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
    let origin = match line.origin() {
      '+' | '-' | ' ' => line.origin(),
      'H' | 'F' => 'H',
      _ => 'H',
    };
    let content = std::str::from_utf8(line.content()).unwrap_or("").to_string();
    lines.push(DiffLine { origin, content });
    true
  })?;

  // Build display string and compute highlights.
  let mut output = String::default();
  for line in &lines {
    match line.origin {
      '+' | '-' | ' ' => {
        output.push(line.origin);
        output.push_str(&line.content);
      }
      _ => {
        // Header lines: include content as-is
        output.push_str(&line.content);
      }
    }
  }

  let highlights = compute_highlights(&lines);

  Ok((output, highlights))
}

fn compute_highlights(lines: &[DiffLine]) -> Vec<Highlight> {
  let mut highlights = Vec::new();
  let mut display_line = 0usize;
  let mut i = 0;

  while i < lines.len() {
    match lines[i].origin {
      '-' => {
        // Collect consecutive '-' lines.
        let del_start = i;
        while i < lines.len() && lines[i].origin == '-' {
          i += 1;
        }
        let del_end = i;

        // Collect consecutive '+' lines that follow.
        let add_start = i;
        while i < lines.len() && lines[i].origin == '+' {
          i += 1;
        }
        let add_end = i;

        if add_start < add_end {
          // Paired group: compute word-level diff.
          let old_text: String =
            lines[del_start..del_end].iter().map(|l| l.content.as_str()).collect();
          let new_text: String =
            lines[add_start..add_end].iter().map(|l| l.content.as_str()).collect();

          let word_diff = TextDiff::from_words(&old_text, &new_text);

          // Map deletions back to display lines for the '-' lines.
          let del_display_start = display_line;
          map_changes_to_display(
            &word_diff,
            ChangeTag::Delete,
            &lines[del_start..del_end],
            del_display_start,
            false,
            &mut highlights,
          );

          // Map insertions back to display lines for the '+' lines.
          let add_display_start = display_line + (del_end - del_start);
          map_changes_to_display(
            &word_diff,
            ChangeTag::Insert,
            &lines[add_start..add_end],
            add_display_start,
            true,
            &mut highlights,
          );
        }

        display_line += (del_end - del_start) + (add_end - add_start);
      }
      '+' => {
        // Unpaired addition.
        display_line += 1;
        i += 1;
      }
      _ => {
        display_line += 1;
        i += 1;
      }
    }
  }

  highlights
}

/// Map word-diff changes of a specific tag back to display line positions.
///
/// `target_tag` is the change tag we're interested in (Delete for '-' lines,
/// Insert for '+' lines). We walk through all changes, tracking position in
/// the old text (for Delete/Equal) or new text (for Insert/Equal).
fn map_changes_to_display<'a>(
  word_diff: &TextDiff<'a, 'a, 'a, str>,
  target_tag: ChangeTag,
  display_lines: &[DiffLine],
  display_line_start: usize,
  is_addition: bool,
  highlights: &mut Vec<Highlight>,
) {
  // Build a flat character offset tracking for the target side.
  // We track which display line and column each character maps to.
  let mut current_line_idx = 0usize;
  // Column starts at 1 because each display line has a prefix char (+/-).
  let mut current_col = 1usize;
  let mut char_offset = 0usize;

  // The opposite tag is the one that doesn't exist in our target side.
  let skip_tag = match target_tag {
    ChangeTag::Delete => ChangeTag::Insert,
    ChangeTag::Insert => ChangeTag::Delete,
    _ => return,
  };

  for change in word_diff.iter_all_changes() {
    let tag = change.tag();
    if tag == skip_tag {
      continue;
    }

    let value = change.value();
    if tag == target_tag {
      // Changed word: record position and highlight.
      let start_line = current_line_idx;
      let start_col = current_col;
      advance_position(value, &mut current_line_idx, &mut current_col, &mut char_offset);
      record_highlights(
        start_line,
        start_col,
        current_line_idx,
        current_col,
        display_line_start,
        is_addition,
        highlights,
        display_lines,
      );
    } else {
      // Equal text: advance position, no highlight.
      advance_position(value, &mut current_line_idx, &mut current_col, &mut char_offset);
    }
  }
}

/// Advance position tracking through text content.
fn advance_position(text: &str, line_idx: &mut usize, col: &mut usize, char_offset: &mut usize) {
  for ch in text.chars() {
    if ch == '\n' {
      *line_idx += 1;
      *col = 1;
      *char_offset = 0;
    } else {
      *col += 1;
      *char_offset += ch.len_utf8();
    }
  }
}

/// Record highlight entries, potentially spanning multiple display lines.
#[allow(clippy::too_many_arguments)]
fn record_highlights(
  start_line: usize,
  start_col: usize,
  end_line: usize,
  end_col: usize,
  display_line_start: usize,
  is_addition: bool,
  highlights: &mut Vec<Highlight>,
  display_lines: &[DiffLine],
) {
  if start_line == end_line {
    if start_col < end_col {
      highlights.push((display_line_start + start_line, start_col, end_col, is_addition));
    }
  } else {
    // First line: from start_col to end of line.
    if start_line < display_lines.len() {
      let first_line_end = display_lines[start_line].content.len() + 1; // +1 for prefix
      if start_col < first_line_end {
        highlights.push((display_line_start + start_line, start_col, first_line_end, is_addition));
      }
    }
    // Middle lines: entire line.
    for mid in (start_line + 1)..end_line {
      if mid < display_lines.len() {
        let line_end = display_lines[mid].content.len() + 1;
        highlights.push((display_line_start + mid, 1, line_end, is_addition));
      }
    }
    // Last line: from start to end_col.
    if end_col > 1 {
      highlights.push((display_line_start + end_line, 1, end_col, is_addition));
    }
  }
}
