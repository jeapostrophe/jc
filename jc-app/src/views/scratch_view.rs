//! jcc's Scratch panel (Cira PLAN TMUX-11): the Cira project's scratch pad,
//! edited here and saved through Cira's daemon half a second after typing
//! stops, one save at a time. The decisions are `jc_core::scratch::Pad`'s;
//! this view runs the requests off the main thread and applies the answers.
//! It is read when its project is named, when it is shown, when the window
//! becomes active, and when the daemon's change feed says its pad was saved
//! elsewhere (Cira PLAN TMUX-15: the workspace follows it,
//! `jc_core::scratch::feed`).

use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use jc_core::scratch::{self, Failure, Notice, Pad, Saved, Scratch, Ticket};
use std::path::PathBuf;
use std::time::Duration;

actions!(scratch_view, [TakeTheirs]);

/// Cmd-S is the workspace's save (bound in the editor's own context, which
/// wins over this view's), which calls [`ScratchView::save_now`].
pub fn init(cx: &mut App) {
  cx.bind_keys([KeyBinding::new("cmd-r", TakeTheirs, Some("ScratchView"))]);
}

/// How long after the last keystroke the pad is saved (the page's pause).
const SAVE_PAUSE: Duration = Duration::from_millis(500);

/// Where a pad lives: the daemon's socket and the Cira project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
  pub socket: PathBuf,
  pub project: String,
}

pub struct ScratchView {
  editor: Entity<InputState>,
  pad: Pad,
  target: Option<Target>,
  /// Bumped at each edit; a pause's save goes ahead only if no edit followed.
  edits: u64,
  _subscription: Subscription,
}

