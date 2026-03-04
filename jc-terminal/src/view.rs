use crate::colors::Palette;
use crate::input::keystroke_to_bytes;
use crate::pty::PtyHandle;
use crate::render::{CellLayout, TerminalRenderState, measure_cell, paint_terminal};
use crate::terminal::{TerminalEvent, TerminalState};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::term::TermMode;
use gpui::{
  App, AsyncApp, Bounds, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable,
  InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent,
  MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, SharedString, Styled, Subscription,
  Timer, WeakEntity, Window, actions, canvas, div, px,
};
use parking_lot::Mutex;
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);
const FONT_SIZE_STEP: Pixels = px(2.0);
const FONT_SIZE_MIN: Pixels = px(8.0);
const FONT_SIZE_MAX: Pixels = px(72.0);

actions!(terminal, [IncreaseFontSize, DecreaseFontSize, ResetFontSize, Copy, Paste]);

/// Register terminal keybindings. Call once during app initialization.
pub fn init(cx: &mut App) {
  cx.bind_keys([
    KeyBinding::new("cmd-=", IncreaseFontSize, Some("Terminal")),
    KeyBinding::new("cmd-+", IncreaseFontSize, Some("Terminal")),
    KeyBinding::new("cmd--", DecreaseFontSize, Some("Terminal")),
    KeyBinding::new("cmd-0", ResetFontSize, Some("Terminal")),
    KeyBinding::new("cmd-c", Copy, Some("Terminal")),
    KeyBinding::new("cmd-v", Paste, Some("Terminal")),
  ]);
}

/// Configuration for a terminal view.
pub struct TerminalConfig {
  pub font_family: SharedString,
  pub font_size: Pixels,
  pub line_height: f32,
  pub initial_cols: u16,
  pub initial_rows: u16,
  pub palette: Option<Palette>,
  /// Optional command to run instead of the default shell.
  /// When set, the terminal spawns this command (e.g. `"claude"`)
  /// rather than the user's login shell.
  pub command: Option<String>,
}

impl Default for TerminalConfig {
  fn default() -> Self {
    Self {
      font_family: "Lilex".into(),
      font_size: px(14.0),
      line_height: 1.3,
      initial_cols: 80,
      initial_rows: 24,
      palette: None,
      command: None,
    }
  }
}

/// Convert a mouse pixel position to an alacritty grid point and cell side.
fn pixel_to_grid(
  pos: gpui::Point<Pixels>,
  origin: gpui::Point<Pixels>,
  layout: CellLayout,
  cols: u16,
  rows: u16,
) -> (Point, Side) {
  let rel_x = (pos.x - origin.x).max(px(0.0));
  let rel_y = (pos.y - origin.y).max(px(0.0));

  let col = (rel_x / layout.width).floor().min(cols.saturating_sub(1) as f32) as usize;
  let row = (rel_y / layout.height).floor().min(rows.saturating_sub(1) as f32) as usize;

  // Which side of the cell midpoint the cursor is on.
  let cell_x = rel_x % layout.width;
  let side = if cell_x > layout.width / 2.0 { Side::Right } else { Side::Left };

  (Point::new(Line(row as i32), Column(col)), side)
}

/// Events emitted by [`TerminalView`] for the host application.
#[derive(Debug, Clone)]
pub enum TerminalViewEvent {
  Bell,
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

/// GPUI view that embeds a terminal emulator.
pub struct TerminalView {
  state: TerminalState,
  pty: Arc<PtyHandle>,
  palette: Palette,
  config: TerminalConfig,
  default_font_size: Pixels,
  focus: FocusHandle,
  last_size: Arc<Mutex<(u16, u16)>>,
  focused: bool,
  cursor_visible: bool,
  cursor_reset_at: Instant,
  cached_layout: Option<CellLayout>,
  /// Canvas origin stored during paint so mouse handlers can convert pixels to grid coords.
  canvas_origin: Arc<Mutex<gpui::Point<Pixels>>>,
  _subscriptions: Vec<Subscription>,
}

impl TerminalView {
  pub fn new(
    mut config: TerminalConfig,
    working_dir: Option<&std::path::Path>,
    _window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let cols = config.initial_cols;
    let rows = config.initial_rows;

    let (bytes_tx, bytes_rx) = flume::unbounded::<Vec<u8>>();
    let (event_tx, event_rx) = flume::unbounded();

    let state = TerminalState::new(cols as usize, rows as usize, event_tx);

    let (pty, reader) = if let Some(ref cmd) = config.command {
      PtyHandle::spawn_command(cmd, cols, rows, working_dir).expect("failed to spawn command")
    } else {
      PtyHandle::spawn_shell(cols, rows, working_dir).expect("failed to spawn shell")
    };
    let pty = Arc::new(pty);

    // Background thread: blocking PTY reads -> channel
    std::thread::spawn({
      let tx = bytes_tx;
      let mut reader = reader;
      move || {
        let mut buf = [0u8; 4096];
        loop {
          match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
              if tx.send(buf[..n].to_vec()).is_err() {
                break;
              }
            }
          }
        }
      }
    });

