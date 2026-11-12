use crate::views::code_view::CodeView;
use crate::views::project_state::SavedPaneLayout;
use gpui::*;
use gpui_component::input::Position;
use jc_core::background::Claude;
use jc_core::todo::SessionKey;
use jc_terminal::{ChildSpec, LaunchSettle, Palette, TerminalConfig, TerminalView};
use std::path::Path;
use std::time::Duration;

/// Snapshot of per-session viewport state, saved on switch-away and restored on switch-back.
pub struct SavedViewState {
  pub layout: SavedPaneLayout,
  pub todo_cursor: Position,
  pub todo_scroll: Point<Pixels>,
  /// Terminal display offsets (lines scrolled back from bottom).
  pub claude_scroll: usize,
  pub general_scroll: usize,
}

pub type SessionId = usize;

/// Discounts an attach's own startup output from the Cmd-P activity marker:
/// `claude attach` repaints the whole screen when it connects, which is jc
/// attaching rather than work done while you were away. The window reopens at
/// every re-attach.
///
/// `quiet` is longer than a moment because a resuming session pauses between
/// its first frame and replaying a long transcript, and that pause must not
/// end the window. `give_up` bounds it, so a child that never goes quiet is
/// baselined anyway rather than tracked for the life of the process. These are
/// facts about the Claude CLI, which is why they live here and not in the
/// terminal crate.
const CLAUDE_LAUNCH_SETTLE: LaunchSettle =
  LaunchSettle { quiet: Duration::from_secs(4), give_up: Duration::from_secs(30) };

pub struct SessionState {
  pub id: SessionId,
  /// The heading's address in TODO.md (`jc_core::todo::SessionKey`): its
  /// `> bg=` short id, or — until the session is migrated — its legacy
  /// `> uuid=`. Empty while a new session is still being started and has no
  /// heading yet.
  pub key: String,
  /// The background session the Claude pane attaches to (`claude attach
  /// <bg>`). Empty until it is known; equal to `key` once the heading is bound.
  pub bg: String,
  /// The conversation uuid hooks report, learned from the first hook (it
  /// starts with `bg`), a `/clear` pair, or `claude agents --json`. Empty until
  /// learned. Never written to TODO.md.
  pub uuid: String,
  /// The stable id (`> cira=`), passed as `CIRA_SESSION`.
  pub cira: Option<String>,
  /// The shell pane was started with `CIRA_SESSION` (see
  /// [`Self::export_cira_to_shell`]).
  shell_has_cira: bool,
  /// Whether the background session runs with `--dangerously-skip-permissions`
  /// — the mode it was started with, compared against the heading's
  /// `> dangerous` to notice a change.
  pub dangerous: bool,
  /// The session process is running, so text typed into the attach reaches
  /// it. Cleared whenever the attach client restarts, set again by
  /// `claude agents --json` or any hook from the session.
  pub live: bool,
  /// A Cmd-Enter message waiting for `live` (see `Workspace::flush_pending_send`).
  pub pending_send: Option<String>,
  /// A start, migration or `> dangerous` change is in flight.
  pub changing: bool,
  /// A `claude agents --json` poll for `live` is in flight.
  pub watching_live: bool,
  /// For a session adopted from a heading an older jc left unbound: that
  /// heading's verified-position address, used to write its `> bg=` line once
  /// the session has started.
  pub unbound_key: Option<SessionKey>,
  /// The `> dangerous` value a failed change targeted, so it is not retried
  /// every reconcile tick. Cleared when the heading changes again.
  pub refused_dangerous: Option<bool>,
  pub label: String,
  pub claude_terminal: Entity<TerminalView>,
  pub general_terminal: Entity<TerminalView>,
  pub code_view: Entity<CodeView>,
  pub saved_view: Option<SavedViewState>,
  /// The Claude pane's child events, observed by the workspace.
  pub _claude_child: Option<Subscription>,
}

impl SessionState {
  /// Note that the user has given this session work.
  ///
  /// Ends the Claude terminal's launch-settle window WITHOUT clearing its
  /// marker — see [`TerminalView::cancel_launch_settle`].
  pub fn mark_user_input(&self, cx: &App) {
    self.claude_terminal.read(cx).cancel_launch_settle();
  }

  /// A session whose Claude pane attaches to `bg`, or — with `bg` empty —
  /// waits for [`Self::attach`] once its background session is known.
  #[allow(clippy::too_many_arguments)]
  pub fn create(
    id: SessionId,
    key: String,
    bg: String,
    label: String,
    cira: Option<String>,
    dangerous: bool,
    claude: &Claude,
    project_path: &Path,
    palette: &Palette,
    window: &mut Window,
    cx: &mut App,
  ) -> Self {
    let claude_config = TerminalConfig {
      child: (!bg.is_empty()).then(|| ChildSpec::Command(claude.attach_argv(&bg))),
      respawn: true,
      palette: Some(palette.clone()),
      launch_settle: Some(CLAUDE_LAUNCH_SETTLE),
      ..Default::default()
    };
    // The shell pane is JC's own (Cira Q-33); it carries the stable id too.
    let general_config = TerminalConfig {
      palette: Some(palette.clone()),
      env: cira.iter().map(|c| ("CIRA_SESSION".to_string(), c.clone())).collect(),
      ..Default::default()
    };

    let project = project_path.to_path_buf();
    let claude_terminal = cx.new(|cx| TerminalView::new(claude_config, Some(&project), window, cx));
    let general_terminal =
      cx.new(|cx| TerminalView::new(general_config, Some(&project), window, cx));
    let code_view = cx.new(|cx| CodeView::new(window, cx));

    Self {
      id,
      key,
      bg,
      uuid: String::new(),
      shell_has_cira: cira.is_some(),
      cira,
      dangerous,
      live: false,
      pending_send: None,
      changing: false,
      watching_live: false,
      unbound_key: None,
      refused_dangerous: None,
      label,
      claude_terminal,
      general_terminal,
      code_view,
      saved_view: None,
      _claude_child: None,
    }
  }

  /// Attach the Claude pane to background session `short`, which is now this
  /// session's address.
  pub fn attach(&mut self, short: &str, claude: &Claude, cx: &mut App) {
    self.key = short.to_string();
    self.bg = short.to_string();
    self.live = false;
    let argv = claude.attach_argv(short);
    self.claude_terminal.update(cx, |t, cx| t.spawn(ChildSpec::Command(argv), cx));
  }

  /// Restart the shell pane with `CIRA_SESSION` when its stable id was minted
  /// after the shell started (a migrated or new session). Only ever happens
  /// while the session is first being started, seconds after the shell opened.
  pub fn export_cira_to_shell(&mut self, cx: &mut App) {
    let Some(cira) = self.cira.clone().filter(|_| !self.shell_has_cira) else { return };
    self.shell_has_cira = true;
    self.general_terminal.update(cx, |t, cx| {
      t.set_env(vec![("CIRA_SESSION".to_string(), cira)]);
      t.spawn(ChildSpec::Shell, cx);
    });
  }

  /// Print a `[jc]` status line in the Claude pane.
  pub fn status(&self, text: &str, cx: &mut App) {
    self.claude_terminal.update(cx, |t, cx| t.show_status(text, cx));
  }
}
