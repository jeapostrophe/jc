use crate::problem::ScriptProblem;
use std::path::Path;
use std::process::Command;

/// Parse a single line of status.sh output into a `ScriptProblem`.
///
/// Format: `{rank:}?file{:line}? - message`
///
/// Examples:
///   - `file:line - message`
///   - `file - message`
///   - `3:file:line - message`
///   - `3:file - message`
pub fn parse_line(line: &str) -> Option<ScriptProblem> {
  let (prefix, message) = line.split_once(" - ")?;
  let message = message.to_string();
  if prefix.is_empty() {
    return None;
  }

  let parts: Vec<&str> = prefix.splitn(3, ':').collect();

  match parts.len() {
    // "file"
    1 => Some(ScriptProblem { rank: None, file: parts[0].into(), line: None, message }),
    // "file:line" or "rank:file"
    2 => {
      if let Ok(rank) = parts[0].parse::<i8>() {
        // rank:file
        Some(ScriptProblem { rank: Some(rank), file: parts[1].into(), line: None, message })
      } else {
        // file:line
        let line_num = parts[1].parse::<usize>().ok();
        Some(ScriptProblem { rank: None, file: parts[0].into(), line: line_num, message })
      }
    }
    // "rank:file:line"
    3 => {
      let rank = parts[0].parse::<i8>().ok();
      let line_num = parts[2].parse::<usize>().ok();
      Some(ScriptProblem { rank, file: parts[1].into(), line: line_num, message })
    }
    _ => None,
  }
}

/// Run `./status.sh` in the given project directory and parse stdout into problems.
///
/// Returns an empty vec if the script doesn't exist, isn't executable, or exits
/// with a non-zero status code.  Stderr is ignored.
pub fn run_status_script(project_path: &Path) -> Vec<ScriptProblem> {
  let script = project_path.join("status.sh");
  if !script.exists() {
    return Vec::new();
  }

  let output = match Command::new("./status.sh")
    .current_dir(project_path)
    .stderr(std::process::Stdio::null())
    .output()
  {
    Ok(o) => o,
    Err(_) => return Vec::new(),
  };

  if !output.status.success() {
    return Vec::new();
  }

  let stdout = String::from_utf8_lossy(&output.stdout);
  stdout.lines().filter_map(parse_line).collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parse_file_and_message() {
    let p = parse_line("src/main.rs - something broke").unwrap();
    assert_eq!(p.rank, None);
    assert_eq!(p.file.to_str().unwrap(), "src/main.rs");
    assert_eq!(p.line, None);
    assert_eq!(p.message, "something broke");
  }

  #[test]
  fn parse_file_line_and_message() {
    let p = parse_line("src/main.rs:42 - something broke").unwrap();
    assert_eq!(p.rank, None);
    assert_eq!(p.file.to_str().unwrap(), "src/main.rs");
    assert_eq!(p.line, Some(42));
    assert_eq!(p.message, "something broke");
  }

  #[test]
  fn parse_rank_file_and_message() {
    let p = parse_line("3:src/main.rs - something broke").unwrap();
    assert_eq!(p.rank, Some(3));
    assert_eq!(p.file.to_str().unwrap(), "src/main.rs");
    assert_eq!(p.line, None);
    assert_eq!(p.message, "something broke");
  }

  #[test]
  fn parse_rank_file_line_and_message() {
    let p = parse_line("3:src/main.rs:42 - something broke").unwrap();
    assert_eq!(p.rank, Some(3));
    assert_eq!(p.file.to_str().unwrap(), "src/main.rs");
    assert_eq!(p.line, Some(42));
    assert_eq!(p.message, "something broke");
  }

  #[test]
  fn parse_no_separator_returns_none() {
    assert!(parse_line("just some text").is_none());
  }

  #[test]
  fn parse_empty_prefix_returns_none() {
    assert!(parse_line(" - message").is_none());
  }

  #[test]
  fn parse_message_with_extra_dashes() {
    let p = parse_line("file.rs - foo - bar - baz").unwrap();
    assert_eq!(p.message, "foo - bar - baz");
  }
}