    // Async task: receive bytes -> process -> notify GPUI
    let term_handle = state.term_handle();
    let pty_for_write = pty.clone();
    cx.spawn(async move |this: WeakEntity<TerminalView>, cx: &mut AsyncApp| {
      // Persistent processor retains state for escape sequences spanning reads.
      let mut processor = alacritty_terminal::vte::ansi::Processor::<
        alacritty_terminal::vte::ansi::StdSyncHandler,
      >::default();

      while let Ok(bytes) = bytes_rx.recv_async().await {
        let mut all_bytes = bytes;
        while let Ok(more) = bytes_rx.try_recv() {
          all_bytes.extend(more);
        }
        {
          let mut term = term_handle.lock();
          processor.advance(&mut *term, &all_bytes);
        }
        // Handle terminal events (PtyWrite for DSR responses, etc.)
        while let Ok(event) = event_rx.try_recv() {
          match event {
            TerminalEvent::PtyWrite(s) => {
              let _ = pty_for_write.write_all(s.as_bytes());
            }
            TerminalEvent::Bell => {
              let _ = cx.update(|cx: &mut App| {
                if let Some(entity) = this.upgrade() {
                  entity.update(cx, |_view, cx| cx.emit(TerminalViewEvent::Bell));
                }
              });
            }
            TerminalEvent::CursorBlinkingChange => {}
            _ => {}
          }
        }
        let _ = cx.update(|cx: &mut App| {
          if let Some(entity) = this.upgrade() {
            cx.notify(entity.entity_id());
          }
        });
      }
    })
    .detach();

    // Cursor blink timer — only toggles when focused and terminal says blinking is enabled.
    cx.spawn(async move |this: WeakEntity<TerminalView>, cx: &mut AsyncApp| {
      loop {
        Timer::after(CURSOR_BLINK_INTERVAL).await;
        let Ok(should_continue) = cx.update(|cx: &mut App| {
          if let Some(entity) = this.upgrade() {
            entity.update(cx, |view, cx| {
              if view.focused {
                if view.cursor_reset_at.elapsed() < CURSOR_BLINK_INTERVAL {
                  return;
                }
                view.cursor_visible = !view.cursor_visible;
                cx.notify();
              }
            });
            true
          } else {
            false
          }
        }) else {
          break;
        };
        if !should_continue {
          break;
        }
      }
    })
    .detach();

    let palette = config.palette.take().unwrap_or_default();
    let default_font_size = config.font_size;
    let focus = cx.focus_handle();

    let _subscriptions = vec![
      cx.on_focus(&focus, _window, Self::on_focus),
      cx.on_blur(&focus, _window, Self::on_blur),
    ];

