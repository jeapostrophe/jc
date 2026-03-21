use crate::colors::Palette;
use crate::terminal::EventProxy;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::CursorShape;
use gpui::{
  App, Bounds, FontStyle, FontWeight, Hsla, Pixels, SharedString, Window, fill, font, point, px,
  size,
};

/// Measured cell dimensions for the current font.
#[derive(Clone, Copy, Debug)]
pub struct CellLayout {
  pub width: Pixels,
  pub height: Pixels,
}

/// Measure the size of a single terminal cell using the given font configuration.
pub fn measure_cell(
  font_family: &SharedString,
  font_size: Pixels,
  line_height: f32,
  window: &mut Window,
) -> CellLayout {
  let f = font(font_family.clone());

  let text_system = window.text_system();
  let shaped = text_system.shape_line(
    "M".into(),
    font_size,
    &[gpui::TextRun {
      len: 1,
      font: f,
      color: gpui::black(),
      background_color: None,
      underline: None,
      strikethrough: None,
    }],
    None,
  );

  let width = shaped.width;
  let height = font_size * line_height;

  CellLayout { width, height }
}

/// Rendering parameters for a terminal paint pass.
pub struct TerminalRenderState<'a> {
  pub palette: &'a Palette,
  pub font_family: &'a SharedString,
  pub font_size: Pixels,
  pub focused: bool,
  pub cursor_visible: bool,
  pub selection: Option<SelectionRange>,
}

