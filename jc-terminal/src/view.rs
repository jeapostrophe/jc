use crate::colors::Palette;
use crate::input::keystroke_to_bytes;
use crate::pty::{ChildSpec, PtyChild, PtyHandle};
use crate::render::{CellLayout, TerminalRenderState, measure_cell, paint_terminal};
use crate::settle::{LaunchSettle, SettleStep, SettleWindow};
use crate::terminal::{TerminalEvent, TerminalState};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::term::TermMode;
use gpui::{
  App, AsyncApp, Bounds, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, FutureExt,
  InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent,
  MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, ScrollWheelEvent, SharedString,
  Styled, Subscription, Task, Timer, WeakEntity, Window, actions, canvas, div, px,
};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);
const FONT_SIZE_STEP: Pixels = px(2.0);
const FONT_SIZE_MIN: Pixels = px(8.0);
const FONT_SIZE_MAX: Pixels = px(72.0);

/// Bounds of the delay before a respawned child: [`RESPAWN_MIN`] after a
/// child that ran at least [`RESPAWN_STABLE`], doubling up to [`RESPAWN_MAX`]
/// while it keeps exiting sooner (an attach to a session that is gone, a
/// missing binary).
const RESPAWN_MIN: Duration = Duration::from_secs(1);
const RESPAWN_MAX: Duration = Duration::from_secs(30);
const RESPAWN_STABLE: Duration = Duration::from_secs(10);

/// How long to wait before re-running a child that just exited after running
/// for `lived`, given the delay used before it (`None` for a first start).
fn respawn_delay(previous: Option<Duration>, lived: Duration) -> Duration {
  match previous {
    Some(previous) if lived < RESPAWN_STABLE => (previous * 2).clamp(RESPAWN_MIN, RESPAWN_MAX),
    _ => RESPAWN_MIN,
  }
}

/// The bytes a wheel movement of `lines` (positive = down) sends to a child
/// that asked for mouse reporting or alternate scroll, reported at 1-based
/// cell `col`, `row`. `None` when the child asked for neither, and the
/// movement belongs to jc's own scrollback.
fn wheel_bytes(mode: TermMode, lines: i32, col: usize, row: usize) -> Option<Vec<u8>> {
  let has_mouse = mode.intersects(TermMode::MOUSE_MODE);
  let alt_scroll = mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL);
  if lines == 0 || !(has_mouse || alt_scroll) {
    return None;
  }
  let count = lines.unsigned_abs() as usize;
  let one: Vec<u8> = if !has_mouse {
    // Alternate scroll: cursor keys.
    if lines > 0 { b"\x1b[B".to_vec() } else { b"\x1b[A".to_vec() }
  } else {
    let button: u8 = if lines > 0 { 65 } else { 64 };
    if mode.contains(TermMode::SGR_MOUSE) {
      format!("\x1b[<{button};{col};{row}M").into_bytes()
    } else {
      let cb = button + 32;
      let cx_byte = (col as u8).saturating_add(32);
      let cy_byte = (row as u8).saturating_add(32);
      vec![b'\x1b', b'[', b'M', cb, cx_byte, cy_byte]
    }
  };
  Some(one.repeat(count))
}

/// What happened to a terminal's child process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildEvent {
  /// A child was (re)started.
  Spawned,
  /// The child exited and was reaped (`code` is `None` when it could not be
  /// started or waited for). A terminal with `respawn` set restarts it.
  Exited { code: Option<u32> },
}

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
  /// What to run. `None` starts no child until [`TerminalView::spawn`]; the
  /// pane shows [`TerminalView::show_status`] lines meanwhile.
  pub child: Option<ChildSpec>,
  /// Added to the child's environment.
  pub env: Vec<(String, String)>,
  /// Re-run the child whenever it exits, after [`respawn_delay`]. Used for
  /// `claude attach`, whose exit (Ctrl+Z, Esc from the agents view, a
  /// `claude stop`) only detaches the client from a session that lives on.
  pub respawn: bool,
  /// Discount this child's launch output from [`TerminalView::has_unseen_output`],
  /// so the flag means "it printed something while you were away" rather than
  /// "jc started it".
  ///
  /// `None` (the default) counts every batch from the moment the child starts.
  /// `Some(policy)` opens a launch-settle window: the baseline is taken once
  /// the child has printed something and then held still for `policy.quiet`, or
  /// unconditionally at `policy.give_up`. Both exits clear, for the reason
  /// recorded on `SettleWindow::step`. The caller supplies the durations
  /// because what counts as settled is a fact about the child being run.
  pub launch_settle: Option<LaunchSettle>,
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
      child: Some(ChildSpec::Shell),
      env: Vec::new(),
      respawn: false,
      launch_settle: None,
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

