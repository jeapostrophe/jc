use std::ops::Range;
use std::path::Path;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct TodoDocument {
  pub claude_section_line: Option<u32>,
  pub sessions: Vec<TodoSession>,
}

#[derive(Debug, Default, Clone)]
pub struct TodoSession {
  pub slug: String,
  pub label: String,
  pub line: u32,
  pub heading_byte_range: Range<usize>,
  pub slug_byte_range: Range<usize>,
  pub messages: Vec<TodoMessage>,
  pub wait: Option<TodoWait>,
}

#[derive(Debug, Default, Clone)]
pub struct TodoMessage {
  pub index: usize,
  pub line: u32,
  pub heading_byte_range: Range<usize>,
}

#[derive(Debug, Default, Clone)]
pub struct TodoWait {
  pub line: u32,
  pub heading_byte_range: Range<usize>,
  pub body_byte_range: Range<usize>,
}

#[derive(Debug, Clone)]
pub enum TodoProblem {
  InvalidSessionSlug { slug: String, line: u32, slug_byte_range: Range<usize> },
}

// ---------------------------------------------------------------------------
// TodoDocument methods
// ---------------------------------------------------------------------------

impl TodoDocument {
  pub fn first_session(&self) -> Option<&TodoSession> {
    self.sessions.first()
  }

  pub fn session_by_slug(&self, slug: &str) -> Option<&TodoSession> {
    self.sessions.iter().find(|s| s.slug == slug)
  }

  pub fn session_slugs(&self) -> Vec<&str> {
    self.sessions.iter().map(|s| s.slug.as_str()).collect()
  }

  /// Returns the 1-based line number of the last line of the WAIT body for the
  /// given session slug. This is the line where a user would type new content.
  /// If the WAIT body is empty, returns the line right after the heading.
  pub fn wait_body_end_line(&self, slug: &str, text: &str) -> Option<u32> {
    let wait = self.session_by_slug(slug)?.wait.as_ref()?;
    let body = &text[wait.body_byte_range.clone()];
    // Count newlines in the body to find how many lines it spans.
    let body_lines = body.chars().filter(|&c| c == '\n').count() as u32;
    // The body starts on the line after the WAIT heading.
    // If body is empty or only whitespace, place cursor on the line after heading.
    // Otherwise, place on the last non-empty line of the body.
    let last_line = wait.line + body_lines.max(1);
    Some(last_line)
  }