impl ScratchView {
  pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
    let editor = cx.new(|cx| {
      InputState::new(window, cx).code_editor("markdown").soft_wrap(true).line_number(false)
    });
    let subscription =
      cx.subscribe_in(&editor, window, |this, _, event: &InputEvent, window, cx| {
        if matches!(event, InputEvent::Change) {
          this.edited(window, cx);
        }
      });
    Self { editor, pad: Pad::default(), target: None, edits: 0, _subscription: subscription }
  }

  /// The Cira project whose pad this is, once named.
  pub fn project(&self) -> Option<&str> {
    self.target.as_ref().map(|t| t.project.as_str())
  }

  pub fn editor(&self) -> &Entity<InputState> {
    &self.editor
  }

  fn text(&self, cx: &App) -> String {
    self.editor.read(cx).value().to_string()
  }

  /// The pad this panel shows: set at each Cira read naming the project. A
  /// new one starts over and is read; edits still unsaved on the previous
  /// one are sent to it first, and its answers are dropped (`Pad::retarget`).
  pub fn set_target(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
    if self.target.as_ref() == Some(&target) {
      return;
    }
    if let Some(old) = self.target.replace(target) {
      let ours = self.text(cx);
      if let Some((text, base, _)) = self.pad.to_save(&ours) {
        cx.background_executor()
          .spawn(async move { scratch::write(&old.socket, &old.project, &text, &base) })
          .detach();
      }
      self.pad.retarget();
      self.editor.update(cx, |s, cx| s.set_value("", window, cx));
    }
    self.refresh(window, cx);
    cx.notify();
  }

  /// Read the pad again: its newer copy is shown, merged with unsaved edits.
  /// While a request is out the read waits for its answer (`Pad::ask_read`).
  pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(target) = self.target.clone() else { return };
    let Some(ticket) = self.pad.ask_read() else { return };
    let read =
      cx.background_executor().spawn(async move { scratch::read(&target.socket, &target.project) });
    cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| {
      let got = read.await;
      let _ = this.update_in(cx, |v, window, cx| v.on_read(ticket, got, window, cx));
    })
    .detach();
  }

  /// A notice from the change feed: a save of this pad that it does not
  /// hold yet (another client's), or a feed that started over, reads it
  /// again. Its own save, told back, reads nothing once its answer is in.
  pub fn told(&mut self, notice: &Notice, window: &mut Window, cx: &mut Context<Self>) {
    let news = match notice {
      Notice::Saved { project, version } => {
        self.project() == Some(project.as_str()) && self.pad.stale(version)
      }
      Notice::Missed => true,
    };
    if news {
      self.refresh(window, cx);
    }
  }

  fn on_read(
    &mut self,
    ticket: Ticket,
    got: Result<Scratch, Failure>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let ours = self.text(cx);
    if let Some(text) = self.pad.read(ticket, &ours, got) {
      self.editor.update(cx, |s, cx| s.set_value_preserving_position(text, window, cx));
    }
    self.next(window, cx); // a merge is saved
    cx.notify();
  }

  /// After an answer: the read owed, else the save owed.
  fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.pad.read_owed() {
      self.refresh(window, cx);
    } else {
      self.save(window, cx);
    }
  }

  fn edited(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if !self.pad.dirty(&self.text(cx)) {
      return;
    }
    self.edits += 1;
    let edits = self.edits;
    cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| {
      Timer::after(SAVE_PAUSE).await;
      let _ = this.update_in(cx, |v, window, cx| {
        if v.edits == edits {
          v.save(window, cx);
        }
      });
    })
    .detach();
  }

  /// Send the save owed, if any.
  fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(target) = self.target.clone() else { return };
    let Some((text, base, ticket)) = self.pad.to_save(&self.text(cx)) else { return };
    let write = cx
      .background_executor()
      .spawn(async move { scratch::write(&target.socket, &target.project, &text, &base) });
    cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| {
      let got = write.await;
      let _ = this.update_in(cx, |v, window, cx| v.on_saved(ticket, got, window, cx));
    })
    .detach();
  }

  fn on_saved(
    &mut self,
    ticket: Ticket,
    got: Result<Scratch, Failure>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    match self.pad.saved(ticket, got) {
      Saved::Stale => return,
      // Typed on while it was in flight, or refused as made on an older copy.
      Saved::Kept | Saved::Conflict => self.next(window, cx),
      Saved::Failed if self.pad.read_owed() => self.refresh(window, cx),
      Saved::Failed => {}
    }
    cx.notify();
  }

  /// At quit: save what is owed, waiting for the answer. Whether nothing is
  /// left unsaved (a save still in flight counts as unsaved).
  pub fn flush(&mut self, cx: &mut Context<Self>) -> bool {
    let ours = self.text(cx);
    if !self.pad.dirty(&ours) {
      return true;
    }
    let (Some(target), Some((text, base, ticket))) = (self.target.clone(), self.pad.to_save(&ours))
    else {
      return false;
    };
    let got = scratch::write(&target.socket, &target.project, &text, &base);
    self.pad.saved(ticket, got) == Saved::Kept
  }

  fn take_theirs(&mut self, _: &TakeTheirs, window: &mut Window, cx: &mut Context<Self>) {
    if let Some(text) = self.pad.take_theirs() {
      self.editor.update(cx, |s, cx| s.set_value_preserving_position(text, window, cx));
      cx.notify();
    } else {
      self.refresh(window, cx);
    }
  }

  /// Cmd-S: save now, keeping these edits over a newer copy they conflict
  /// with.
  pub fn save_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.pad.keep_mine() {
      cx.notify();
    }
    self.save(window, cx);
  }
}

impl Focusable for ScratchView {
  fn focus_handle(&self, cx: &App) -> FocusHandle {
    self.editor.read(cx).focus_handle(cx)
  }
}

impl Render for ScratchView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let waiting = match (&self.target, self.pad.ready()) {
      (None, _) => Some("The scratch pad opens once Cira names this project".to_string()),
      (Some(_), false) if self.pad.banner().is_none() => Some("Reading the scratch pad…".into()),
      _ => None,
    };
    div()
      .id("scratch-view")
      .key_context("ScratchView")
      .track_focus(&self.editor.read(cx).focus_handle(cx))
      .size_full()
      .font_family("Lilex")
      .on_action(cx.listener(Self::take_theirs))
      .child(super::file_banner(self.pad.banner().or(waiting), cx))
      // Until a read has succeeded it takes no typing (`Pad::ready`).
      .child(
        Input::new(&self.editor)
          .h_full()
          .appearance(false)
          .bordered(false)
          .disabled(!self.pad.ready()),
      )
  }
}
