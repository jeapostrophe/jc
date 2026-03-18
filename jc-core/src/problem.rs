use std::path::PathBuf;

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
  Stop,
  Permission,
  Idle,
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
// Rank and description
// ---------------------------------------------------------------------------

impl SessionProblem {
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
      Self::Claude(ClaudeProblem::Stop) => 3,
      Self::Claude(ClaudeProblem::Idle) => 4,
      Self::Terminal(TerminalProblem::Bell) => 5,
      Self::Todo(AppTodoProblem::UnsentWait { .. }) => 6,
    }
  }

  pub fn description(&self) -> String {
    match self {
      Self::Claude(ClaudeProblem::Permission) => "Permission prompt".into(),
      Self::Claude(ClaudeProblem::Stop) => "Stopped".into(),
      Self::Claude(ClaudeProblem::Idle) => "Idle prompt".into(),
      Self::Terminal(TerminalProblem::Bell) => "Bell".into(),
      Self::Todo(AppTodoProblem::UnsentWait { label }) => format!("Unsent wait: {label}"),
    }
  }
}

impl ProjectProblem {
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
