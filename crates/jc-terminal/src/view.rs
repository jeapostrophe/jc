use crate::colors::Palette;
use crate::input::keystroke_to_bytes;
use crate::pty::PtyHandle;
use crate::render::{CellLayout, measure_cell, paint_terminal};
use crate::terminal::TerminalState;
use gpui::{
  App, AsyncApp, Bounds, Context, FocusHandle, Focusable, InteractiveElement, IntoElement,
  KeyDownEvent, ParentElement, Pixels, Render, SharedString, Styled, WeakEntity, Window, canvas,
  div, px,
};
use parking_lot::Mutex;
use std::io::Read;
use std::sync::Arc;

/// Configuration for a terminal view.
pub struct TerminalConfig {
  pub font_family: SharedString,
  pub font_size: Pixels,
  pub line_height: f32,
  pub initial_cols: u16,
  pub initial_rows: u16,
}

impl Default for TerminalConfig {
  fn default() -> Self {
    Self {
      font_family: "Menlo".into(),
      font_size: px(14.0),
      line_height: 1.3,
      initial_cols: 80,
      initial_rows: 24,
    }
  }
}

/// GPUI view that embeds a terminal emulator.
pub struct TerminalView {
  state: TerminalState,
  pty: Arc<PtyHandle>,
  palette: Palette,
  config: TerminalConfig,
  focus: FocusHandle,
  last_size: Arc<Mutex<(u16, u16)>>,
}

impl TerminalView {
  pub fn new(
    config: TerminalConfig,
    working_dir: Option<&std::path::Path>,
    _window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let cols = config.initial_cols;
    let rows = config.initial_rows;

    let (bytes_tx, bytes_rx) = flume::unbounded::<Vec<u8>>();
    let (event_tx, _event_rx) = flume::unbounded();

    let state = TerminalState::new(cols as usize, rows as usize, event_tx);

    let (pty, reader) =
      PtyHandle::spawn_shell(cols, rows, working_dir).expect("failed to spawn shell");
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
    cx.spawn(async move |this: WeakEntity<TerminalView>, cx: &mut AsyncApp| {
      while let Ok(bytes) = bytes_rx.recv_async().await {
        let mut all_bytes = bytes;
        while let Ok(more) = bytes_rx.try_recv() {
          all_bytes.extend(more);
        }
        {
          let mut term = term_handle.lock();
          let mut processor = alacritty_terminal::vte::ansi::Processor::<
            alacritty_terminal::vte::ansi::StdSyncHandler,
          >::default();
          processor.advance(&mut *term, &all_bytes);
        }
        let _ = cx.update(|cx: &mut App| {
          if let Some(entity) = this.upgrade() {
            cx.notify(entity.entity_id());
          }
        });
      }
    })
    .detach();

    Self {
      state,
      pty,
      palette: Palette::default(),
      config,
      focus: cx.focus_handle(),
      last_size: Arc::new(Mutex::new((cols, rows))),
    }
  }
}

impl Focusable for TerminalView {
  fn focus_handle(&self, _: &App) -> FocusHandle {
    self.focus.clone()
  }
}

impl Render for TerminalView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let font_family = self.config.font_family.clone();
    let font_size = self.config.font_size;
    let line_height = self.config.line_height;
    let palette_fg = self.palette.foreground;
    let palette_bg = self.palette.background;

    div()
      .id("terminal")
      .key_context("Terminal")
      .track_focus(&self.focus)
      .size_full()
      .bg(palette_bg)
      .text_color(palette_fg)
      .on_key_down(cx.listener({
        let pty = self.pty.clone();
        move |this, event: &KeyDownEvent, _window, _cx| {
          let mode = this.state.with_term(|t| *t.mode());
          if let Some(bytes) = keystroke_to_bytes(&event.keystroke, mode) {
            let _ = pty.write_all(&bytes);
          }
        }
      }))
      .child(canvas(
        {
          let font_family = font_family.clone();
          move |bounds: Bounds<Pixels>, window: &mut Window, _cx: &mut App| {
            let layout = measure_cell(&font_family, font_size, line_height, window);
            (bounds, layout, font_family)
          }
        },
        {
          let term_handle = self.state.term_handle();
          let palette = Palette::default();
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
            let mut size = last_size.lock();
            if new_cols > 0 && new_rows > 0 && (new_cols != size.0 || new_rows != size.1) {
              *size = (new_cols, new_rows);
              let _ = pty_for_resize.resize(new_cols, new_rows);
              term.resize(crate::terminal::TermDimensions {
                cols: new_cols as usize,
                rows: new_rows as usize,
              });
            }
            drop(size);

            paint_terminal(
              &term,
              prep_bounds,
              layout,
              &palette,
              &font_family,
              font_size,
              line_height,
              window,
              cx,
            );
          }
        },
      ))
  }
}