    Self {
      state,
      pty,
      palette,
      config,
      default_font_size,
      focus,
      last_size: Arc::new(Mutex::new((cols, rows))),
      focused: false,
      cursor_visible: true,
      cursor_reset_at: Instant::now(),
      cached_layout: None,
      canvas_origin: Arc::new(Mutex::new(gpui::Point::default())),
      _subscriptions,
    }
  }

  /// Update the terminal color palette at runtime.
  pub fn set_palette(&mut self, palette: Palette) {
    self.palette = palette;
  }

  /// Write raw bytes to the terminal's PTY.
  pub fn write_bytes_to_pty(&self, bytes: &[u8]) {
    let _ = self.pty.write_all(bytes);
  }

  /// Returns true if the terminal has bracketed-paste mode enabled.
  pub fn bracketed_paste_mode(&self) -> bool {
    self.state.with_term(|t| t.mode().contains(TermMode::BRACKETED_PASTE))
  }

  /// Get a clone of the PTY handle for use in background threads.
  pub fn pty_handle(&self) -> Arc<PtyHandle> {
    self.pty.clone()
  }

  /// Get the selected text from the terminal, if any.
  pub fn selected_text(&self) -> Option<String> {
    self.state.with_term(|term| term.selection_to_string())
  }

  fn grid_point_and_side(&self, pos: gpui::Point<Pixels>, layout: CellLayout) -> (Point, Side) {
    let origin = *self.canvas_origin.lock();
    let (cols, rows) = *self.last_size.lock();
    pixel_to_grid(pos, origin, layout, cols, rows)
  }

  fn on_focus(&mut self, _: &mut Window, cx: &mut Context<Self>) {
    self.focused = true;
    {
      let handle = self.state.term_handle();
      let mut term = handle.lock();
      term.is_focused = true;
      if term.mode().contains(TermMode::FOCUS_IN_OUT) {
        let _ = self.pty.write_all(b"\x1b[I");
      }
    }
    self.reset_cursor_blink();
    cx.notify();
  }

  fn on_blur(&mut self, _: &mut Window, cx: &mut Context<Self>) {
    self.focused = false;
    {
      let handle = self.state.term_handle();
      let mut term = handle.lock();
      term.is_focused = false;
      if term.mode().contains(TermMode::FOCUS_IN_OUT) {
        let _ = self.pty.write_all(b"\x1b[O");
      }
    }
    cx.notify();
  }

  fn reset_cursor_blink(&mut self) {
    self.cursor_visible = true;
    self.cursor_reset_at = Instant::now();
  }

  fn increase_font_size(
    &mut self,
    _: &IncreaseFontSize,
    _window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let new_size = self.config.font_size + FONT_SIZE_STEP;
    self.config.font_size = new_size.min(FONT_SIZE_MAX);
    self.cached_layout = None;
    cx.notify();
  }

  fn decrease_font_size(
    &mut self,
    _: &DecreaseFontSize,
    _window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let new_size = self.config.font_size - FONT_SIZE_STEP;
    self.config.font_size = new_size.max(FONT_SIZE_MIN);
    self.cached_layout = None;
    cx.notify();
  }

  fn reset_font_size(&mut self, _: &ResetFontSize, _window: &mut Window, cx: &mut Context<Self>) {
    self.config.font_size = self.default_font_size;
    self.cached_layout = None;
    cx.notify();
  }

  fn copy(&mut self, _: &Copy, _window: &mut Window, cx: &mut Context<Self>) {
    if let Some(text) = self.selected_text() {
      cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
  }

  fn paste(&mut self, _: &Paste, _window: &mut Window, cx: &mut Context<Self>) {
    if let Some(item) = cx.read_from_clipboard()
      && let Some(text) = item.text()
    {
      let bracketed = self.bracketed_paste_mode();
      let pty = self.pty.clone();
      std::thread::spawn(move || {
        if bracketed {
          let _ = pty.write_all(b"\x1b[200~");
          // Strip ESC chars from pasted text in bracketed mode (security).
          let sanitized = text.replace('\x1b', "");
          let _ = pty.write_all(sanitized.as_bytes());
          let _ = pty.write_all(b"\x1b[201~");
        } else {
          // Normalize newlines: terminals expect \r.
          let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
          let _ = pty.write_all(normalized.as_bytes());
        }
      });
    }
  }

  fn mouse_down(&mut self, position: gpui::Point<Pixels>, click_count: usize, layout: CellLayout) {
    let (point, side) = self.grid_point_and_side(position, layout);
    let selection_type = match click_count {
      1 => SelectionType::Simple,
      2 => SelectionType::Semantic,
      3 => SelectionType::Lines,
      _ => SelectionType::Lines,
    };
    let selection = Selection::new(selection_type, point, side);
    let handle = self.state.term_handle();
    let mut term = handle.lock();
    term.selection = Some(selection);
  }

  fn mouse_drag(&mut self, position: gpui::Point<Pixels>, layout: CellLayout) {
    let (point, side) = self.grid_point_and_side(position, layout);
    let handle = self.state.term_handle();
    let mut term = handle.lock();
    if let Some(ref mut selection) = term.selection {
      selection.update(point, side);
    }
  }

  fn mouse_up(&mut self, position: gpui::Point<Pixels>, click_count: usize, layout: CellLayout) {
    let (point, side) = self.grid_point_and_side(position, layout);
    let handle = self.state.term_handle();
    let mut term = handle.lock();
    if let Some(ref mut selection) = term.selection {
      selection.update(point, side);
    }
    // Single-click with no drag (start == end): clear selection.
    if click_count == 1
      && let Some(ref sel) = term.selection
      && sel.ty == SelectionType::Simple
    {
      // Check if selection resolves to empty.
      drop(term);
      let text = self.state.with_term(|t| t.selection_to_string());
      if text.is_none() || text.as_deref() == Some("") {
        let handle = self.state.term_handle();
        let mut term = handle.lock();
        term.selection = None;
      }
    }
  }
}

