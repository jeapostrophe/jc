use crate::views::code_view::CodeView;
use crate::views::project_state::SavedPaneLayout;
use gpui::*;
use gpui_component::input::Position;
use jc_core::background::{self, Claude};
use jc_core::cira::{self, HookState, Marker};
use jc_core::hooks::HookEventKind;
use jc_core::todo::{IdKind, SessionKey};
use jc_terminal::{ChildSpec, Palette, TerminalConfig, TerminalView};
use std::path::Path;
use std::time::{Duration, SystemTime};

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
  /// The stable id (`> cira=`), passed as `CIRA_SESSION`. Minted at creation
  /// for a heading jc will start a session for (see [`Heading::new`]), so the
  /// shell pane has it from the start; written to the heading when the
  /// session is bound.
  pub cira: Option<String>,
  /// Whether the background session runs with `--dangerously-skip-permissions`
  /// — the mode it was started with, compared against the heading's
  /// `> dangerous` to notice a change.
  pub dangerous: bool,
  /// The session process is running, so text typed into the attach reaches
  /// it. Cleared whenever the attach client restarts, set again by
  /// `claude agents --json` or any hook from the session.
  pub live: bool,
  /// A start, migration or `> dangerous` change is in flight. A session that
  /// is changing is never removed: its completion still has to bind the
  /// heading to whatever job it started.
  pub changing: bool,
  /// Its start failed or was refused; the pane says why. It no longer counts
  /// as adopted, so the pickers offer the heading again.
  pub failed: bool,
  /// A `claude agents --json` poll for `live` is in flight.
  pub watching_live: bool,
  /// For a session adopted from a heading an older jc left unbound: that
  /// heading's verified-position address, used to write its `> bg=` line once
  /// the session has started.
  pub unbound_key: Option<SessionKey>,
  /// For a session continuing a transcript no heading names: that
  /// conversation's uuid, while its start is in flight.
  pub adopting: Option<String>,
  /// The `> dangerous` value a failed change targeted, so it is not retried
  /// every reconcile tick. Cleared when the heading changes again.
  pub refused_dangerous: Option<bool>,
  /// Cira's state for this session and since when, from the last `cira
  /// status --json`; `None` while Cira is unreachable or does not know it.
  pub cira_state: Option<(cira::State, Option<SystemTime>)>,
  /// The state jc's own hooks last reported, and when it changed to it.
  pub hook_state: Option<(HookState, SystemTime)>,
  /// When the session was last on screen: set as jc switches away from it.
  /// `None` until then, and the marker falls back to `> last=`.
  pub seen: Option<SystemTime>,
  pub label: String,
  pub claude_terminal: Entity<TerminalView>,
  pub general_terminal: Entity<TerminalView>,
  pub code_view: Entity<CodeView>,
  pub saved_view: Option<SavedViewState>,
  /// The Claude pane's child events, observed by the workspace.
  pub claude_child_events: Option<Subscription>,
}

/// What a session is created from: its heading's id (empty for a session whose
/// heading is written once it has started), label, stable id and mode.
pub struct Heading {
  pub key: String,
  pub label: String,
  pub cira: Option<String>,
  pub dangerous: bool,
}

impl Heading {
  /// `cira` is minted here when jc is about to start a session for the
  /// heading (a legacy `> uuid=`, an unbound or a new heading) and it has
  /// none. A `> bg=` heading without one is left alone: its running session
  /// was started without a `CIRA_SESSION`, and claiming one would be false.
  pub fn new(
    key: String,
    kind: IdKind,
    label: String,
    cira: Option<String>,
    dangerous: bool,
  ) -> Self {
    let starts = key.is_empty() || kind == IdKind::Uuid;
    let cira = cira.or_else(|| starts.then(|| uuid::Uuid::new_v4().to_string()));
    Self { key, label, cira, dangerous }
  }
}

impl SessionState {
  /// Record what a hook received at `at` says about the session's state, if
  /// it says anything (`jc_core::cira::note_hook`).
  pub fn note_hook(&mut self, kind: &HookEventKind, at: SystemTime) {
    if let Some(next) = HookState::from_hook(kind) {
      self.hook_state = Some(cira::note_hook(self.hook_state, next, at));
    }
  }

  /// The Cmd-P picker's marker (`jc_core::cira::marker`). `last_active` is the
  /// heading's `> last=`, the last send: the session was on screen then, which
  /// is the best jc knows before it has switched away from it.
  pub fn marker(&self, last_active: u64) -> Option<Marker> {
    let last_send =
      (last_active > 0).then(|| SystemTime::UNIX_EPOCH + Duration::from_secs(last_active));
    cira::marker(self.cira_state, self.hook_state, self.seen.or(last_send))
  }

  /// Does a hook naming conversation `hook_session` come from this session?
  pub fn owns_hook(&self, hook_session: &str) -> bool {
    background::hook_matches(&self.bg, &self.uuid, hook_session)
  }

  /// A session whose Claude pane waits for [`Self::attach`].
  pub fn create(
    id: SessionId,
    heading: Heading,
    project_path: &Path,
    palette: &Palette,
    window: &mut Window,
    cx: &mut App,
  ) -> Self {
    let Heading { key, label, cira, dangerous } = heading;
    let claude_config = TerminalConfig {
      child: None,
      respawn: true,
      palette: Some(palette.clone()),
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
      bg: String::new(),
      uuid: String::new(),
      cira,
      dangerous,
      live: false,
      changing: false,
      failed: false,
      watching_live: false,
      unbound_key: None,
      adopting: None,
      refused_dangerous: None,
      cira_state: None,
      hook_state: None,
      seen: None,
      label,
      claude_terminal,
      general_terminal,
      code_view,
      saved_view: None,
      claude_child_events: None,
    }
  }

  /// Attach the Claude pane to background session `short`, which is now this
  /// session's address.
  ///
  /// The pane re-attaches whenever the client exits, and attaching resumes a
  /// stopped job. So anything that stops or replaces the job must detach first
  /// (drop the session, or `TerminalView::stop_child`), or the pane brings the
  /// job straight back.
  pub fn attach(&mut self, short: &str, claude: &Claude, cx: &mut App) {
    self.key = short.to_string();
    self.bg = short.to_string();
    self.live = false;
    let argv = claude.attach_argv(short);
    self.claude_terminal.update(cx, |t, cx| t.spawn(ChildSpec::Command(argv), cx));
  }

  /// Print a `[jc]` status line in the Claude pane.
  pub fn status(&self, text: &str, cx: &mut App) {
    self.claude_terminal.update(cx, |t, cx| t.show_status(text, cx));
  }
}