/// Paint the terminal grid onto a GPUI canvas.
pub fn paint_terminal(
  term: &Term<EventProxy>,
  bounds: Bounds<Pixels>,
  layout: CellLayout,
  state: &TerminalRenderState<'_>,
  content_changed: bool,
  selection_changed: bool,
  window: &mut Window,
  cx: &mut App,
) {
  let grid = term.grid();
  let num_lines = grid.screen_lines();
  let num_cols = grid.columns();
  let display_offset = grid.display_offset() as i32;
  let cursor = grid.cursor.point;
  let cursor_shape = term.cursor_style().shape;
  let show_cursor = term.mode().contains(alacritty_terminal::term::TermMode::SHOW_CURSOR);

  let palette = state.palette;
  let font_size = state.font_size;

  let origin = bounds.origin;

  let selection_color = Hsla { h: 210.0 / 360.0, s: 0.6, l: 0.5, a: 0.35 };

  // Pass 1: Paint cell backgrounds (skip when content unchanged)
  if content_changed {
    for line_idx in 0..num_lines {
      let line = Line(line_idx as i32 - display_offset);
      for col_idx in 0..num_cols {
        let col = Column(col_idx);
        let cell = &grid[Point::new(line, col)];

        let bg = if cell.flags.contains(Flags::INVERSE) {
          palette.resolve_fg(&cell.fg)
        } else {
          palette.resolve_bg(&cell.bg)
        };
        if bg != palette.background {
          let x = origin.x + layout.width * col_idx as f32;
          let y = origin.y + layout.height * line_idx as f32;
          window.paint_quad(fill(Bounds::new(point(x, y), size(layout.width, layout.height)), bg));
        }
      }
    }
  }

  // Pass 1.5: Paint selection highlight (skip when neither content nor selection changed)
  if content_changed || selection_changed {
    if let Some(ref sel) = state.selection {
      for line_idx in 0..num_lines {
        let line = Line(line_idx as i32 - display_offset);
        for col_idx in 0..num_cols {
          let pt = Point::new(line, Column(col_idx));
          if sel.contains(pt) {
            let x = origin.x + layout.width * col_idx as f32;
            let y = origin.y + layout.height * line_idx as f32;
            window.paint_quad(fill(
              Bounds::new(point(x, y), size(layout.width, layout.height)),
              selection_color,
            ));
          }
        }
      }
    }
  }

  // Pass 2: Paint text — one shape_line() call per row (skip when content unchanged)
  if content_changed {
    for line_idx in 0..num_lines {
      let line = Line(line_idx as i32 - display_offset);

      let mut row_string = String::with_capacity(num_cols * 4);
      let mut runs: Vec<gpui::TextRun> = Vec::new();
      let mut current_run_len: usize = 0;
      let mut current_fg = Hsla::default();
      let mut current_weight = FontWeight::NORMAL;
      let mut current_style = FontStyle::Normal;
      let mut has_non_whitespace = false;
      let mut first_cell = true;

      for col_idx in 0..num_cols {
        let col = Column(col_idx);
        let cell = &grid[Point::new(line, col)];

        if cell.c == '\0' || cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
          // Null/spacer cells: append a space to keep column alignment
          let byte_len = ' '.len_utf8();
          if first_cell {
            // Initialize style from this cell (won't matter much since it's whitespace)
            current_fg = palette.resolve_fg(&cell.fg);
            current_weight = FontWeight::NORMAL;
            current_style = FontStyle::Normal;
            first_cell = false;
          }
          row_string.push(' ');
          current_run_len += byte_len;
          continue;
        }

        // Compute style for this cell
        let fg = if cell.flags.contains(Flags::INVERSE) {
          palette.resolve_bg(&cell.bg)
        } else {
          palette.resolve_fg(&cell.fg)
        };
        let weight =
          if cell.flags.contains(Flags::BOLD) { FontWeight::BOLD } else { FontWeight::NORMAL };
        let style =
          if cell.flags.contains(Flags::ITALIC) { FontStyle::Italic } else { FontStyle::Normal };

        if cell.c != ' ' {
          has_non_whitespace = true;
        }

        if first_cell {
          current_fg = fg;
          current_weight = weight;
          current_style = style;
          first_cell = false;
        } else if fg != current_fg || weight != current_weight || style != current_style {
          // Style changed — push the current run and start a new one
          if current_run_len > 0 {
            let mut f = font(state.font_family.clone());
            f.weight = current_weight;
            f.style = current_style;
            runs.push(gpui::TextRun {
              len: current_run_len,
              font: f,
              color: current_fg,
              background_color: None,
              underline: None,
              strikethrough: None,
            });
          }
          current_run_len = 0;
          current_fg = fg;
          current_weight = weight;
          current_style = style;
        }

        let byte_len = cell.c.len_utf8();
        row_string.push(cell.c);
        current_run_len += byte_len;
      }

      // Push the final run
      if current_run_len > 0 {
        let mut f = font(state.font_family.clone());
        f.weight = current_weight;
        f.style = current_style;
        runs.push(gpui::TextRun {
          len: current_run_len,
          font: f,
          color: current_fg,
          background_color: None,
          underline: None,
          strikethrough: None,
        });
      }

      // Skip rows that are entirely whitespace
      if !has_non_whitespace || runs.is_empty() {
        continue;
      }

      let shared: SharedString = row_string.into();
      let shaped = window.text_system().shape_line(shared, font_size, &runs, None);

      let x = origin.x;
      let y = origin.y + layout.height * line_idx as f32;
      let _ = shaped.paint(point(x, y), layout.height, window, cx);
    }
  }

  // Pass 3: Paint cursor (only when not scrolled into history)
  if show_cursor && display_offset == 0 {
    let cursor_color = palette.cursor;
    let x = origin.x + layout.width * cursor.column.0 as f32;
    let y = origin.y + layout.height * cursor.line.0 as f32;
    let cursor_bounds = Bounds::new(point(x, y), size(layout.width, layout.height));

    if !state.focused {
      // Unfocused: hollow rectangle outline
      let border = px(1.0);
      // Top edge
      window.paint_quad(fill(Bounds::new(point(x, y), size(layout.width, border)), cursor_color));
      // Bottom edge
      window.paint_quad(fill(
        Bounds::new(point(x, y + layout.height - border), size(layout.width, border)),
        cursor_color,
      ));
      // Left edge
      window.paint_quad(fill(Bounds::new(point(x, y), size(border, layout.height)), cursor_color));
      // Right edge
      window.paint_quad(fill(
        Bounds::new(point(x + layout.width - border, y), size(border, layout.height)),
        cursor_color,
      ));
    } else if state.cursor_visible {
      // Focused + visible blink phase: solid cursor
      match cursor_shape {
        CursorShape::Beam => {
          window
            .paint_quad(fill(Bounds::new(point(x, y), size(px(2.0), layout.height)), cursor_color));
        }
        CursorShape::Underline => {
          let underline_y = y + layout.height - px(2.0);
          window.paint_quad(fill(
            Bounds::new(point(x, underline_y), size(layout.width, px(2.0))),
            cursor_color,
          ));
        }
        _ => {
          let mut color = cursor_color;
          color.a = 0.5;
          window.paint_quad(fill(cursor_bounds, color));
        }
      }
    }
  }
}