impl Focusable for TerminalView {
  fn focus_handle(&self, _: &App) -> FocusHandle {
    self.focus.clone()
  }
}

impl Render for TerminalView {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let font_family = self.config.font_family.clone();
    let font_size = self.config.font_size;
    let line_height = self.config.line_height;
    let palette_fg = self.palette.foreground;
    let palette_bg = self.palette.background;
    let focused = self.focused;
    let cursor_visible = self.cursor_visible;

    // Snapshot the current selection range for rendering.
    let selection_range: Option<SelectionRange> =
      self.state.with_term(|term| term.renderable_content().selection);

    // Cache cell layout — only re-measure when font config changes.
    let layout = *self
      .cached_layout
      .get_or_insert_with(|| measure_cell(&font_family, font_size, line_height, window));

    div()
      .id("terminal")
      .key_context("Terminal")
      .track_focus(&self.focus)
      .size_full()
      .bg(palette_bg)
      .text_color(palette_fg)
      .on_action(cx.listener(Self::increase_font_size))
      .on_action(cx.listener(Self::decrease_font_size))
      .on_action(cx.listener(Self::reset_font_size))
      .on_action(cx.listener(Self::copy))
      .on_action(cx.listener(Self::paste))
      .on_mouse_down(
        MouseButton::Left,
        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
          this.mouse_down(event.position, event.click_count, layout);
          cx.notify();
        }),
      )
      .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
        if event.dragging() {
          this.mouse_drag(event.position, layout);
          cx.notify();
        }
      }))
      .on_mouse_up(
        MouseButton::Left,
        cx.listener(move |this, event: &MouseUpEvent, _window, cx| {
          this.mouse_up(event.position, event.click_count, layout);
          cx.notify();
        }),
      )
      .on_key_down(cx.listener({
        let pty = self.pty.clone();
        move |this, event: &KeyDownEvent, _window, _cx| {
          this.reset_cursor_blink();
          let mode = this.state.with_term(|t| *t.mode());
          if let Some(bytes) = keystroke_to_bytes(&event.keystroke, mode) {
            // Clear selection on any key press that generates terminal input.
            {
              let handle = this.state.term_handle();
              let mut term = handle.lock();
              term.selection = None;
            }
            let _ = pty.write_all(&bytes);
          }
        }
      }))
      .child(canvas(
        {
          let font_family = font_family.clone();
          let canvas_origin = self.canvas_origin.clone();
          move |bounds: Bounds<Pixels>, _window: &mut Window, _cx: &mut App| {
            *canvas_origin.lock() = bounds.origin;
            (bounds, layout, font_family)
          }
        },
        {
          let term_handle = self.state.term_handle();
          let palette = self.palette.clone();
          let pty_for_resize = self.pty.clone();
          let last_size = self.last_size.clone();

          move |_bounds: Bounds<Pixels>,
                (prep_bounds, layout, font_family): (Bounds<Pixels>, CellLayout, SharedString),
                window: &mut Window,
                cx: &mut App| {
            let mut term = term_handle.lock();

            // Detect and apply resize
            let new_cols = (prep_bounds.size.width / layout.width).floor() as u16;
            let new_rows = (prep_bounds.size.height / layout.height).floor() as u16;
            let mut last = last_size.lock();
            if new_cols > 0 && new_rows > 0 && (new_cols != last.0 || new_rows != last.1) {
              *last = (new_cols, new_rows);
              let _ = pty_for_resize.resize(new_cols, new_rows);
              term.resize(crate::terminal::TermDimensions {
                cols: new_cols as usize,
                rows: new_rows as usize,
              });
            }
            drop(last);

            let render_state = TerminalRenderState {
              palette: &palette,
              font_family: &font_family,
              font_size,
              line_height,
              focused,
              cursor_visible,
              selection: selection_range,
            };
            paint_terminal(&term, prep_bounds, layout, &render_state, window, cx);
          }
        },
      ))
  }
}
