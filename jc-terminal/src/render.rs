use crate::colors::Palette;
use crate::terminal::EventProxy;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::CursorShape;
use gpui::{
  App, Bounds, FontStyle, FontWeight, Pixels, SharedString, Window, fill, font, point, px, size,
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
  pub line_height: f32,
  pub focused: bool,
  pub cursor_visible: bool,
}

/// Paint the terminal grid onto a GPUI canvas.
pub fn paint_terminal(
  term: &Term<EventProxy>,
  bounds: Bounds<Pixels>,
  layout: CellLayout,
  state: &TerminalRenderState<'_>,
  window: &mut Window,
  cx: &mut App,
) {
  let grid = term.grid();
  let num_lines = grid.screen_lines();
  let num_cols = grid.columns();
  let cursor = term.grid().cursor.point;
  let cursor_shape = term.cursor_style().shape;
  let show_cursor = term.mode().contains(alacritty_terminal::term::TermMode::SHOW_CURSOR);

  let palette = state.palette;
  let font_size = state.font_size;
  let line_height = state.line_height;

  let origin = bounds.origin;
  let line_height_px = font_size * line_height;

  // Pass 1: Paint cell backgrounds
  for line_idx in 0..num_lines {
    let line = Line(line_idx as i32);
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

  // Pass 2: Paint text
  for line_idx in 0..num_lines {
    let line = Line(line_idx as i32);
    for col_idx in 0..num_cols {
      let col = Column(col_idx);
      let cell = &grid[Point::new(line, col)];

      if cell.c == ' ' || cell.c == '\0' || cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
        continue;
      }

      let fg = if cell.flags.contains(Flags::INVERSE) {
        palette.resolve_bg(&cell.bg)
      } else {
        palette.resolve_fg(&cell.fg)
      };
      let weight =
        if cell.flags.contains(Flags::BOLD) { FontWeight::BOLD } else { FontWeight::NORMAL };
      let style =
        if cell.flags.contains(Flags::ITALIC) { FontStyle::Italic } else { FontStyle::Normal };

      let mut f = font(state.font_family.clone());
      f.weight = weight;
      f.style = style;

      let s: String = cell.c.to_string();
      let len = s.len();
      let shared: SharedString = s.into();

      // Shape and paint in one go to avoid borrow conflicts
      let shaped = window.text_system().shape_line(
        shared,
        font_size,
        &[gpui::TextRun {
          len,
          font: f,
          color: fg,
          background_color: None,
          underline: None,
          strikethrough: None,
        }],
        None,
      );

      let x = origin.x + layout.width * col_idx as f32;
      let y = origin.y + layout.height * line_idx as f32;
      let _ = shaped.paint(point(x, y), line_height_px, window, cx);
    }
  }

  // Pass 3: Paint cursor
  if show_cursor {
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
        CursorShape::Block => {
          let mut color = cursor_color;
          color.a = 0.5;
          window.paint_quad(fill(cursor_bounds, color));
        }
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
