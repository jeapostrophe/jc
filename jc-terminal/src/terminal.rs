use alacritty_terminal::event::Event;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::term::Config;
use alacritty_terminal::term::Term;
use parking_lot::Mutex;
use std::sync::Arc;

/// Events forwarded from the terminal to the host application.
#[derive(Debug, Clone)]
pub enum TerminalEvent {
  Wakeup,
  Title,
  Exit,
  ChildExit,
  PtyWrite(String),
  CursorBlinkingChange,
  /// The child asked to put this text on the clipboard (OSC 52).
  ClipboardStore(String),
}

/// Forwards alacritty events to a flume channel.
pub struct EventProxy {
  tx: flume::Sender<TerminalEvent>,
}

impl EventProxy {
  pub fn new(tx: flume::Sender<TerminalEvent>) -> Self {
    Self { tx }
  }
}

impl EventListener for EventProxy {
  fn send_event(&self, event: Event) {
    let terminal_event = match event {
      Event::Wakeup => TerminalEvent::Wakeup,
      Event::Title(_) => TerminalEvent::Title,
      Event::Exit => TerminalEvent::Exit,
      Event::ChildExit(_) => TerminalEvent::ChildExit,
      Event::PtyWrite(s) => TerminalEvent::PtyWrite(s),
      Event::CursorBlinkingChange => TerminalEvent::CursorBlinkingChange,
      // macOS has one clipboard: a selection store (`p`, `s`) goes there too.
      Event::ClipboardStore(_, text) => TerminalEvent::ClipboardStore(text),
      _ => return,
    };
    let _ = self.tx.send(terminal_event);
  }
}

/// Dimensions type for terminal sizing.
pub struct TermDimensions {
  pub cols: usize,
  pub rows: usize,
}

impl alacritty_terminal::grid::Dimensions for TermDimensions {
  fn total_lines(&self) -> usize {
    self.rows
  }

  fn screen_lines(&self) -> usize {
    self.rows
  }

  fn columns(&self) -> usize {
    self.cols
  }
}

/// Wraps alacritty's Term behind a shared mutex.
pub struct TerminalState {
  term: Arc<Mutex<Term<EventProxy>>>,
}

impl TerminalState {
  pub fn new(cols: usize, rows: usize, event_tx: flume::Sender<TerminalEvent>) -> Self {
    let proxy = EventProxy::new(event_tx);
    let dims = TermDimensions { cols, rows };
    let config = Config::default();
    let term = Term::new(config, &dims, proxy);

    Self { term: Arc::new(Mutex::new(term)) }
  }

  /// Read-only access to the terminal.
  pub fn with_term<R>(&self, f: impl FnOnce(&Term<EventProxy>) -> R) -> R {
    let term = self.term.lock();
    f(&term)
  }

  /// Mutable access to the terminal.
  pub fn with_term_mut<R>(&self, f: impl FnOnce(&mut Term<EventProxy>) -> R) -> R {
    let mut term = self.term.lock();
    f(&mut term)
  }

  /// The last line with text at or above the cursor, trimmed: what a child
  /// that exited said last (a tmux client's reason).
  pub fn last_text_line(&self) -> Option<String> {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    self.with_term(|term| {
      let grid = term.grid();
      let top = -(grid.history_size() as i32);
      let cursor = grid.cursor.point.line.0;
      (top..=cursor).rev().find_map(|line| {
        let row = &grid[Line(line)];
        let text: String = (0..grid.columns()).map(|c| row[Column(c)].c).collect();
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
      })
    })
  }

  /// Get a clone of the Arc for use in canvas closures.
  pub fn term_handle(&self) -> Arc<Mutex<Term<EventProxy>>> {
    self.term.clone()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn printed(bytes: &[u8]) -> TerminalState {
    let (tx, _rx) = flume::unbounded();
    let state = TerminalState::new(40, 6, tx);
    let mut processor = alacritty_terminal::vte::ansi::Processor::<
      alacritty_terminal::vte::ansi::StdSyncHandler,
    >::default();
    state.with_term_mut(|term| processor.advance(term, bytes));
    state
  }

  #[test]
  fn the_last_line_a_child_printed_is_its_reason() {
    // What a tmux client that lost its server leaves on screen.
    let state = printed(b"$ ls\r\nfile\r\n[lost server]\r\n");
    assert_eq!(state.last_text_line().as_deref(), Some("[lost server]"));
    // Lines scrolled into history still count.
    let state = printed(b"no server running on /h/tmux.sock\r\n\r\n\r\n\r\n\r\n\r\n\r\n");
    assert_eq!(state.last_text_line().as_deref(), Some("no server running on /h/tmux.sock"));
    assert_eq!(printed(b"").last_text_line(), None);
  }
}
