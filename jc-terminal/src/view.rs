use crate::colors::Palette;
use crate::input::keystroke_to_bytes;
use crate::pty::PtyHandle;
use crate::render::{CellLayout, TerminalRenderState, measure_cell, paint_terminal};
use crate::settle::{LAUNCH_GIVE_UP, LAUNCH_QUIET, SettleStep, SettleWindow};
use crate::terminal::{TerminalEvent, TerminalState};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::term::TermMode;
use gpui::{
  App, AsyncApp, Bounds, ClipboardItem, Context, FocusHandle, Focusable, InteractiveElement,
  IntoElement, KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
  ParentElement, Pixels, Render, ScrollWheelEvent, SharedString, Styled, Subscription, Timer,
  WeakEntity, Window, actions, canvas, div, px,
};
use parking_lot::Mutex;
use std::future::Future;
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Poll;
use std::time::{Duration, Instant};

const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);
const FONT_SIZE_STEP: Pixels = px(2.0);
const FONT_SIZE_MIN: Pixels = px(8.0);
const FONT_SIZE_MAX: Pixels = px(72.0);

actions!(
  terminal,
  [
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    Copy,
    Paste,
    SendTab,
    SendShiftTab,
    SendEnter,
    SendShiftEnter
  ]
);

/// Register terminal keybindings. Call once during app initialization.
pub fn init(cx: &mut App) {
  cx.bind_keys([
    KeyBinding::new("cmd-=", IncreaseFontSize, Some("Terminal")),
    KeyBinding::new("cmd-+", IncreaseFontSize, Some("Terminal")),
    KeyBinding::new("cmd--", DecreaseFontSize, Some("Terminal")),
    KeyBinding::new("cmd-0", ResetFontSize, Some("Terminal")),
    KeyBinding::new("cmd-c", Copy, Some("Terminal")),
    KeyBinding::new("cmd-v", Paste, Some("Terminal")),
    // Intercept keys that Root or Input contexts would otherwise consume.
    KeyBinding::new("tab", SendTab, Some("Terminal")),
    KeyBinding::new("shift-tab", SendShiftTab, Some("Terminal")),
    KeyBinding::new("enter", SendEnter, Some("Terminal")),
    KeyBinding::new("shift-enter", SendShiftEnter, Some("Terminal")),
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

/// Why the notification relay woke up.
#[derive(Debug, PartialEq, Eq)]
enum Wake {
  /// A batch was parsed.
  Batch,
  /// The deadline elapsed before any batch arrived.
  Deadline,
  /// The sending half is gone.
  Closed,
}

/// Wait for the next value on `rx`, or for `deadline` to complete if one is
/// given — whichever happens first. A ready batch always wins a tie, since
/// dropping it on the floor would lose a wake.
///
/// Dropping the half-finished receive future is safe: flume's `RecvFut` only
/// unregisters its hook on drop (and hands the wakeup to another receiver if it
/// had already been signalled), so a batch racing the deadline stays queued and
/// is delivered on the next call rather than being swallowed here.
async fn recv_or_deadline<T, F: Future<Output = Instant>>(
  rx: &flume::Receiver<T>,
  deadline: Option<F>,
) -> Wake {
  let Some(deadline) = deadline else {
    return if rx.recv_async().await.is_ok() { Wake::Batch } else { Wake::Closed };
  };
  let mut recv = std::pin::pin!(rx.recv_async());
  let mut deadline = std::pin::pin!(deadline);
  std::future::poll_fn(move |cx| {
    if let Poll::Ready(result) = recv.as_mut().poll(cx) {
      return Poll::Ready(if result.is_ok() { Wake::Batch } else { Wake::Closed });
    }
    if deadline.as_mut().poll(cx).is_ready() {
      return Poll::Ready(Wake::Deadline);
    }
    Poll::Pending
  })
  .await
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
  /// Canvas origin stored during paint so mouse handlers can convert pixels to grid coords.
  canvas_origin: Arc<Mutex<gpui::Point<Pixels>>>,
  /// Shared flag: when false, the background processing thread batches more
  /// aggressively and the notification relay skips `cx.notify()`.
  visible: Arc<AtomicBool>,
  /// Batches of child output parsed so far, bumped by the VTE thread. Compared
  /// against `seen_batches` to answer "did anything happen since you last
  /// looked?" without any per-chunk main-thread work.
  output_batches: Arc<AtomicUsize>,
  /// Value of `output_batches` at the last [`TerminalView::clear_output_seen`].
  /// Main-thread only — the VTE thread never reads or writes it, so this is a
  /// plain atomic for `&self` interior mutability, not shared state.
  seen_batches: AtomicUsize,
  /// Launch-settle window (see [`TerminalView::discount_launch_output`]), or
  /// `None` when none is open — which is the case before one is opened and
  /// again once the notification relay, which both stamps batches onto it and
  /// drives it, has seen it through to its end.
  settle: Arc<Mutex<Option<SettleWindow>>>,
  _subscriptions: Vec<Subscription>,
  /// Cursor blink task — only runs while focused.
  _blink_task: Option<gpui::Task<()>>,
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
    std::thread::spawn(move || {
      let mut reader = reader;
      let mut buf = [0u8; 4096];
      loop {
        match reader.read(&mut buf) {
          Ok(0) | Err(_) => break,
          Ok(n) => {
            if bytes_tx.send(buf[..n].to_vec()).is_err() {
              break;
            }
          }
        }
      }
    });

    // Background thread: VTE parsing off the main thread.
    // The heavy `processor.advance()` work runs here; only lightweight
    // `cx.notify()` / bell emission happens on the main executor.
    let term_handle = state.term_handle();
    let pty_for_write = pty.clone();
    let visible = Arc::new(AtomicBool::new(true));
    let visible_for_bg = visible.clone();
    let (notify_tx, notify_rx) = flume::unbounded::<()>(); // one signal per parsed batch
    let output_batches = Arc::new(AtomicUsize::new(0));
    let output_batches_for_bg = output_batches.clone();
    std::thread::spawn(move || {
      let mut processor = alacritty_terminal::vte::ansi::Processor::<
        alacritty_terminal::vte::ansi::StdSyncHandler,
      >::default();

      const COALESCE_CAP: usize = 64 * 1024; // 64 KB
      const HIDDEN_COALESCE_CAP: usize = 256 * 1024; // 256 KB
      while let Ok(bytes) = bytes_rx.recv() {
        let is_visible = visible_for_bg.load(Ordering::Relaxed);
        let cap = if is_visible { COALESCE_CAP } else { HIDDEN_COALESCE_CAP };
        let mut all_bytes = bytes;
        while all_bytes.len() < cap {
          match bytes_rx.try_recv() {
            Ok(more) => all_bytes.extend(more),
            Err(_) => break,
          }
        }
        {
          let mut term = term_handle.lock();
          processor.advance(&mut *term, &all_bytes);
        }
        output_batches_for_bg.fetch_add(1, Ordering::Relaxed);
        // Handle terminal events. PtyWrite is answered here on the VTE thread;
        // the rest only matter as "something changed, repaint".
        while let Ok(event) = event_rx.try_recv() {
          if let TerminalEvent::PtyWrite(s) = event {
            let _ = pty_for_write.write_all(s.as_bytes());
          }
        }
        if notify_tx.send(()).is_err() {
          break;
        }
      }
    });

    // Lightweight main-thread relay: notify GPUI for repaint, and drive the
    // launch-settle window (see `discount_launch_output`) off the same batch
    // signal. One task, two wake sources — the batch that would reset a
    // debounce is exactly the batch this loop already waits on.
    let visible_for_relay = visible.clone();
    let settle = Arc::new(Mutex::new(None::<SettleWindow>));
    let settle_for_relay = settle.clone();
    cx.spawn(async move |this: WeakEntity<TerminalView>, cx: &mut AsyncApp| {
      loop {
        // Service the settle window first: it either wants the baseline taken
        // now, or says how long this loop may sleep before it needs asking
        // again. With no window open there is nothing to time out on.
        let step = settle_for_relay.lock().as_mut().map(|window| window.step(Instant::now()));
        let deadline = match step {
          Some(SettleStep::Wait(duration)) => Some(duration),
          Some(SettleStep::Clear) => {
            let _ = cx.update(|cx: &mut App| {
              if let Some(entity) = this.upgrade() {
                entity.read(cx).clear_output_seen();
              }
            });
            *settle_for_relay.lock() = None;
            None
          }
          Some(SettleStep::Done) => {
            *settle_for_relay.lock() = None;
            None
          }
          None => None,
        };

        match recv_or_deadline(&notify_rx, deadline.map(Timer::after)).await {
          // The child printed. Restart the quiet wait — even while hidden,
          // since a backgrounded session's child is still settling and the
          // marker it feeds is read from the picker.
          Wake::Batch => {
            if let Some(window) = settle_for_relay.lock().as_mut() {
              window.note_batch(Instant::now());
            }
            // Skip repaint for hidden terminals — no point rendering offscreen content.
            if visible_for_relay.load(Ordering::Relaxed) {
              let _ = cx.update(|cx: &mut App| {
                if let Some(entity) = this.upgrade() {
                  cx.notify(entity.entity_id());
                }
              });
            }
          }
          // The settle window's next decision point came first — go ask it.
          Wake::Deadline => {}
          // The VTE thread is gone: the child has exited.
          Wake::Closed => break,
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
      visible,
      output_batches,
      seen_batches: AtomicUsize::new(0),
      settle,
      _subscriptions,
      _blink_task: None,
    }
  }

  /// Update the terminal color palette at runtime.
  pub fn set_palette(&mut self, palette: Palette) {
    self.palette = palette;
  }

  /// Has the child written anything since the last [`Self::clear_output_seen`]?
  pub fn has_unseen_output(&self) -> bool {
    self.output_batches.load(Ordering::Relaxed) != self.seen_batches.load(Ordering::Relaxed)
  }

  /// Take everything printed so far as seen — the user is looking at it now.
  pub fn clear_output_seen(&self) {
    self.seen_batches.store(self.output_batches.load(Ordering::Relaxed), Ordering::Relaxed);
  }

  /// Discount this child's launch output, so [`Self::has_unseen_output`] means
  /// "the child printed something while you were away" rather than "jc started
  /// it". Call once, just after spawning; the window is per terminal, so a
  /// session restored mid-run gets its own.
  ///
  /// The baseline is taken once the child has printed something and then held
  /// still for [`LAUNCH_QUIET`], or unconditionally at [`LAUNCH_GIVE_UP`]. Both
  /// exits clear: a child that never settles is still printing and re-marks
  /// itself on its next batch, whereas a marker left set here would never
  /// retire (only switching away clears one).
  ///
  /// No task of its own: the notification relay started in [`Self::new`] is
  /// already woken by every parsed batch, so it drives the window it finds
  /// here.
  pub fn discount_launch_output(&self) {
    *self.settle.lock() = Some(SettleWindow::new(LAUNCH_QUIET, LAUNCH_GIVE_UP, Instant::now()));
  }

  /// End the launch-settle window WITHOUT clearing, because the user has given
  /// this session work: from here on everything the child prints answers
  /// something you asked for, so it is activity by definition. Without this a
  /// prompt sent during the window keeps restarting the quiet wait, and the
  /// baseline lands exactly when Claude finishes — wiping the marker for the
  /// work you were waiting on.
  pub fn cancel_launch_settle(&self) {
    if let Some(window) = self.settle.lock().as_mut() {
      window.cancel();
    }
  }

  /// Mark this terminal as visible or hidden.  Hidden terminals still process
  /// PTY bytes (so state is correct when switching back) but batch more
  /// aggressively and skip `cx.notify()` to reduce main-thread overhead.
  pub fn set_visible(&self, is_visible: bool) {
    self.visible.store(is_visible, Ordering::Relaxed);
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

  /// Write text to the terminal PTY, using bracketed paste if the terminal
  /// expects it. Sanitizes ESC characters and normalizes newlines.
  pub fn write_text(&self, text: &str) {
    if self.bracketed_paste_mode() {
      let mut buf = Vec::with_capacity(text.len() + 12);
      buf.extend_from_slice(b"\x1b[200~");
      let sanitized = text.replace('\x1b', "");
      buf.extend_from_slice(sanitized.as_bytes());
      buf.extend_from_slice(b"\x1b[201~");
      let _ = self.pty.write_all(&buf);
    } else {
      let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
      let _ = self.pty.write_all(normalized.as_bytes());
    }
  }

  /// Lines scrolled back from bottom (0 = at bottom).
  pub fn display_offset(&self) -> usize {
    self.state.with_term(|t| t.grid().display_offset())
  }

  /// Scroll to `offset` lines from bottom. Clamped to available history.
  pub fn set_display_offset(&self, offset: usize) {
    self.state.with_term_mut(|term| {
      // No Scroll::Absolute variant — reset then scroll up.
      term.scroll_display(Scroll::Bottom);
      if offset > 0 {
        term.scroll_display(Scroll::Delta(offset.min(i32::MAX as usize) as i32));
      }
    });
  }

  /// Scroll the terminal scrollback by the given number of lines (positive = down).
  pub fn scroll_lines(&mut self, lines: i32, cx: &mut Context<Self>) {
    // Scroll::Delta uses the opposite convention: positive scrolls *up* (toward history).
    self.state.with_term_mut(|term| {
      term.scroll_display(Scroll::Delta(-lines));
    });
    cx.notify();
  }

  /// Scroll the terminal scrollback by the given number of pages (positive = down).
  pub fn scroll_pages(&mut self, pages: i32, cx: &mut Context<Self>) {
    let rows = self.last_size.lock().1 as i32;
    self.scroll_lines(pages * rows, cx);
  }

  /// Get the selected text from the terminal, if any.
  pub fn selected_text(&self) -> Option<String> {
    self.state.with_term(|term| term.selection_to_string())
  }

  fn grid_point_and_side(&self, pos: gpui::Point<Pixels>, layout: CellLayout) -> (Point, Side) {
    let origin = *self.canvas_origin.lock();
    let (cols, rows) = *self.last_size.lock();
    let (mut point, side) = pixel_to_grid(pos, origin, layout, cols, rows);
    // Adjust for scroll position: when scrolled back into history,
    // visible row 0 is at Line(-display_offset) in grid coordinates.
    let display_offset = self.state.with_term(|t| t.grid().display_offset() as i32);
    point.line = Line(point.line.0 - display_offset);
    (point, side)
  }

  fn on_focus(&mut self, _: &mut Window, cx: &mut Context<Self>) {
    self.focused = true;
    let send_focus = self.state.with_term_mut(|term| {
      term.is_focused = true;
      term.mode().contains(TermMode::FOCUS_IN_OUT)
    });
    if send_focus {
      let _ = self.pty.write_all(b"\x1b[I");
    }
    self.reset_cursor_blink();
    self.start_blink_task(cx);
    cx.notify();
  }

  fn on_blur(&mut self, _: &mut Window, cx: &mut Context<Self>) {
    self.focused = false;
    // Stop the blink timer — no point toggling the cursor while unfocused.
    self._blink_task = None;
    let send_blur = self.state.with_term_mut(|term| {
      term.is_focused = false;
      term.mode().contains(TermMode::FOCUS_IN_OUT)
    });
    if send_blur {
      let _ = self.pty.write_all(b"\x1b[O");
    }
    cx.notify();
  }

  /// Start the cursor blink async task. Replaces any existing task.
  fn start_blink_task(&mut self, cx: &mut Context<Self>) {
    self._blink_task =
      Some(cx.spawn(async move |this: WeakEntity<TerminalView>, cx: &mut AsyncApp| {
        loop {
          Timer::after(CURSOR_BLINK_INTERVAL).await;
          let Ok(should_continue) = cx.update(|cx: &mut App| {
            if let Some(entity) = this.upgrade() {
              entity.update(cx, |view, cx| {
                if !view.focused {
                  // Focus was lost between timer fire and main-thread update — stop.
                  view._blink_task = None;
                  return false;
                }
                if view.cursor_reset_at.elapsed() >= CURSOR_BLINK_INTERVAL {
                  view.cursor_visible = !view.cursor_visible;
                  cx.notify();
                }
                true
              })
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
      }));
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
      self.write_text(&text);
    }
  }

  fn send_tab(&mut self, _: &SendTab, _window: &mut Window, _cx: &mut Context<Self>) {
    self.reset_cursor_blink();
    self.state.with_term_mut(|term| {
      term.selection = None;
      term.scroll_display(Scroll::Bottom);
    });
    let _ = self.pty.write_all(b"\t");
  }

  fn send_shift_tab(&mut self, _: &SendShiftTab, _window: &mut Window, _cx: &mut Context<Self>) {
    self.reset_cursor_blink();
    self.state.with_term_mut(|term| {
      term.selection = None;
      term.scroll_display(Scroll::Bottom);
    });
    let _ = self.pty.write_all(b"\x1b[Z");
  }

  fn send_enter(&mut self, _: &SendEnter, _window: &mut Window, _cx: &mut Context<Self>) {
    self.reset_cursor_blink();
    self.state.with_term_mut(|term| {
      term.selection = None;
      term.scroll_display(Scroll::Bottom);
    });
    let _ = self.pty.write_all(b"\r");
  }

  fn send_shift_enter(
    &mut self,
    _: &SendShiftEnter,
    _window: &mut Window,
    _cx: &mut Context<Self>,
  ) {
    self.reset_cursor_blink();
    self.state.with_term_mut(|term| {
      term.selection = None;
      term.scroll_display(Scroll::Bottom);
    });
    let _ = self.pty.write_all(b"\x1b[13;2u");
  }

  /// Send a mouse event to the PTY in SGR or legacy format.
  /// `button`: 0 = left click, 32 = motion with button held.
  /// `pressed`: true for press/motion, false for release.
  fn send_mouse_event(
    &mut self,
    button: u8,
    pressed: bool,
    position: gpui::Point<Pixels>,
    layout: CellLayout,
  ) {
    let (point, _) = self.grid_point_and_side(position, layout);
    let col = point.column.0 + 1;
    let row = point.line.0 + 1;
    let sgr = self.state.with_term(|t| t.mode().contains(TermMode::SGR_MOUSE));
    if sgr {
      let suffix = if pressed { 'M' } else { 'm' };
      let seq = format!("\x1b[<{button};{col};{row}{suffix}");
      let _ = self.pty.write_all(seq.as_bytes());
    } else {
      let cb = button + 32;
      let cx_byte = (col as u8).saturating_add(32);
      let cy_byte = (row as u8).saturating_add(32);
      let seq = [b'\x1b', b'[', b'M', cb, cx_byte, cy_byte];
      let _ = self.pty.write_all(&seq);
    }
  }

  fn mouse_down(&mut self, position: gpui::Point<Pixels>, click_count: usize, layout: CellLayout) {
    let (point, side) = self.grid_point_and_side(position, layout);
    let selection_type = match click_count {
      1 => SelectionType::Simple,
      2 => SelectionType::Semantic,
      _ => SelectionType::Lines,
    };
    let selection = Selection::new(selection_type, point, side);
    self.state.with_term_mut(|term| term.selection = Some(selection));
  }

  fn mouse_drag(&mut self, position: gpui::Point<Pixels>, layout: CellLayout) {
    let (point, side) = self.grid_point_and_side(position, layout);
    self.state.with_term_mut(|term| {
      if let Some(ref mut selection) = term.selection {
        selection.update(point, side);
      }
    });
  }

  fn mouse_up(&mut self, position: gpui::Point<Pixels>, click_count: usize, layout: CellLayout) {
    let (point, side) = self.grid_point_and_side(position, layout);
    let is_simple_click = self.state.with_term_mut(|term| {
      if let Some(ref mut selection) = term.selection {
        selection.update(point, side);
      }
      click_count == 1 && term.selection.as_ref().is_some_and(|sel| sel.ty == SelectionType::Simple)
    });
    if is_simple_click {
      let text = self.state.with_term(|t| t.selection_to_string());
      if text.is_none_or(|s| s.is_empty()) {
        self.state.with_term_mut(|term| term.selection = None);
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
      .on_action(cx.listener(Self::send_tab))
      .on_action(cx.listener(Self::send_shift_tab))
      .on_action(cx.listener(Self::send_enter))
      .on_action(cx.listener(Self::send_shift_enter))
      .on_mouse_down(
        MouseButton::Left,
        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
          let mouse_mode = this.state.with_term(|t| t.mode().intersects(TermMode::MOUSE_MODE));
          if mouse_mode && !event.modifiers.shift {
            this.send_mouse_event(0, true, event.position, layout);
          } else {
            this.mouse_down(event.position, event.click_count, layout);
          }
          cx.notify();
        }),
      )
      .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
        if event.dragging() {
          let mouse_mode = this.state.with_term(|t| t.mode().intersects(TermMode::MOUSE_MODE));
          if mouse_mode && !event.modifiers.shift {
            this.send_mouse_event(32, true, event.position, layout);
          } else {
            this.mouse_drag(event.position, layout);
          }
          cx.notify();
        }
      }))
      .on_mouse_up(
        MouseButton::Left,
        cx.listener(move |this, event: &MouseUpEvent, _window, cx| {
          let mouse_mode = this.state.with_term(|t| t.mode().intersects(TermMode::MOUSE_MODE));
          if mouse_mode && !event.modifiers.shift {
            this.send_mouse_event(0, false, event.position, layout);
          } else {
            this.mouse_up(event.position, event.click_count, layout);
          }
          cx.notify();
        }),
      )
      .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _window, cx| {
        let delta_lines = match event.delta {
          gpui::ScrollDelta::Lines(delta) => -delta.y as i32,
          gpui::ScrollDelta::Pixels(delta) => {
            let line_height = layout.height;
            -(delta.y / line_height).round() as i32
          }
        };
        if delta_lines == 0 {
          return;
        }

        let mode = this.state.with_term(|t| *t.mode());
        let has_mouse = mode.intersects(TermMode::MOUSE_MODE);
        let alt_scroll =
          mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL);

        if has_mouse || alt_scroll {
          if alt_scroll && !has_mouse {
            // Send cursor up/down key sequences
            let (key, count) = if delta_lines > 0 {
              (b"\x1b[B" as &[u8], delta_lines as usize) // Down
            } else {
              (b"\x1b[A" as &[u8], (-delta_lines) as usize) // Up
            };
            for _ in 0..count {
              let _ = this.pty.write_all(key);
            }
          } else {
            // Send SGR mouse scroll events
            let button = if delta_lines > 0 { 65 } else { 64 };
            let count = delta_lines.unsigned_abs() as usize;
            let (point, _) = this.grid_point_and_side(event.position, layout);
            let col = point.column.0 + 1;
            let row = point.line.0 + 1;
            if mode.contains(TermMode::SGR_MOUSE) {
              let seq = format!("\x1b[<{button};{col};{row}M");
              for _ in 0..count {
                let _ = this.pty.write_all(seq.as_bytes());
              }
            } else {
              let cb = button + 32;
              let cx_byte = (col as u8).saturating_add(32);
              let cy_byte = (row as u8).saturating_add(32);
              let seq = [b'\x1b', b'[', b'M', cb, cx_byte, cy_byte];
              for _ in 0..count {
                let _ = this.pty.write_all(&seq);
              }
            }
          }
        } else {
          // Normal mode: scroll the scrollback buffer
          this.state.with_term_mut(|term| {
            term.scroll_display(Scroll::Delta(-delta_lines));
          });
        }
        cx.notify();
      }))
      .on_key_down(cx.listener({
        let pty = self.pty.clone();
        move |this, event: &KeyDownEvent, _window, _cx| {
          this.reset_cursor_blink();
          let mode = this.state.with_term(|t| *t.mode());
          if let Some(bytes) = keystroke_to_bytes(&event.keystroke, mode) {
            this.state.with_term_mut(|term| {
              term.selection = None;
              term.scroll_display(Scroll::Bottom);
            });
            let _ = pty.write_all(&bytes);
          }
        }
      }))
      .child(
        canvas(
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

              // The layout bounds may be larger than the visible area because
              // `height: 100%` in the size_full() chain can resolve against the
              // window rather than the flex-allocated space.  Use the content
              // mask (set by parent overflow_hidden) to get the true visible size.
              let visible = prep_bounds.intersect(&window.content_mask().bounds);
              let new_cols = (visible.size.width / layout.width).floor() as u16;
              let new_rows = (visible.size.height / layout.height).floor() as u16;
              let mut last = last_size.lock();
              if new_cols > 0 && new_rows > 0 && (new_cols != last.0 || new_rows != last.1) {
                *last = (new_cols, new_rows);
                let pixel_width = f32::from(visible.size.width) as u16;
                let pixel_height = f32::from(visible.size.height) as u16;
                let _ = pty_for_resize.resize(new_cols, new_rows, pixel_width, pixel_height);
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
                focused,
                cursor_visible,
                selection: selection_range,
              };
              paint_terminal(&term, prep_bounds, layout, &render_state, window, cx);
            }
          },
        )
        .size_full(),
      )
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::pin::pin;
  use std::task::{Context as TaskContext, Waker};

  /// Run a future that is expected to finish on its first poll. No executor is
  /// involved, so parking is a test failure rather than a wait.
  fn drive<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = Waker::noop();
    match future.as_mut().poll(&mut TaskContext::from_waker(waker)) {
      Poll::Ready(value) => value,
      Poll::Pending => panic!("future parked with nothing to wake it"),
    }
  }

  /// A deadline that has already come due.
  fn elapsed() -> impl Future<Output = Instant> {
    std::future::ready(Instant::now())
  }

  /// A batch and the deadline coming due at the same moment must resolve as the
  /// batch. Preferring the deadline would drop that wake, and the settle window
  /// would never learn the child had printed.
  #[test]
  fn a_batch_wins_a_tie_with_the_deadline() {
    let (tx, rx) = flume::unbounded::<()>();
    tx.send(()).unwrap();
    assert_eq!(
      drive(recv_or_deadline(&rx, Some(elapsed()))),
      Wake::Batch,
      "a parsed batch must not be lost to a deadline that came due at the same time"
    );
  }

  /// With nothing to receive, the deadline is what wakes the relay — that is
  /// how the settle window's own timing gets serviced at all.
  #[test]
  fn the_deadline_wakes_the_relay_when_no_batch_arrives() {
    let (_tx, rx) = flume::unbounded::<()>();
    assert_eq!(drive(recv_or_deadline(&rx, Some(elapsed()))), Wake::Deadline);
  }

  /// Losing the race must not lose the message: the receive future is dropped
  /// when the deadline wins, and a batch sent just after it must still be
  /// delivered on the next call. This is what lets the relay re-arm its wait
  /// after every settle step without swallowing output.
  #[test]
  fn a_batch_sent_around_the_deadline_survives_the_dropped_receive() {
    let (tx, rx) = flume::unbounded::<()>();
    assert_eq!(drive(recv_or_deadline(&rx, Some(elapsed()))), Wake::Deadline);
    tx.send(()).unwrap();
    assert_eq!(
      drive(recv_or_deadline(&rx, Some(elapsed()))),
      Wake::Batch,
      "the batch queued after the deadline won must survive the dropped receive future"
    );
  }

  /// A dead child ends the relay rather than leaving it waiting on a channel
  /// nothing can send to.
  #[test]
  fn a_closed_channel_reports_closed() {
    let (tx, rx) = flume::unbounded::<()>();
    drop(tx);
    assert_eq!(drive(recv_or_deadline(&rx, Some(elapsed()))), Wake::Closed);
  }
}
