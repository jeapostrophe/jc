use crate::colors::Palette;
use crate::input::keystroke_to_bytes;
use crate::pty::PtyHandle;
use crate::render::{CellLayout, TerminalRenderState, measure_cell, paint_terminal};
use crate::terminal::{TerminalEvent, TerminalState};
use alacritty_terminal::term::TermMode;
use gpui::{
  App, AsyncApp, Bounds, Context, FocusHandle, Focusable, InteractiveElement, IntoElement,
  KeyBinding, KeyDownEvent, ParentElement, Pixels, Render, SharedString, Styled, Subscription,
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

actions!(terminal, [IncreaseFontSize, DecreaseFontSize, ResetFontSize]);

/// Register terminal keybindings. Call once during app initialization.
pub fn init(cx: &mut App) {
  cx.bind_keys([
    KeyBinding::new("cmd-=", IncreaseFontSize, Some("Terminal")),
    KeyBinding::new("cmd-+", IncreaseFontSize, Some("Terminal")),
    KeyBinding::new("cmd--", DecreaseFontSize, Some("Terminal")),
    KeyBinding::new("cmd-0", ResetFontSize, Some("Terminal")),
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
      .on_key_down(cx.listener({
        let pty = self.pty.clone();
        move |this, event: &KeyDownEvent, _window, _cx| {
          this.reset_cursor_blink();
          let mode = this.state.with_term(|t| *t.mode());
          if let Some(bytes) = keystroke_to_bytes(&event.keystroke, mode) {
            let _ = pty.write_all(&bytes);
          }
        }
      }))
      .child(canvas(
        {
          let font_family = font_family.clone();
          move |bounds: Bounds<Pixels>, _window: &mut Window, _cx: &mut App| {
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
            };
            paint_terminal(&term, prep_bounds, layout, &render_state, window, cx);
          }
        },
      ))
  }
}
