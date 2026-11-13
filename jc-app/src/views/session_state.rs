use crate::views::code_view::CodeView;
use crate::views::project_state::SavedPaneLayout;
use gpui::*;
use gpui_component::input::Position;
use jc_core::background::{self, Claude};
use jc_core::cira::{self, HookState, Marker};
use jc_core::hooks::HookEventKind;
use jc_core::lifecycle::{Backoff, ShellChild, ShellPane, ShellRun};
use jc_core::todo::SessionKey;
use jc_terminal::{ChildSpec, LaunchSettle, Palette, TerminalConfig, TerminalView};
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

/// Discounts `claude`'s own startup output from jc's Cmd-P activity marker: its
/// banner, and the transcript replay on `--resume`, are jc launching the
/// session rather than work done while you were away.
///
/// `quiet` is longer than a moment because `claude --resume` pauses between
/// printing the banner and replaying a long transcript, and that pause must not
/// end the window. `give_up` bounds it, so a child that never goes quiet is
/// baselined anyway rather than tracked for the life of the process. These are
/// facts about the Claude CLI, which is why they live here and not in the
/// terminal crate.
const CLAUDE_LAUNCH_SETTLE: LaunchSettle =
  LaunchSettle { quiet: Duration::from_secs(4), give_up: Duration::from_secs(30) };

/// How a session's panes start, by flavor (`jc_core::flavor::Flavor`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
  /// jc: the Claude pane runs this `claude` command line itself
  /// (`jc_core::claude::claude_argv`), the Terminal pane jc's own shell.
  Own(Vec<String>),
  /// jcc: the Claude pane waits for [`SessionState::attach`] to the job Cira
  /// names, the Terminal pane for the shell Cira keeps.
  Cira,
}

pub struct SessionState {
  pub id: SessionId,
  /// The heading's address in TODO.md (`jc_core::todo::SessionKey`): for jc
  /// its `> uuid=`; for jcc its `> cira=` id, or — until the session is
  /// migrated — a legacy `> uuid=`. Empty while a new jcc session's heading is
  /// not written yet.
  pub key: String,
  /// jc only: Claude is working (set by a send and the `UserPromptSubmit`
  /// hook, cleared by `Stop`/`StopFailure`/`IdlePrompt`), so quitting asks first.
  pub busy: bool,
  /// The id Cira minted for it (`> cira=`), its `CIRA_SESSION`. `None` until
  /// Cira has answered `cira session new` for it.
  pub cira: Option<String>,
  /// The short id the Claude pane's attach client runs on (`claude attach
  /// <bg>`), empty while no client runs. Cira names it; jc never keeps it.
  pub bg: String,
  /// The conversation uuid hooks report. jc: the heading's, followed through
  /// a `/clear`. jcc: learned from Cira, a hook, or a `/clear` pair; empty
  /// until learned, and never written to TODO.md.
  pub uuid: String,
  /// The session process is running, so text typed into the attach reaches
  /// it. Cleared whenever the attach client restarts, set again by
  /// `claude agents --json` or any hook from the session.
  pub live: bool,
  /// A `cira session new` for it is in flight; nothing else is asked for it
  /// until it answers.
  pub asking: bool,
  /// Its `cira session new` failed (the pane says why). It no longer counts
  /// as adopted, so the pickers offer the heading again.
  pub failed: bool,
  /// When its `cira session new` went unanswered: the next status read is
  /// searched for the session it may have made.
  pub lost: Option<SystemTime>,
  /// A `claude agents --json` poll for `live` is in flight.
  pub watching_live: bool,
  /// For a session adopted from a heading an older jc left unbound: that
  /// heading's verified-position address, used to write its `> cira=` line
  /// once Cira has minted it.
  pub unbound_key: Option<SessionKey>,
  /// For a session continuing a transcript: that conversation's uuid, while
  /// Cira is asked to start it.
  pub adopting: Option<String>,
  /// Cira's view of it from the last `cira status --json`; `None` while
  /// Cira is unreachable or does not list it.
  pub cira_session: Option<cira::Session>,
  /// When the Claude pane last attached, or Jay last asked it to: a stop
  /// through Cira after this is not undone by attaching.
  pub attach_since: Option<SystemTime>,
  /// When its attach client last exited, and the pause before the next
  /// attach.
  pub exited: Backoff,
  /// The last reason the pane showed for not attaching, so it is shown once.
  pub shown: Option<String>,
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
  /// What the Terminal pane runs, and its asks of Cira.
  pub shell: ShellPane,
  /// The Terminal pane's child events, observed by the workspace.
  pub shell_child_events: Option<Subscription>,
}

/// What a session is created from: its heading's id (empty for a session whose
/// heading is written once Cira has answered), label, and Cira id.
pub struct Heading {
  pub key: String,
  pub label: String,
  pub cira: Option<String>,
}

impl Heading {
  pub fn of(todo: &jc_core::todo::TodoSession) -> Self {
    Self { key: todo.id.clone(), label: todo.label.clone(), cira: todo.cira().map(str::to_string) }
  }
}

impl SessionState {
  /// jc: note that the user has given this session work.
  ///
  /// Ends the Claude terminal's launch-settle window WITHOUT clearing its
  /// marker — see [`TerminalView::cancel_launch_settle`].
  pub fn mark_user_input(&mut self, cx: &App) {
    self.busy = true;
    self.claude_terminal.read(cx).cancel_launch_settle();
  }

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
  /// By the short id its pane attaches to, or the job Cira bound (a fork the
  /// pane has not followed yet), else its current uuid.
  pub fn owns_hook(&self, hook_session: &str) -> bool {
    let job = self.cira_session.as_ref().and_then(|s| s.job.as_ref()).map(|j| j.short.as_str());
    background::hook_matches(&self.bg, &self.uuid, hook_session)
      || job.is_some_and(|short| background::hook_matches(short, "", hook_session))
  }

