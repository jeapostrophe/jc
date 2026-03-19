use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Problem layers
// ---------------------------------------------------------------------------

/// Priority layers for the Cmd-; problem rotation system.
/// Lower layer = higher priority. L0 problems are handled cross-session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProblemLayer {
  /// Permission prompt, API error (StopFailure) — cross-session, always first.
  L0,
  /// Terminal bell, unreviewed diffs, script problems — review before sending.
  L1,
  /// Unsent WAIT — ready to send new work (suppressed if busy or L1 exists).
  L2,
  /// Session idle + has_ever_been_busy — needs new work started.
  L3,
}

// ---------------------------------------------------------------------------
// Navigation target
// ---------------------------------------------------------------------------

/// Where to navigate when the user jumps to a problem.
#[derive(Debug, Clone, PartialEq)]
pub enum ProblemTarget {
  ClaudeTerminal,
  GeneralTerminal,
  TodoEditor,
  DiffView { file: PathBuf },
  CodeView { file: PathBuf, line: Option<usize> },
}

// ---------------------------------------------------------------------------
// Per-view leaf enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaudeProblem {
  Permission,
  StopFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalProblem {
  Bell,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DiffProblem {
  UnreviewedFile(PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptProblem {
  pub rank: Option<i8>,
  pub file: PathBuf,
  pub line: Option<usize>,
  pub message: String,
}

/// App-level view of todo problems (distinct from the parser-level `TodoProblem`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AppTodoProblem {
  UnsentWait { label: String },
}

// ---------------------------------------------------------------------------
// Wrapper enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum SessionProblem {
  Claude(ClaudeProblem),
  Terminal(TerminalProblem),
  Todo(AppTodoProblem),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProjectProblem {
  Diff(DiffProblem),
  Script(ScriptProblem),
}

// ---------------------------------------------------------------------------
// Layer, rank, target, and description
// ---------------------------------------------------------------------------

impl SessionProblem {
  pub fn layer(&self) -> ProblemLayer {
    match self {
      Self::Claude(ClaudeProblem::Permission | ClaudeProblem::StopFailure) => ProblemLayer::L0,
      Self::Terminal(TerminalProblem::Bell) => ProblemLayer::L1,
      Self::Todo(AppTodoProblem::UnsentWait { .. }) => ProblemLayer::L2,
    }
  }

  pub fn target(&self) -> ProblemTarget {
    match self {
      Self::Claude(_) => ProblemTarget::ClaudeTerminal,
      Self::Terminal(TerminalProblem::Bell) => ProblemTarget::GeneralTerminal,
      Self::Todo(_) => ProblemTarget::TodoEditor,
    }
  }

  pub fn rank(&self) -> i8 {
    match self {
      Self::Claude(ClaudeProblem::Permission) => 1,
      Self::Claude(ClaudeProblem::StopFailure) => 2,
      Self::Terminal(TerminalProblem::Bell) => 5,
      Self::Todo(AppTodoProblem::UnsentWait { .. }) => 6,
    }
  }

  pub fn description(&self) -> String {
    match self {
      Self::Claude(ClaudeProblem::Permission) => "Permission prompt".into(),
      Self::Claude(ClaudeProblem::StopFailure) => "API error".into(),
      Self::Terminal(TerminalProblem::Bell) => "Bell".into(),
      Self::Todo(AppTodoProblem::UnsentWait { label }) => format!("Unsent wait: {label}"),
    }
  }
}

impl ProjectProblem {
  pub fn layer(&self) -> ProblemLayer {
    match self {
      Self::Diff(_) => ProblemLayer::L1,
      Self::Script(_) => ProblemLayer::L1,
    }
  }

  pub fn target(&self) -> ProblemTarget {
    match self {
      Self::Diff(DiffProblem::UnreviewedFile(f)) => ProblemTarget::DiffView { file: f.clone() },
      Self::Script(sp) => ProblemTarget::CodeView { file: sp.file.clone(), line: sp.line },
    }
  }

  pub fn rank(&self) -> i8 {
    match self {
      Self::Diff(DiffProblem::UnreviewedFile(_)) => 10,
      Self::Script(sp) => sp.rank.unwrap_or(20),
    }
  }

  pub fn description(&self) -> String {
    match self {
      Self::Diff(DiffProblem::UnreviewedFile(path)) => {
        format!("Unreviewed: {}", path.display())
      }
      Self::Script(sp) => sp.message.clone(),
    }
  }
}
