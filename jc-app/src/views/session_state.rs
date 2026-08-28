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
  /// True while Claude is actively working. Set by `UserPromptSubmit` hook and
  /// `send_to_terminal`; cleared by `Stop`/`StopFailure`/`IdlePrompt` hooks.
  pub busy: bool,
  pub saved_view: Option<SavedViewState>,
}

impl SessionState {
  /// Note that the user has given this session work.
  ///
  /// Ends the Claude terminal's launch-settle window WITHOUT clearing its
  /// marker — see [`TerminalView::cancel_launch_settle`].
  pub fn mark_user_input(&mut self, cx: &App) {
    self.busy = true;
    self.claude_terminal.read(cx).cancel_launch_settle();
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
    // Claude's banner and transcript replay are jc starting the session, not
    // work done while you were away, so they must not leave the Cmd-P marker
    // set. Opened here rather than at startup because `Workspace::open_project`
    // can restore a project's sessions at any point in the run.
    claude_terminal.update(cx, |terminal, cx| terminal.discount_launch_output(cx));
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
      busy: false,
      saved_view: None,
    }
  }
}