  /// Returns the byte offset at the end of the WAIT body for the given
  /// session. This is where comments should be inserted. If the session has
  /// no WAIT, returns `None`.
  pub fn comment_insert_offset(&self, slug: &str) -> Option<usize> {
    let session = self.session_by_slug(slug)?;
    let wait = session.wait.as_ref()?;
    Some(wait.body_byte_range.end)
  }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

pub fn parse(text: &str) -> TodoDocument {
  let mut doc = TodoDocument::default();
  let mut current_session: Option<TodoSession> = None;
  let mut byte_offset: usize = 0;

  for (line_idx, line) in text.lines().enumerate() {
    let line_num = line_idx as u32 + 1;
    let line_start = byte_offset;
    let line_end = line_start + line.len();

    if line.starts_with("# ") {
      // Any top-level heading ends the current session.
      finalize_session(&mut doc, &mut current_session, line_start);

      if line == "# Claude" {
        doc.claude_section_line = Some(line_num);
      }
    } else if let Some(after_h2) = line.strip_prefix("## ") {
      // Any second-level heading ends the current session.
      finalize_session(&mut doc, &mut current_session, line_start);

      if let Some(rest) = after_h2.strip_prefix("Session ") {
        // Split on first `: ` to get slug and label.
        if let Some(colon_pos) = rest.find(": ") {
          let slug = &rest[..colon_pos];
          let label = &rest[colon_pos + ": ".len()..];

          let slug_abs_start = line_start + "## Session ".len();
          let slug_abs_end = slug_abs_start + slug.len();

          current_session = Some(TodoSession {
            slug: slug.to_string(),
            label: label.to_string(),
            line: line_num,
            heading_byte_range: line_start..line_end,
            slug_byte_range: slug_abs_start..slug_abs_end,
            ..Default::default()
          });
        }
      }
    } else if let Some(after_h3) = line.strip_prefix("### ") {
      if after_h3 == "WAIT" {
        if let Some(ref mut session) = current_session {
          // Close any previous WAIT body range (shouldn't happen, but be safe).
          finalize_wait_body(session, line_start);

          session.wait = Some(TodoWait {
            line: line_num,
            heading_byte_range: line_start..line_end,
            body_byte_range: 0..0, // will be finalized later
          });
        }
      } else if let Some(rest) = after_h3.strip_prefix("Message ")
        && let Ok(n) = rest.parse::<usize>()
        && let Some(ref mut session) = current_session
      {
        session.messages.push(TodoMessage {
          index: n,
          line: line_num,
          heading_byte_range: line_start..line_end,
        });
      }
    }

    // Advance byte_offset past this line and its newline character.
    // Account for the newline separator. The last line may not have a trailing
    // newline, so we check bounds.
    byte_offset = line_end;
    if byte_offset < text.len() {
      // Skip the newline character(s).
      if text.as_bytes()[byte_offset] == b'\n' {
        byte_offset += 1;
      } else if text.as_bytes()[byte_offset] == b'\r' {
        byte_offset += 1;
        if byte_offset < text.len() && text.as_bytes()[byte_offset] == b'\n' {
          byte_offset += 1;
        }
      }
    }
  }

  // Finalize the last session at the end of the document.
  finalize_session(&mut doc, &mut current_session, text.len());

  doc
}

/// Push a completed session into the document, finalizing any open WAIT body range.
fn finalize_session(
  doc: &mut TodoDocument,
  current_session: &mut Option<TodoSession>,
  boundary: usize,
) {
  if let Some(mut session) = current_session.take() {
    finalize_wait_body(&mut session, boundary);
    doc.sessions.push(session);
  }
}

/// Close the WAIT body range, extending it from the end of the WAIT heading
/// line to `boundary` (the start of the next heading, or end of document).
fn finalize_wait_body(session: &mut TodoSession, boundary: usize) {
  if let Some(ref mut wait) = session.wait
    && wait.body_byte_range == (0..0)
  {
    // Body starts right after the WAIT heading line (including its newline).
    let body_start = wait.heading_byte_range.end;
    // If there's a newline after the heading, skip past it.
    wait.body_byte_range = body_start..boundary;
  }
}

// ---------------------------------------------------------------------------
// Session heading insertion
// ---------------------------------------------------------------------------

/// Returns true if the document has at least one session whose slug matches
/// a JSONL session group on disk.
pub fn has_valid_sessions(doc: &TodoDocument, project_path: &Path) -> bool {
  doc
    .sessions
    .iter()
    .any(|s| crate::session::discover_session_group(project_path, &s.slug).is_some())
}

/// Build new text with a `## Session <slug>: New session` heading inserted.
/// If a `# Claude` section exists, the heading goes right after it. Otherwise
/// a `# Claude` section is appended at the end of the text.
pub fn insert_session_heading(text: &str, doc: &TodoDocument, slug: &str) -> String {
  let heading = format!("## Session {slug}: New session\n### WAIT\n");

  if let Some(claude_line) = doc.claude_section_line {
    // Find byte offset right after the `# Claude` line.
    let mut offset = 0;
    for (i, line) in text.lines().enumerate() {
      offset += line.len();
      // Skip past the newline character(s).
      if text.as_bytes().get(offset) == Some(&b'\n') {
        offset += 1;
      } else if text.as_bytes().get(offset) == Some(&b'\r') {
        offset += 1;
        if text.as_bytes().get(offset) == Some(&b'\n') {
          offset += 1;
        }
      }
      if i as u32 + 1 == claude_line {
        let mut new_text = String::with_capacity(text.len() + heading.len());
        new_text.push_str(&text[..offset]);
        new_text.push_str(&heading);
        new_text.push_str(&text[offset..]);
        return new_text;
      }
    }
  }

  // No `# Claude` section; append one at the end.
  let mut new_text = text.to_string();
  if !new_text.is_empty() && !new_text.ends_with('\n') {
    new_text.push('\n');
  }
  if !new_text.is_empty() {
    new_text.push('\n');
  }
  new_text.push_str("# Claude\n");
  new_text.push_str(&heading);
  new_text
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

pub fn validate(doc: &TodoDocument, project_path: &Path) -> Vec<TodoProblem> {
  let mut problems = Vec::default();

  for session in &doc.sessions {
    if crate::session::discover_session_group(project_path, &session.slug).is_none() {
      problems.push(TodoProblem::InvalidSessionSlug {
        slug: session.slug.clone(),
        line: session.line,
        slug_byte_range: session.slug_byte_range.clone(),
      });
    }
  }

  problems
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn empty_document() {
    let doc = parse("");
    assert!(doc.claude_section_line.is_none());
    assert!(doc.sessions.is_empty());
    assert!(doc.first_session().is_none());
    assert!(doc.session_slugs().is_empty());
  }

  #[test]
  fn single_session_with_messages_and_wait() {
    let text = "\
# Claude
## Session my-slug: My Label
### Message 0
some body text
### Message 1
more body text
### WAIT
draft content here
";
    let doc = parse(text);

    assert_eq!(doc.claude_section_line, Some(1));
    assert_eq!(doc.sessions.len(), 1);

    let session = &doc.sessions[0];
    assert_eq!(session.slug, "my-slug");
    assert_eq!(session.label, "My Label");
    assert_eq!(session.line, 2);
    assert_eq!(session.messages.len(), 2);
    assert_eq!(session.messages[0].index, 0);
    assert_eq!(session.messages[0].line, 3);
    assert_eq!(session.messages[1].index, 1);
    assert_eq!(session.messages[1].line, 5);

    let wait = session.wait.as_ref().unwrap();
    assert_eq!(wait.line, 7);

    // The WAIT body should contain "draft content here\n".
    let body = &text[wait.body_byte_range.clone()];
    assert!(body.contains("draft content here"));
  }

  #[test]
  fn multiple_sessions() {
    let text = "\
# Claude
## Session alpha: First Session
### Message 0
body
## Session beta: Second Session
### Message 0
body
### WAIT
wait body
";
    let doc = parse(text);

    assert_eq!(doc.sessions.len(), 2);
    assert_eq!(doc.sessions[0].slug, "alpha");
    assert_eq!(doc.sessions[0].label, "First Session");
    assert_eq!(doc.sessions[1].slug, "beta");
    assert_eq!(doc.sessions[1].label, "Second Session");
    assert!(doc.sessions[0].wait.is_none());
    assert!(doc.sessions[1].wait.is_some());
  }

  #[test]
  fn session_with_no_wait() {
    let text = "\
# Claude
## Session no-wait: No Wait Here
### Message 0
body text
### Message 1
more body
";
    let doc = parse(text);

    assert_eq!(doc.sessions.len(), 1);
    let session = &doc.sessions[0];
    assert_eq!(session.slug, "no-wait");
    assert!(session.wait.is_none());
    assert_eq!(session.messages.len(), 2);
  }

  #[test]
  fn session_with_no_messages() {
    let text = "\
# Claude
## Session empty: Empty Session
### WAIT
some wait body
";
    let doc = parse(text);

    assert_eq!(doc.sessions.len(), 1);
    let session = &doc.sessions[0];
    assert_eq!(session.slug, "empty");
    assert_eq!(session.label, "Empty Session");
    assert!(session.messages.is_empty());
    assert!(session.wait.is_some());
  }

  #[test]
  fn comment_insert_offset_returns_correct_byte_offset() {
    let text = "\
# Claude
## Session test: Test Session
### WAIT
draft body
";
    let doc = parse(text);

    let offset = doc.comment_insert_offset("test").unwrap();
    // The offset should be at the end of the WAIT body, which is the end of
    // the document.
    assert_eq!(offset, text.len());

    // Verify the body range content.
    let session = doc.session_by_slug("test").unwrap();
    let wait = session.wait.as_ref().unwrap();
    let body = &text[wait.body_byte_range.clone()];
    assert!(body.contains("draft body"));
  }

  #[test]
  fn session_by_slug_and_session_slugs() {
    let text = "\
# Claude
## Session aaa: First
### Message 0
## Session bbb: Second
### WAIT
body
## Session ccc: Third
";
    let doc = parse(text);

    assert_eq!(doc.session_slugs(), vec!["aaa", "bbb", "ccc"]);
    assert_eq!(doc.session_by_slug("bbb").unwrap().label, "Second");
    assert!(doc.session_by_slug("nonexistent").is_none());
  }

  #[test]
  fn first_session_returns_first() {
    let text = "\
# Claude
## Session first: The First
## Session second: The Second
";
    let doc = parse(text);

    let first = doc.first_session().unwrap();
    assert_eq!(first.slug, "first");
    assert_eq!(first.label, "The First");
  }

  #[test]
  fn comment_insert_offset_none_without_wait() {
    let text = "\
# Claude
## Session no-wait: No Wait
### Message 0
body
";
    let doc = parse(text);
    assert!(doc.comment_insert_offset("no-wait").is_none());
  }

  #[test]
  fn slug_byte_range_covers_slug_text() {
    let text = "## Session my-slug: My Label\n";
    let doc = parse(text);
    let session = &doc.sessions[0];
    assert_eq!(&text[session.slug_byte_range.clone()], "my-slug");
  }

  #[test]
  fn heading_byte_range_covers_full_line() {
    let text = "## Session test: Test Label\nsome body\n";
    let doc = parse(text);
    let session = &doc.sessions[0];
    assert_eq!(&text[session.heading_byte_range.clone()], "## Session test: Test Label");
  }

  #[test]
  fn top_level_heading_ends_session() {
    let text = "\
## Session inside: Inside
### Message 0
# Other Section
## Session outside: Outside
";
    let doc = parse(text);
    assert_eq!(doc.sessions.len(), 2);
    assert_eq!(doc.sessions[0].slug, "inside");
    assert_eq!(doc.sessions[0].messages.len(), 1);
    assert_eq!(doc.sessions[1].slug, "outside");
  }

  #[test]
  fn wait_body_range_bounded_by_next_heading() {
    let text = "\
## Session a: A
### WAIT
wait content
## Session b: B
";
    let doc = parse(text);

    let session_a = doc.session_by_slug("a").unwrap();
    let wait = session_a.wait.as_ref().unwrap();
    let body = &text[wait.body_byte_range.clone()];
    assert!(body.contains("wait content"));
    // Body should NOT contain the next session heading.
    assert!(!body.contains("## Session b"));
  }

  #[test]
  fn insert_session_heading_with_claude_section() {
    let text = "\
# TODO
some notes

# Claude
";
    let doc = parse(text);
    let result = insert_session_heading(text, &doc, "my-slug");
    assert!(result.contains("# Claude\n## Session my-slug: New session\n### WAIT\n"));
    // Verify it re-parses correctly.
    let new_doc = parse(&result);
    assert_eq!(new_doc.sessions.len(), 1);
    assert_eq!(new_doc.sessions[0].slug, "my-slug");
    assert!(new_doc.sessions[0].wait.is_some());
  }

  #[test]
  fn insert_session_heading_without_claude_section() {
    let text = "\
# TODO
some notes
";
    let doc = parse(text);
    let result = insert_session_heading(text, &doc, "test-slug");
    assert!(result.contains("# Claude\n## Session test-slug: New session\n### WAIT\n"));
    let new_doc = parse(&result);
    assert_eq!(new_doc.sessions.len(), 1);
    assert_eq!(new_doc.sessions[0].slug, "test-slug");
  }

  #[test]
  fn insert_session_heading_empty_document() {
    let text = "";
    let doc = parse(text);
    let result = insert_session_heading(text, &doc, "fresh");
    assert_eq!(result, "# Claude\n## Session fresh: New session\n### WAIT\n");
    let new_doc = parse(&result);
    assert_eq!(new_doc.sessions.len(), 1);
    assert_eq!(new_doc.sessions[0].slug, "fresh");
  }

  #[test]
  fn insert_session_heading_with_existing_sessions() {
    let text = "\
# Claude
## Session old: Old Session
### WAIT
notes
";
    let doc = parse(text);
    let result = insert_session_heading(text, &doc, "new-slug");
    // New heading should be inserted right after `# Claude`, before the old session.
    let new_doc = parse(&result);
    assert_eq!(new_doc.sessions.len(), 2);
    assert_eq!(new_doc.sessions[0].slug, "new-slug");
    assert_eq!(new_doc.sessions[1].slug, "old");
  }
}