  /// Cira's desired mode for it, once Cira lists it.
  pub fn dangerous(&self) -> Option<bool> {
    self.cira_session.as_ref().map(|s| s.dangerous)
  }

  /// A session started as `start` says: jc's runs `claude` at once, jcc's
  /// waits for [`Self::attach`].
  #[allow(clippy::too_many_arguments)]
  pub fn create(
    id: SessionId,
    heading: Heading,
    start: Start,
    project_path: &Path,
    palette: &Palette,
    window: &mut Window,
    cx: &mut App,
  ) -> Self {
    let Heading { key, label, cira } = heading;
    let own = match start {
      Start::Own(argv) => Some(argv),
      Start::Cira => None,
    };
    let (claude_config, general_config) = match own {
      Some(argv) => (
        TerminalConfig {
          child: Some(ChildSpec::Command(argv)),
          palette: Some(palette.clone()),
          launch_settle: Some(CLAUDE_LAUNCH_SETTLE),
          ..Default::default()
        },
        TerminalConfig { palette: Some(palette.clone()), ..Default::default() },
      ),
      None => (
        TerminalConfig { child: None, palette: Some(palette.clone()), ..Default::default() },
        // The shell pane attaches to the session's shell in Cira's tmux server
        // (TMUX-02), asked for by its Cira id at the first status read once Cira
        // has minted one (`ShellPane`).
        TerminalConfig {
          child: None,
          palette: Some(palette.clone()),
          env: shell_env(cira.as_deref()),
          ..Default::default()
        },
      ),
    };
    let is_own = claude_config.child.is_some();

    let project = project_path.to_path_buf();
    let claude_terminal = cx.new(|cx| TerminalView::new(claude_config, Some(&project), window, cx));
    let general_terminal =
      cx.new(|cx| TerminalView::new(general_config, Some(&project), window, cx));
    let code_view = cx.new(|cx| CodeView::new(window, cx));

    if !is_own && cira.is_none() {
      general_terminal
        .update(cx, |t, cx| t.show_status("the shell starts once Cira has named this session", cx));
    }
    Self {
      id,
      uuid: if is_own { key.clone() } else { String::new() },
      key,
      busy: false,
      cira,
      bg: String::new(),
      live: false,
      asking: false,
      failed: false,
      lost: None,
      watching_live: false,
      unbound_key: None,
      adopting: None,
      cira_session: None,
      attach_since: None,
      exited: Backoff::default(),
      shown: None,
      cira_state: None,
      hook_state: None,
      seen: None,
      label,
      claude_terminal,
      general_terminal,
      code_view,
      saved_view: None,
      claude_child_events: None,
      shell: ShellPane::default(),
      shell_child_events: None,
    }
  }

  /// Cira has minted `cira` for this session, whose heading it now
  /// addresses; the shell pane asks for its shell at the next status read.
  pub fn bind(&mut self, cira: &str, cx: &mut App) {
    self.key = cira.to_string();
    self.cira = Some(cira.to_string());
    let env = shell_env(Some(cira));
    self.general_terminal.update(cx, |t, _| t.set_env(env));
  }

  /// Run `child` in the shell pane (`ShellPane` decided it). jc's own shell
  /// keeps `CIRA_SESSION` and says it is not shared.
  pub fn run_shell(&mut self, child: ShellChild, cx: &mut App) {
    self.general_terminal.update(cx, |t, cx| match child {
      ShellChild::Shared(argv) => t.spawn(ChildSpec::Command(argv), cx),
      ShellChild::Own(why) => {
        t.show_status(
          &format!(
            "not shared: {why}. This shell is jc's own and ends when jc quits; exit it to \
             ask Cira again."
          ),
          cx,
        );
        t.spawn(ChildSpec::Shell, cx);
      }
      ShellChild::Held(why) => t.show_status(&format!("no shell: {why}"), cx),
      ShellChild::Retry(why) => {
        t.show_status(&format!("Cira gave no shell: {why}; asking again"), cx)
      }
    });
  }

  /// The shell pane runs jc's own shell: its header says "not shared".
  pub fn shell_not_shared(&self) -> bool {
    self.shell.run() == ShellRun::Own
  }

  /// Attach the Claude pane to the job `short` Cira names, replacing any
  /// client it runs.
  pub fn attach(&mut self, short: &str, claude: &Claude, cx: &mut App) {
    self.bg = short.to_string();
    self.live = false;
    self.shown = None;
    self.attach_since = Some(SystemTime::now());
    let argv = claude.attach_argv(short);
    self.claude_terminal.update(cx, |t, cx| t.spawn(ChildSpec::Command(argv), cx));
  }

  /// Show why the pane attaches nothing, once per reason.
  pub fn show_once(&mut self, why: &str, cx: &mut App) {
    if self.shown.as_deref() != Some(why) {
      self.shown = Some(why.to_string());
      self.status(why, cx);
    }
  }

  /// Print a `[jc]` status line in the Claude pane.
  pub fn status(&self, text: &str, cx: &mut App) {
    self.claude_terminal.update(cx, |t, cx| t.show_status(text, cx));
  }
}

/// The shell pane's added environment: the session's Cira id.
fn shell_env(cira: Option<&str>) -> Vec<(String, String)> {
  cira.map(|c| vec![("CIRA_SESSION".to_string(), c.to_string())]).unwrap_or_default()
}
