use crate::views::code_view::CodeView;
use crate::views::project_state::SavedPaneLayout;
use gpui::*;
use gpui_component::input::Position;
use jc_terminal::{Palette, TerminalConfig, TerminalView};
use std::path::Path;

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

/// Progress toward taking a session's Cmd-P activity baseline.
///
/// A freshly spawned `claude` prints a banner and, when resuming, replays its
/// transcript. That is jc starting the session, not work that happened while you
/// were away, so it must not leave the session marked. Each session waits for
/// its OWN child to settle and is then baselined exactly once — a per-session
/// state machine rather than a startup-wide one, because projects can be opened
/// at any time (`Workspace::open_project`) and a session already baselined must
/// never be silently re-baselined over real activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityBaseline {
  /// Still settling. `last_batches` is the output count at the previous tick,
  /// `quiet_ticks` how many consecutive ticks it has not moved, and `ticks` the
  /// total elapsed, so a child that never settles is still bounded.
  Pending { last_batches: usize, quiet_ticks: usize, ticks: usize },
  /// Settled (or the session got user input, which ends the startup window
  /// early). Leave this session's counter alone from now on.
  Taken,
}

impl Default for ActivityBaseline {
  fn default() -> Self {
    Self::Pending { last_batches: 0, quiet_ticks: 0, ticks: 0 }
  }
}

/// How the Claude CLI should attach to a session UUID.
///
/// The two are NOT interchangeable, and picking wrong leaves a dead pane.
/// Measured 2026-08-27: `claude --resume <uuid>` with no `<uuid>.jsonl` on disk
/// prints "No conversation found with session ID: <uuid>" and exits — that is
/// true both for a UUID Claude has garbage-collected and for one jc minted but
/// never sent a prompt to. `claude --session-id <uuid>` on that same UUID starts
/// a fresh conversation under it. So the choice is made by whether the
/// transcript exists ([`ProjectState::launch_for`]), and a session whose
/// transcript vanished is revived rather than retired — which is why jc has no
/// "expired" state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
  /// `--session-id <uuid>`: claim a UUID with no conversation behind it.
  New,
  /// `--resume <uuid>`: continue an existing conversation. Requires the
  /// transcript to be present.
  Resume,
}

pub struct SessionState {
  #[allow(dead_code)]
  pub id: SessionId,
  /// The Claude session UUID. Assigned by jc at launch (`--session-id`) and
  /// recorded in TODO.md, so it is always present.
  pub uuid: String,
  pub label: String,
  pub claude_terminal: Entity<TerminalView>,
  pub general_terminal: Entity<TerminalView>,
  pub code_view: Entity<CodeView>,
  /// Whether this session's startup output has been discounted yet.
  pub activity_baseline: ActivityBaseline,
  /// True while Claude is actively working. Set by `UserPromptSubmit` hook and
  /// `send_to_terminal`; cleared by `Stop`/`StopFailure`/`IdlePrompt` hooks.
  pub busy: bool,
  pub saved_view: Option<SavedViewState>,
}

impl SessionState {
  /// Note that the user has given this session work.
  ///
  /// Ends the startup-baseline window WITHOUT clearing the marker: from here on
  /// everything the child prints is a response to something you asked for, so it
  /// is activity by definition. Without this, a prompt sent during the settle
  /// window keeps resetting `quiet_ticks`, and the baseline is finally taken at
  /// the exact moment Claude finishes — wiping the `*` for the work you were
  /// waiting on.
  pub fn mark_user_input(&mut self) {
    self.busy = true;
    self.activity_baseline = ActivityBaseline::Taken;
  }

  #[allow(clippy::too_many_arguments)]
  pub fn create(
    id: SessionId,
    uuid: String,
    label: String,
    project_path: &Path,
    palette: &Palette,
    dangerous: bool,
    launch: Launch,
    window: &mut Window,
    cx: &mut App,
  ) -> Self {
    let flag = match launch {
      Launch::New => "--session-id",
      Launch::Resume => "--resume",
    };
    let mut command = format!("claude {flag} {uuid}");
    if dangerous {
      command.push_str(" --dangerously-skip-permissions");
    }

    let claude_config = TerminalConfig {
      command: Some(command),
      palette: Some(palette.clone()),
      ..Default::default()
    };
    let general_config = TerminalConfig { palette: Some(palette.clone()), ..Default::default() };

    let project = project_path.to_path_buf();
    let claude_terminal = cx.new(|cx| TerminalView::new(claude_config, Some(&project), window, cx));
    let general_terminal =
      cx.new(|cx| TerminalView::new(general_config, Some(&project), window, cx));
    let code_view = cx.new(|cx| CodeView::new(window, cx));

    Self {
      id,
      uuid,
      label,
      claude_terminal,
      general_terminal,
      code_view,
      activity_baseline: ActivityBaseline::default(),
      busy: false,
      saved_view: None,
    }
  }
}