/// GPUI view that embeds a terminal emulator.
pub struct TerminalView {
  state: TerminalState,
  /// The running child's PTY; `None` before the first spawn and after an exit.
  /// Dropping it hangs the child up.
  pty: Option<Arc<PtyHandle>>,
  working_dir: Option<PathBuf>,
  /// Bumped at every (re)spawn, so an exit reported by a replaced child is
  /// ignored.
  generation: u64,
  spawned_at: Instant,
  /// `output_batches` when the current child started.
  batches_at_spawn: usize,
  /// The delay used before the last respawn (see [`respawn_delay`]).
  last_respawn_delay: Option<Duration>,
  _respawn_task: Option<Task<()>>,
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
  /// Launch-settle window, opened from `TerminalConfig::launch_settle`, or
  /// `None` when none is open — the case for a terminal that asked for no
  /// window, and again once the notification relay (which both stamps batches
  /// onto it and drives it) has seen one through to its end.
  ///
  /// Main-thread only, like `seen_batches`: the VTE thread never touches it,
  /// and the relay runs on the foreground executor. The lock is for `&self`
  /// interior mutability across two owners, not for cross-thread contention.
  settle: Arc<Mutex<Option<SettleWindow>>>,
  /// Events the emulator raises (terminal replies), drained by the VTE thread.
  term_events: flume::Receiver<TerminalEvent>,
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

    let (event_tx, event_rx) = flume::unbounded();
    let state = TerminalState::new(cols as usize, rows as usize, event_tx);
    let palette = config.palette.take().unwrap_or_default();
    let default_font_size = config.font_size;
    let focus = cx.focus_handle();

    let _subscriptions = vec![
      cx.on_focus(&focus, _window, Self::on_focus),
      cx.on_blur(&focus, _window, Self::on_blur),
    ];

