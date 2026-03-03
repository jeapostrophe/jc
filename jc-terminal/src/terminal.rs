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
  Bell,
  Title(String),
  Exit,
  ChildExit(i32),
  PtyWrite(String),
  CursorBlinkingChange,
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
      Event::Bell => TerminalEvent::Bell,
      Event::Title(s) => TerminalEvent::Title(s),
      Event::Exit => TerminalEvent::Exit,
      Event::ChildExit(code) => TerminalEvent::ChildExit(code),
      Event::PtyWrite(s) => TerminalEvent::PtyWrite(s),
      Event::CursorBlinkingChange => TerminalEvent::CursorBlinkingChange,
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

  /// Get a clone of the Arc for use in canvas closures.
  pub fn term_handle(&self) -> Arc<Mutex<Term<EventProxy>>> {
    self.term.clone()
  }
}