    let mut view = Self {
      state,
      pty: None,
      working_dir: working_dir.map(PathBuf::from),
      generation: 0,
      spawned_at: Instant::now(),
      batches_at_spawn: 0,
      last_respawn_delay: None,
      _respawn_task: None,
      term_events: event_rx,
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
      visible: Arc::new(AtomicBool::new(true)),
      output_batches: Arc::new(AtomicUsize::new(0)),
      seen_batches: AtomicUsize::new(0),
      settle: Arc::new(Mutex::new(None)),
      _subscriptions,
      _blink_task: None,
    };
    view.start_child(cx);
    view
  }

  /// Run `spec` in this terminal, hanging up any child it already runs, and
  /// keep re-running it on exit if the terminal was made with `respawn`.
  pub fn spawn(&mut self, spec: ChildSpec, cx: &mut Context<Self>) {
    self.config.child = Some(spec);
    self.last_respawn_delay = None;
    self.start_child(cx);
  }

  /// Replace the environment added to children started from now on.
  pub fn set_env(&mut self, env: Vec<(String, String)>) {
    self.config.env = env;
  }

  /// Hang up the child and run nothing until the next [`Self::spawn`].
  pub fn stop_child(&mut self, cx: &mut Context<Self>) {
    self.config.child = None;
    self.hang_up();
    cx.notify();
  }

  /// Drop the current child (hanging it up), any pending respawn, and any exit
  /// it has yet to report.
  fn hang_up(&mut self) {
    self.generation += 1;
    self.pty = None;
    self._respawn_task = None;
  }

  /// Has the current child printed anything yet?
  pub fn has_output_since_spawn(&self) -> bool {
    self.pty.is_some() && self.output_batches.load(Ordering::Relaxed) != self.batches_at_spawn
  }

  /// Print a dim `[jc] text` line, as if the child had printed it. For a
  /// terminal waiting on something before its child can start, or one whose
  /// child exited.
  pub fn show_status(&mut self, text: &str, cx: &mut Context<Self>) {
    let line = format!("\r\n\x1b[2m[jc] {text}\x1b[0m\r\n");
    let mut processor = alacritty_terminal::vte::ansi::Processor::<
      alacritty_terminal::vte::ansi::StdSyncHandler,
    >::default();
    self.state.with_term_mut(|term| processor.advance(term, line.as_bytes()));
    cx.notify();
  }

  /// Start `config.child`, replacing (and hanging up) any current child.
  fn start_child(&mut self, cx: &mut Context<Self>) {
    self.hang_up();
    let generation = self.generation;
    let Some(spec) = self.config.child.clone() else { return };

    let (cols, rows) = *self.last_size.lock();
    let child =
      match PtyChild::spawn(&spec, &self.config.env, cols, rows, self.working_dir.as_deref()) {
        Ok(child) => child,
        Err(e) => {
          self.show_status(&format!("could not start {spec:?}: {e:#}"), cx);
          self.spawned_at = Instant::now();
          self.on_child_exit(generation, None, cx);
          return;
        }
      };
    self.pty = Some(child.handle.clone());
    self.spawned_at = Instant::now();
    self.batches_at_spawn = self.output_batches.load(Ordering::Relaxed);
    if let Some(policy) = self.config.launch_settle {
      *self.settle.lock() = Some(SettleWindow::new(policy, Instant::now()));
    }
    cx.emit(ChildEvent::Spawned);

    // Background thread: VTE parsing off the main thread. The heavy
    // `processor.advance()` work runs here; only lightweight `cx.notify()`
    // happens on the main executor. It holds the PTY weakly, for terminal
    // replies only, so dropping the view still hangs the child up.
    let term_handle = self.state.term_handle();
    let pty_for_write: Weak<PtyHandle> = Arc::downgrade(&child.handle);
    let term_events = self.term_events.clone();
    let visible_for_bg = self.visible.clone();
    let output_batches_for_bg = self.output_batches.clone();
    let bytes_rx = child.bytes;
    let (notify_tx, notify_rx) = flume::unbounded::<()>(); // one signal per parsed batch
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
        while let Ok(event) = term_events.try_recv() {
          if let TerminalEvent::PtyWrite(s) = event
            && let Some(pty) = pty_for_write.upgrade()
          {
            let _ = pty.write_all(s.as_bytes());
          }
        }
        if notify_tx.send(()).is_err() {
          break;
        }
      }
    });

    // Lightweight main-thread relay: notify GPUI for repaint, and drive the
    // launch-settle window off the same batch signal. One task, two wake
    // sources — the batch that would reset a debounce is exactly the batch this
    // loop already waits on.
    //
    // The window is installed above, before the relay exists, rather than
    // being poked in afterwards: a window that arrived after the relay had
    // already parked on an unbounded receive would never be serviced, and a
    // child that then went quiet would leave the marker set for the life of
    // the process.
    let visible_for_relay = self.visible.clone();
    let settle_for_relay = self.settle.clone();
    let exited = child.exited;
    cx.spawn(async move |this: WeakEntity<TerminalView>, cx: &mut AsyncApp| {
      loop {
        // Service the settle window first: it either wants the baseline taken
        // now, or says how long this loop may wait before it needs asking
        // again. With no window open there is nothing to time out on.
        //
        // `finished` is what makes "baselined exactly once" a property of the
        // tested type; dropping the window here is the cheap consequence, so
        // the steady state stops stepping entirely.
        let step = settle_for_relay.lock().as_mut().map(|window| window.step(Instant::now()));
        let deadline = match step {
          Some(SettleStep::Wait(duration)) => Some(duration),
          Some(SettleStep::Clear) => {
            if this.update(cx, |view, _| view.clear_output_seen()).is_err() {
              return; // view dropped
            }
            *settle_for_relay.lock() = None;
            None
          }
          Some(SettleStep::Done) => {
            *settle_for_relay.lock() = None;
            None
          }
          None => None,
        };

        // A ready batch beats a ready deadline (`WithTimeout` polls the inner
        // future first), so a wake is never dropped in the tie.
        let woke = match deadline {
          Some(duration) => {
            notify_rx.recv_async().with_timeout(duration, cx.background_executor()).await
          }
          None => Ok(notify_rx.recv_async().await),
        };

        match woke {
          // The child printed. Restart the quiet wait — even while hidden,
          // since a backgrounded session's child is still settling and the
          // marker it feeds is read from the picker.
          Ok(Ok(())) => {
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
          Err(_) => {}
          // The VTE thread is gone: the child has closed the PTY.
          Ok(Err(_)) => break,
        }
      }
      let code = exited.recv_async().await.ok().flatten();
      let _ = this.update(cx, |view, cx| view.on_child_exit(generation, code, cx));
    })
    .detach();
  }

  fn on_child_exit(&mut self, generation: u64, code: Option<u32>, cx: &mut Context<Self>) {
    if generation != self.generation {
      return; // a replaced child
    }
    self.pty = None;
    cx.emit(ChildEvent::Exited { code });
    let code = code.map_or("unknown".to_string(), |c| c.to_string());
    if !self.config.respawn {
      self.show_status(&format!("exited (code {code})"), cx);
      return;
    }
    let delay = respawn_delay(self.last_respawn_delay, self.spawned_at.elapsed());
    self.last_respawn_delay = Some(delay);
    self.show_status(&format!("exited (code {code}); restarting in {}s", delay.as_secs()), cx);
    self._respawn_task =
      Some(cx.spawn(async move |this: WeakEntity<TerminalView>, cx: &mut AsyncApp| {
        Timer::after(delay).await;
        let _ = this.update(cx, |view, cx| {
          if view.generation == generation {
            view.start_child(cx);
          }
        });
      }));
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

  fn write_pty(&self, bytes: &[u8]) {
    if let Some(pty) = &self.pty {
      let _ = pty.write_all(bytes);
    }
  }

  /// Returns true if the terminal has bracketed-paste mode enabled.
  pub fn bracketed_paste_mode(&self) -> bool {
    self.state.with_term(|t| t.mode().contains(TermMode::BRACKETED_PASTE))
  }

  /// Get a clone of the PTY handle for use in background threads, or `None`
  /// while no child runs.
  pub fn pty_handle(&self) -> Option<Arc<PtyHandle>> {
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
      self.write_pty(&buf);
    } else {
      let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
      self.write_pty(normalized.as_bytes());
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

  /// Scroll by the given number of lines (positive = down), as the wheel
  /// would: a child that asked for mouse reporting (`claude attach`, which
  /// runs fullscreen on the alternate screen with no scrollback) gets wheel
  /// events at the middle of the pane; otherwise jc's scrollback moves.
  pub fn scroll_lines(&mut self, lines: i32, cx: &mut Context<Self>) {
    let mode = self.state.with_term(|t| *t.mode());
    let (cols, rows) = *self.last_size.lock();
    let col = cols as usize / 2 + 1;
    let row = rows as usize / 2 + 1;
    if let Some(bytes) = wheel_bytes(mode, lines, col, row) {
      self.write_pty(&bytes);
      return;
    }
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
      self.write_pty(b"\x1b[I");
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
      self.write_pty(b"\x1b[O");
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
    self.write_pty(b"\t");
  }

  fn send_shift_tab(&mut self, _: &SendShiftTab, _window: &mut Window, _cx: &mut Context<Self>) {
    self.reset_cursor_blink();
    self.state.with_term_mut(|term| {
      term.selection = None;
      term.scroll_display(Scroll::Bottom);
    });
    self.write_pty(b"\x1b[Z");
  }

  fn send_enter(&mut self, _: &SendEnter, _window: &mut Window, _cx: &mut Context<Self>) {
    self.reset_cursor_blink();
    self.state.with_term_mut(|term| {
      term.selection = None;
      term.scroll_display(Scroll::Bottom);
    });
    self.write_pty(b"\r");
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
    self.write_pty(b"\x1b[13;2u");
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
      self.write_pty(seq.as_bytes());
    } else {
      let cb = button + 32;
      let cx_byte = (col as u8).saturating_add(32);
      let cy_byte = (row as u8).saturating_add(32);
      let seq = [b'\x1b', b'[', b'M', cb, cx_byte, cy_byte];
      self.write_pty(&seq);
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

impl EventEmitter<ChildEvent> for TerminalView {}

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
        let (point, _) = this.grid_point_and_side(event.position, layout);
        let (col, row) = (point.column.0 + 1, (point.line.0 + 1).max(1) as usize);
        if let Some(bytes) = wheel_bytes(mode, delta_lines, col, row) {
          this.write_pty(&bytes);
        } else {
          // Normal mode: scroll the scrollback buffer
          this.state.with_term_mut(|term| {
            term.scroll_display(Scroll::Delta(-delta_lines));
          });
        }
        cx.notify();
      }))
      .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, _cx| {
        this.reset_cursor_blink();
        let mode = this.state.with_term(|t| *t.mode());
        if let Some(bytes) = keystroke_to_bytes(&event.keystroke, mode) {
          this.state.with_term_mut(|term| {
            term.selection = None;
            term.scroll_display(Scroll::Bottom);
          });
          this.write_pty(&bytes);
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
                if let Some(pty) = &pty_for_resize {
                  let _ = pty.resize(new_cols, new_rows, pixel_width, pixel_height);
                }
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

  #[test]
  fn respawn_backs_off_while_the_child_keeps_exiting_quickly() {
    let quick = Duration::from_millis(200);
    assert_eq!(respawn_delay(None, quick), RESPAWN_MIN);
    let mut delay = respawn_delay(None, quick);
    for _ in 0..10 {
      delay = respawn_delay(Some(delay), quick);
    }
    assert_eq!(delay, RESPAWN_MAX, "doubles up to the cap");
    // A child that ran a while (a long attach ended by Ctrl+Z) comes back fast.
    assert_eq!(respawn_delay(Some(RESPAWN_MAX), RESPAWN_STABLE), RESPAWN_MIN);
  }

  /// What `claude attach` asks for (SP-03): alt screen, mouse 1000/1002/1003
  /// with SGR encoding.
  fn attach_mode() -> TermMode {
    TermMode::ALT_SCREEN
      | TermMode::MOUSE_REPORT_CLICK
      | TermMode::MOUSE_DRAG
      | TermMode::MOUSE_MOTION
      | TermMode::SGR_MOUSE
  }

  #[test]
  fn wheel_goes_to_a_child_in_mouse_mode_as_sgr_buttons() {
    assert_eq!(wheel_bytes(attach_mode(), 2, 40, 12).unwrap(), b"\x1b[<65;40;12M\x1b[<65;40;12M");
    assert_eq!(wheel_bytes(attach_mode(), -1, 1, 1).unwrap(), b"\x1b[<64;1;1M");
  }

  #[test]
  fn wheel_moves_jc_scrollback_for_a_plain_child() {
    // A plain shell or a plain `claude`: no mouse mode, primary screen.
    assert_eq!(wheel_bytes(TermMode::default() - TermMode::ALTERNATE_SCROLL, 3, 1, 1), None);
    assert_eq!(
      wheel_bytes(TermMode::default(), 3, 1, 1),
      None,
      "alternate scroll needs the alt screen"
    );
    assert_eq!(wheel_bytes(attach_mode(), 0, 1, 1), None);
  }

  #[test]
  fn alternate_scroll_without_mouse_sends_cursor_keys() {
    let mode = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
    assert_eq!(wheel_bytes(mode, -2, 1, 1).unwrap(), b"\x1b[A\x1b[A");
  }
}
