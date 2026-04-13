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

/// Session lifecycle state as marked in TODO.md headings.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
  /// Normal active session (no prefix).
  #[default]
  Active,
  /// Disabled/dormant — `[D]` prefix. Present but should not auto-attach.
  Disabled,
  /// Expired — `[X]` prefix. JSONL was garbage-collected by Claude.
  Expired,
}

#[derive(Debug, Default, Clone)]
pub struct TodoSession {
  pub uuid: String,
  pub label: String,
  pub status: SessionStatus,
  pub line: u32,
  pub heading_byte_range: Range<usize>,
  /// 1-based line number of the `> uuid=...` line.
  pub uuid_line: u32,
  /// Byte range of the uuid value within the full document text
  /// (i.e. the characters after `> uuid=`).
  pub uuid_byte_range: Range<usize>,
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
  UnsentWait { label: String },
}

// ---------------------------------------------------------------------------
// TodoDocument methods
// ---------------------------------------------------------------------------

impl TodoDocument {
  pub fn first_session(&self) -> Option<&TodoSession> {
    self.sessions.first()
  }

  pub fn session_by_uuid(&self, uuid: &str) -> Option<&TodoSession> {
    self.sessions.iter().find(|s| s.uuid == uuid)
  }

  pub fn session_by_label(&self, label: &str) -> Option<&TodoSession> {
    self.sessions.iter().find(|s| s.label == label)
  }

  pub fn session_uuids(&self) -> Vec<&str> {
    self.sessions.iter().map(|s| s.uuid.as_str()).collect()
  }

  /// Returns the 1-based line number of the last line of the WAIT body for the
  /// given session (by label). This is the line where a user would type new content.
  /// If the WAIT body is empty, returns the line right after the heading.
  pub fn wait_body_end_line(&self, label: &str, text: &str) -> Option<u32> {
    let wait = self.session_by_label(label)?.wait.as_ref()?;
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
  /// session (by label). This is where comments should be inserted.
  /// If the session has no WAIT, returns `None`.
  pub fn comment_insert_offset(&self, label: &str) -> Option<usize> {
    let session = self.session_by_label(label)?;
    let wait = session.wait.as_ref()?;
    Some(wait.body_byte_range.end)
  }

  /// Returns the byte offset where a session's content ends (the start of the
  /// next session heading, or end of document).
  pub fn session_end_offset(&self, label: &str, text_len: usize) -> Option<usize> {
    let idx = self.sessions.iter().position(|s| s.label == label)?;
    if idx + 1 < self.sessions.len() {
      Some(self.sessions[idx + 1].heading_byte_range.start)
    } else {
      Some(text_len)
    }
  }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

pub fn parse(text: &str) -> TodoDocument {
  let mut doc = TodoDocument::default();
  let mut current_session: Option<TodoSession> = None;
  let mut byte_offset: usize = 0;
  // State: we just saw an `## Label` heading and are looking for `> uuid=...` next.
  let mut expecting_uuid_for: Option<TodoSession> = None;
  // Only create sessions for `##` headings inside a `# Claude` section.
  let mut in_claude_section = false;

  for (line_idx, line) in text.lines().enumerate() {
    let line_num = line_idx as u32 + 1;
    let line_start = byte_offset;
    let line_end = line_start + line.len();

    // If we're expecting a `> uuid=` line after a heading:
    if let Some(ref mut pending) = expecting_uuid_for {
      if let Some(rest) = line.strip_prefix("> uuid=") {
        pending.uuid = rest.to_string();
        pending.uuid_line = line_num;
        let uuid_value_start = line_start + "> uuid=".len();
        pending.uuid_byte_range = uuid_value_start..line_end;
        // Promote to current session.
        finalize_session(&mut doc, &mut current_session, line_start);
        current_session = expecting_uuid_for.take();
      } else {
        // No uuid line — accept the session with an empty UUID.
        finalize_session(&mut doc, &mut current_session, line_start);
        current_session = expecting_uuid_for.take();
        // Fall through to normal parsing of this line.
      }
    }

    if expecting_uuid_for.is_some() {
      // Already handled above, skip normal parsing for this line.
    } else if line.starts_with("# ") {
      // Any top-level heading ends the current session and leaves the Claude section.
      finalize_session(&mut doc, &mut current_session, line_start);
      in_claude_section = line == "# Claude";

      if in_claude_section {
        doc.claude_section_line = Some(line_num);
      }
    } else if let Some(after_h2) = line.strip_prefix("## ") {
      // Any second-level heading ends the current session.
      finalize_session(&mut doc, &mut current_session, line_start);

      // Only treat `##` headings as sessions inside `# Claude`.
      if !in_claude_section {
        // Ignore — this heading is outside the Claude section.
      } else if after_h2.starts_with("[DELETED] ") {
        // Skip sessions marked as [DELETED].
      } else if !after_h2.is_empty() {
        // Check for [D] (disabled) or [X] (expired/GC'd) prefix.
        let (label, status) = if let Some(rest) = after_h2.strip_prefix("[D] ") {
          (rest.to_string(), SessionStatus::Disabled)
        } else if let Some(rest) = after_h2.strip_prefix("[X] ") {
          (rest.to_string(), SessionStatus::Expired)
        } else {
          (after_h2.to_string(), SessionStatus::Active)
        };
        expecting_uuid_for = Some(TodoSession {
          label,
          status,
          line: line_num,
          heading_byte_range: line_start..line_end,
          ..Default::default()
        });
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
// Insert WAIT section
// ---------------------------------------------------------------------------

/// Insert a `### WAIT\n` heading at the end of a session that lacks one.
/// Returns the new document text, or `None` if the session already has a WAIT.
pub fn insert_wait_section(text: &str, doc: &TodoDocument, label: &str) -> Option<String> {
  let session = doc.session_by_label(label)?;
  if let Some(wait) = &session.wait {
    // WAIT exists — ensure there is a blank line after the heading so the
    // cursor has somewhere to land.  If the body is empty and the next
    // character after the heading newline is not another newline, insert one.
    let after_heading = wait.heading_byte_range.end;
    // Skip past the newline that terminates the heading line itself.
    let body_start = if text.as_bytes().get(after_heading) == Some(&b'\n') {
      after_heading + 1
    } else {
      after_heading
    };
    // If the body is empty (or whitespace-only) and the very next byte is
    // not a newline, we need to insert a blank line.
    let body = &text[wait.body_byte_range.clone()];
    let needs_blank = body.trim().is_empty() && text.as_bytes().get(body_start) != Some(&b'\n');
    if !needs_blank {
      return None;
    }
    let mut new_text = String::with_capacity(text.len() + 1);
    new_text.push_str(&text[..body_start]);
    new_text.push('\n');
    new_text.push_str(&text[body_start..]);
    return Some(new_text);
  }
  let end = doc.session_end_offset(label, text.len())?;
  let mut new_text = String::with_capacity(text.len() + 16);
  new_text.push_str(&text[..end]);
  // Ensure a blank line before the heading.
  if !new_text.ends_with("\n\n") {
    if !new_text.ends_with('\n') {
      new_text.push('\n');
    }
    new_text.push('\n');
  }
  new_text.push_str("### WAIT\n");
  new_text.push_str(&text[end..]);
  Some(new_text)
}

// ---------------------------------------------------------------------------
// Session heading insertion
// ---------------------------------------------------------------------------

/// Build new text with a `## <label>\n> uuid=<uuid>\n\n### WAIT\n` heading inserted.
/// If a `# Claude` section exists, the heading goes right after it. Otherwise
/// a `# Claude` section is appended at the end of the text.
pub fn insert_session_heading(text: &str, doc: &TodoDocument, uuid: &str, label: &str) -> String {
  let heading = format!("## {label}\n> uuid={uuid}\n\n### WAIT\n");

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
// Session deletion marking
// ---------------------------------------------------------------------------

/// Mark a session as expired (`[X]` prefix) because its JSONL was garbage-collected.
/// Returns the modified text, or `None` if the label was not found or already expired.
pub fn mark_session_expired(text: &str, doc: &TodoDocument, label: &str) -> Option<String> {
  let session = doc.session_by_label(label)?;
  if session.status == SessionStatus::Expired {
    return None; // already marked
  }
  let range = session.heading_byte_range.clone();
  let heading = &text[range.clone()];
  // Remove [D] if present, then add [X].
  let new_heading = match session.status {
    SessionStatus::Disabled => {
      heading.replacen(&format!("## [D] {label}"), &format!("## [X] {label}"), 1)
    }
    _ => heading.replacen(&format!("## {label}"), &format!("## [X] {label}"), 1),
  };
  let mut new_text = String::with_capacity(text.len() + 4);
  new_text.push_str(&text[..range.start]);
  new_text.push_str(&new_heading);
  new_text.push_str(&text[range.end..]);
  Some(new_text)
}

/// Toggle the `[D]` (disabled/dormant) prefix on a session heading.
/// Returns the modified text, or `None` if the label was not found.
pub fn toggle_session_disabled(text: &str, doc: &TodoDocument, label: &str) -> Option<String> {
  let session = doc.session_by_label(label)?;
  let range = session.heading_byte_range.clone();
  let heading = &text[range.clone()];
  let new_heading = if session.status == SessionStatus::Disabled {
    // Remove [D] prefix: `## [D] Label` → `## Label`
    heading.replacen(&format!("## [D] {label}"), &format!("## {label}"), 1)
  } else {
    // Add [D] prefix: `## Label` → `## [D] Label`
    heading.replacen(&format!("## {label}"), &format!("## [D] {label}"), 1)
  };
  let mut new_text = String::with_capacity(text.len() + 4);
  new_text.push_str(&text[..range.start]);
  new_text.push_str(&new_heading);
  new_text.push_str(&text[range.end..]);
  Some(new_text)
}

// ---------------------------------------------------------------------------
// UUID update
// ---------------------------------------------------------------------------

/// Update a session's UUID in the document text. Returns the modified text,
/// or `None` if the session is not found.
pub fn update_session_uuid(
  text: &str,
  doc: &TodoDocument,
  label: &str,
  new_uuid: &str,
) -> Option<String> {
  let session = doc.session_by_label(label)?;
  if session.uuid_byte_range == (0..0) {
    return None;
  }
  let mut new_text = String::with_capacity(text.len() + new_uuid.len());
  new_text.push_str(&text[..session.uuid_byte_range.start]);
  new_text.push_str(new_uuid);
  new_text.push_str(&text[session.uuid_byte_range.end..]);
  Some(new_text)
}

// ---------------------------------------------------------------------------
// Send from WAIT
// ---------------------------------------------------------------------------

pub struct SendResult {
  pub new_text: String,
  pub message_text: String,
  pub message_index: usize,
  /// Byte offset of the first character after the new `### WAIT\n` heading.
  pub wait_body_offset: usize,
}

/// Extract text from the WAIT section and turn it into a new `### Message N`.
///
/// `selection` is a byte range in the full document. If it's empty (collapsed
/// cursor), everything before the cursor in the WAIT body is sent (or the
/// entire body if the cursor is outside/at the start of the body). Returns
/// `None` if there's no WAIT section or the effective text is empty.
pub fn send_from_wait(
  text: &str,
  session: &TodoSession,
  selection: Range<usize>,
) -> Option<SendResult> {
  let wait = session.wait.as_ref()?;
  let body_range = wait.body_byte_range.clone();

  // Determine the effective range within the body.
  let effective = if selection.start == selection.end {
    // No selection — send everything before the cursor (or the whole body if
    // the cursor is outside/at the start of the body).
    let cursor = selection.start;
    if cursor > body_range.start && cursor <= body_range.end {
      body_range.start..cursor
    } else {
      body_range.clone()
    }
  } else {
    // Intersect selection with the body range.
    let start = selection.start.max(body_range.start);
    let end = selection.end.min(body_range.end);
    if start >= end {
      return None;
    }
    start..end
  };

  let selected_text = text[effective.clone()].trim();
  if selected_text.is_empty() {
    return None;
  }
  let message_text = selected_text.to_string();

  // Compute next message index.
  let message_index = session.messages.iter().map(|m| m.index + 1).max().unwrap_or(0);

  // Build remaining body (parts of the body before and after the effective range).
  let before_sel = &text[body_range.start..effective.start];
  let after_sel = &text[effective.end..body_range.end];
  let remaining = format!("{}{}", before_sel, after_sel);

  // Rebuild the document:
  //   everything before WAIT heading
  //   + ### Message N\n{text}\n
  //   + ### WAIT\n{remaining}
  //   + everything after body end
  let before_wait = &text[..wait.heading_byte_range.start];
  let after_body = &text[body_range.end..];

  let mut new_text = String::with_capacity(text.len() + message_text.len() + 32);
  new_text.push_str(before_wait);
  new_text.push_str(&format!("### Message {}\n", message_index));
  new_text.push_str(&message_text);
  new_text.push('\n');
  new_text.push_str("### WAIT\n");
  let wait_body_offset = new_text.len();
  new_text.push_str(&remaining);
  new_text.push_str(after_body);

  Some(SendResult { new_text, message_text, message_index, wait_body_offset })
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

pub fn validate(doc: &TodoDocument, _project_path: &Path, text: &str) -> Vec<TodoProblem> {
  let mut problems = Vec::default();

  for session in &doc.sessions {
    if let Some(wait) = &session.wait {
      let body = &text[wait.body_byte_range.clone()];
      if !body.trim().is_empty() {
        problems.push(TodoProblem::UnsentWait { label: session.label.clone() });
      }
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
    assert!(doc.session_uuids().is_empty());
  }

  #[test]
  fn h2_outside_claude_section_ignored() {
    let text = "\
# APU
## Voices
some notes
## performance
more notes

# Claude
## Real Session
> uuid=abc

### WAIT
";
    let doc = parse(text);
    assert_eq!(doc.sessions.len(), 1);
    assert_eq!(doc.sessions[0].label, "Real Session");
    assert_eq!(doc.sessions[0].uuid, "abc");
  }

  #[test]
  fn single_session_with_messages_and_wait() {
    let text = "\
# Claude
## My Label
> uuid=abc-123

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
    assert_eq!(session.uuid, "abc-123");
    assert_eq!(session.label, "My Label");
    assert_eq!(session.line, 2);
    assert_eq!(session.messages.len(), 2);
    assert_eq!(session.messages[0].index, 0);
    assert_eq!(session.messages[1].index, 1);

    let wait = session.wait.as_ref().unwrap();

    // The WAIT body should contain "draft content here\n".
    let body = &text[wait.body_byte_range.clone()];
    assert!(body.contains("draft content here"));
  }

  #[test]
  fn multiple_sessions() {
    let text = "\
# Claude
## First Session
> uuid=aaa

### Message 0
body
## Second Session
> uuid=bbb

### Message 0
body
### WAIT
wait body
";
    let doc = parse(text);

    assert_eq!(doc.sessions.len(), 2);
    assert_eq!(doc.sessions[0].uuid, "aaa");
    assert_eq!(doc.sessions[0].label, "First Session");
    assert_eq!(doc.sessions[1].uuid, "bbb");
    assert_eq!(doc.sessions[1].label, "Second Session");
    assert!(doc.sessions[0].wait.is_none());
    assert!(doc.sessions[1].wait.is_some());
  }

  #[test]
  fn session_with_no_wait() {
    let text = "\
# Claude
## No Wait Here
> uuid=no-wait

### Message 0
body text
### Message 1
more body
";
    let doc = parse(text);

    assert_eq!(doc.sessions.len(), 1);
    let session = &doc.sessions[0];
    assert_eq!(session.uuid, "no-wait");
    assert!(session.wait.is_none());
    assert_eq!(session.messages.len(), 2);
  }

  #[test]
  fn session_with_no_messages() {
    let text = "\
# Claude
## Empty Session
> uuid=empty

### WAIT
some wait body
";
    let doc = parse(text);

    assert_eq!(doc.sessions.len(), 1);
    let session = &doc.sessions[0];
    assert_eq!(session.uuid, "empty");
    assert_eq!(session.label, "Empty Session");
    assert!(session.messages.is_empty());
    assert!(session.wait.is_some());
  }

  #[test]
  fn comment_insert_offset_returns_correct_byte_offset() {
    let text = "\
# Claude
## Test Session
> uuid=test

### WAIT
draft body
";
    let doc = parse(text);

    let offset = doc.comment_insert_offset("Test Session").unwrap();
    // The offset should be at the end of the WAIT body, which is the end of
    // the document.
    assert_eq!(offset, text.len());

    // Verify the body range content.
    let session = doc.session_by_label("Test Session").unwrap();
    let wait = session.wait.as_ref().unwrap();
    let body = &text[wait.body_byte_range.clone()];
    assert!(body.contains("draft body"));
  }

  #[test]
  fn session_by_uuid_and_session_uuids() {
    let text = "\
# Claude
## First
> uuid=aaa

### Message 0
## Second
> uuid=bbb

### WAIT
body
## Third
> uuid=ccc

";
    let doc = parse(text);

    assert_eq!(doc.session_uuids(), vec!["aaa", "bbb", "ccc"]);
    assert_eq!(doc.session_by_uuid("bbb").unwrap().label, "Second");
    assert!(doc.session_by_uuid("nonexistent").is_none());
  }

  #[test]
  fn first_session_returns_first() {
    let text = "\
# Claude
## The First
> uuid=first

## The Second
> uuid=second

";
    let doc = parse(text);

    let first = doc.first_session().unwrap();
    assert_eq!(first.uuid, "first");
    assert_eq!(first.label, "The First");
  }

  #[test]
  fn comment_insert_offset_none_without_wait() {
    let text = "\
# Claude
## No Wait
> uuid=no-wait

### Message 0
body
";
    let doc = parse(text);
    assert!(doc.comment_insert_offset("No Wait").is_none());
  }

  #[test]
  fn uuid_byte_range_covers_uuid_text() {
    let text = "# Claude\n## My Label\n> uuid=my-uuid\n";
    let doc = parse(text);
    let session = &doc.sessions[0];
    assert_eq!(&text[session.uuid_byte_range.clone()], "my-uuid");
  }

  #[test]
  fn heading_byte_range_covers_full_line() {
    let text = "# Claude\n## Test Label\n> uuid=test\nsome body\n";
    let doc = parse(text);
    let session = &doc.sessions[0];
    assert_eq!(&text[session.heading_byte_range.clone()], "## Test Label");
  }

  #[test]
  fn top_level_heading_ends_session() {
    let text = "\
# Claude
## Inside
> uuid=inside

### Message 0
# Other Section
## Outside
> uuid=outside

";
    let doc = parse(text);
    // `## Inside` is under `# Claude`, but `## Outside` is under `# Other Section`.
    assert_eq!(doc.sessions.len(), 1);
    assert_eq!(doc.sessions[0].uuid, "inside");
    assert_eq!(doc.sessions[0].messages.len(), 1);
  }

  #[test]
  fn wait_body_range_bounded_by_next_heading() {
    let text = "\
# Claude
## A
> uuid=a

### WAIT
wait content
## B
> uuid=b

";
    let doc = parse(text);

    let session_a = doc.session_by_label("A").unwrap();
    let wait = session_a.wait.as_ref().unwrap();
    let body = &text[wait.body_byte_range.clone()];
    assert!(body.contains("wait content"));
    // Body should NOT contain the next session heading.
    assert!(!body.contains("## B"));
  }

  #[test]
  fn insert_session_heading_with_claude_section() {
    let text = "\
# TODO
some notes

# Claude
";
    let doc = parse(text);
    let result = insert_session_heading(text, &doc, "my-uuid", "My Label");
    assert!(result.contains("# Claude\n## My Label\n> uuid=my-uuid\n"));
    // Verify it re-parses correctly.
    let new_doc = parse(&result);
    assert_eq!(new_doc.sessions.len(), 1);
    assert_eq!(new_doc.sessions[0].uuid, "my-uuid");
    assert!(new_doc.sessions[0].wait.is_some());
  }

  #[test]
  fn insert_session_heading_without_claude_section() {
    let text = "\
# TODO
some notes
";
    let doc = parse(text);
    let result = insert_session_heading(text, &doc, "test-uuid", "Test Label");
    assert!(result.contains("# Claude\n## Test Label\n> uuid=test-uuid\n"));
    let new_doc = parse(&result);
    assert_eq!(new_doc.sessions.len(), 1);
    assert_eq!(new_doc.sessions[0].uuid, "test-uuid");
  }

  #[test]
  fn insert_session_heading_empty_document() {
    let text = "";
    let doc = parse(text);
    let result = insert_session_heading(text, &doc, "", "Fresh");
    assert_eq!(result, "# Claude\n## Fresh\n> uuid=\n\n### WAIT\n");
    let new_doc = parse(&result);
    assert_eq!(new_doc.sessions.len(), 1);
    assert_eq!(new_doc.sessions[0].uuid, "");
    assert_eq!(new_doc.sessions[0].label, "Fresh");
  }

  #[test]
  fn insert_session_heading_with_existing_sessions() {
    let text = "\
# Claude
## Old Session
> uuid=old

### WAIT
notes
";
    let doc = parse(text);
    let result = insert_session_heading(text, &doc, "new-uuid", "New Label");
    // New heading should be inserted right after `# Claude`, before the old session.
    let new_doc = parse(&result);
    assert_eq!(new_doc.sessions.len(), 2);
    assert_eq!(new_doc.sessions[0].uuid, "new-uuid");
    assert_eq!(new_doc.sessions[1].uuid, "old");
  }

  // -------------------------------------------------------------------------
  // send_from_wait tests
  // -------------------------------------------------------------------------

  #[test]
  fn send_from_wait_basic() {
    let text = "\
# Claude
## S
> uuid=s

### Message 0
hello
### WAIT
draft text
";
    let doc = parse(text);
    let session = doc.session_by_label("S").unwrap();
    let wait = session.wait.as_ref().unwrap();

    // Select just "draft text" within the body.
    let body_start = wait.body_byte_range.start;
    let sel_start = body_start + text[body_start..].find("draft text").unwrap();
    let sel_end = sel_start + "draft text".len();

    let result = send_from_wait(text, session, sel_start..sel_end).unwrap();
    assert_eq!(result.message_text, "draft text");
    assert_eq!(result.message_index, 1);
    assert!(result.new_text.contains("### Message 1\ndraft text\n### WAIT\n"));

    // Re-parse to verify structure.
    let new_doc = parse(&result.new_text);
    let new_session = new_doc.session_by_label("S").unwrap();
    assert_eq!(new_session.messages.len(), 2);
    assert!(new_session.wait.is_some());
  }

  #[test]
  fn send_from_wait_no_selection_sends_all() {
    let text = "\
# Claude
## S
> uuid=s

### WAIT
all body content
";
    let doc = parse(text);
    let session = doc.session_by_label("S").unwrap();

    // Empty selection (collapsed cursor) → send entire body.
    let result = send_from_wait(text, session, 0..0).unwrap();
    assert_eq!(result.message_text, "all body content");
    assert_eq!(result.message_index, 0);
    assert!(result.new_text.contains("### Message 0\nall body content\n### WAIT\n"));
  }

  #[test]
  fn send_from_wait_partial_selection() {
    let text = "\
# Claude
## S
> uuid=s

### WAIT
line one
line two
line three
";
    let doc = parse(text);
    let session = doc.session_by_label("S").unwrap();
    let wait = session.wait.as_ref().unwrap();
    let body = &text[wait.body_byte_range.clone()];

    // Select just "line two".
    let offset_in_body = body.find("line two").unwrap();
    let sel_start = wait.body_byte_range.start + offset_in_body;
    let sel_end = sel_start + "line two".len();

    let result = send_from_wait(text, session, sel_start..sel_end).unwrap();
    assert_eq!(result.message_text, "line two");

    // Remaining body should have line one and line three.
    let new_doc = parse(&result.new_text);
    let new_wait = new_doc.session_by_label("S").unwrap().wait.as_ref().unwrap();
    let new_body = &result.new_text[new_wait.body_byte_range.clone()];
    assert!(new_body.contains("line one"));
    assert!(new_body.contains("line three"));
    assert!(!new_body.contains("line two"));
  }

  #[test]
  fn send_from_wait_cursor_sends_before_cursor() {
    let text = "\
# Claude
## S
> uuid=s

### WAIT
one two three
";
    let doc = parse(text);
    let session = doc.session_by_label("S").unwrap();
    let wait = session.wait.as_ref().unwrap();
    // Place cursor after "one two" — find the exact position.
    let body = &text[wait.body_byte_range.clone()];
    let offset_in_body = body.find(" three").unwrap();
    let cursor = wait.body_byte_range.start + offset_in_body;
    let result = send_from_wait(text, session, cursor..cursor).unwrap();
    assert_eq!(result.message_text, "one two");
    // The remaining text ("three\n") stays in the WAIT body.
    let new_doc = parse(&result.new_text);
    let new_session = new_doc.session_by_label("S").unwrap();
    let new_body = &result.new_text[new_session.wait.as_ref().unwrap().body_byte_range.clone()];
    assert_eq!(new_body.trim(), "three");
  }

  #[test]
  fn send_from_wait_cursor_multiline() {
    let text = "\
# Claude
## S
> uuid=s

### WAIT
line one
line two
line three
";
    let doc = parse(text);
    let session = doc.session_by_label("S").unwrap();
    let wait = session.wait.as_ref().unwrap();
    // Place cursor at the start of "line two" — should send "line one".
    let body = &text[wait.body_byte_range.clone()];
    let offset_in_body = body.find("line two").unwrap();
    let cursor = wait.body_byte_range.start + offset_in_body;
    let result = send_from_wait(text, session, cursor..cursor).unwrap();
    assert_eq!(result.message_text, "line one");
    let new_doc = parse(&result.new_text);
    let new_session = new_doc.session_by_label("S").unwrap();
    let new_body = &result.new_text[new_session.wait.as_ref().unwrap().body_byte_range.clone()];
    assert!(new_body.contains("line two"));
    assert!(new_body.contains("line three"));
  }

  #[test]
  fn send_from_wait_empty_body() {
    let text = "\
# Claude
## S
> uuid=s

### WAIT
";
    let doc = parse(text);
    let session = doc.session_by_label("S").unwrap();

    // Empty body → should return None.
    assert!(send_from_wait(text, session, 0..0).is_none());
  }

  #[test]
  fn send_from_wait_no_wait_section() {
    let text = "\
# Claude
## S
> uuid=s

### Message 0
hello
";
    let doc = parse(text);
    let session = doc.session_by_label("S").unwrap();
    assert!(send_from_wait(text, session, 0..0).is_none());
  }

  #[test]
  fn send_from_wait_multiple_messages() {
    let text = "\
# Claude
## S
> uuid=s

### Message 0
first
### Message 1
second
### WAIT
third
";
    let doc = parse(text);
    let session = doc.session_by_label("S").unwrap();
    let result = send_from_wait(text, session, 0..0).unwrap();
    assert_eq!(result.message_index, 2);
    assert_eq!(result.message_text, "third");
  }

  #[test]
  fn disabled_sessions_are_parsed_with_flag() {
    let text = "\
# Claude
## [D] Dormant Session
> uuid=dormant

### WAIT
## Active Session
> uuid=active

### WAIT
";
    let doc = parse(text);
    assert_eq!(doc.sessions.len(), 2);
    assert_eq!(doc.sessions[0].label, "Dormant Session");
    assert!(doc.sessions[0].status == SessionStatus::Disabled);
    assert_eq!(doc.sessions[0].uuid, "dormant");
    assert_eq!(doc.sessions[1].label, "Active Session");
    assert_ne!(doc.sessions[1].status, SessionStatus::Disabled);
  }

  #[test]
  fn toggle_session_disabled_roundtrip() {
    let text = "\
# Claude
## My Session
> uuid=my-uuid

### WAIT
";
    let doc = parse(text);
    assert_ne!(doc.sessions[0].status, SessionStatus::Disabled);

    // Disable it.
    let disabled_text = toggle_session_disabled(text, &doc, "My Session").unwrap();
    assert!(disabled_text.contains("## [D] My Session"));
    let doc2 = parse(&disabled_text);
    assert_eq!(doc2.sessions[0].label, "My Session");
    assert!(doc2.sessions[0].status == SessionStatus::Disabled);

    // Re-enable it.
    let enabled_text = toggle_session_disabled(&disabled_text, &doc2, "My Session").unwrap();
    assert!(enabled_text.contains("## My Session"));
    assert!(!enabled_text.contains("[D]"));
    let doc3 = parse(&enabled_text);
    assert_ne!(doc3.sessions[0].status, SessionStatus::Disabled);
  }

  #[test]
  fn bracket_deleted_sessions_are_skipped() {
    let text = "\
# Claude
## [DELETED] Old Label
> uuid=old

### WAIT
## Active Session
> uuid=active

### WAIT
";
    let doc = parse(text);
    assert_eq!(doc.sessions.len(), 1);
    assert_eq!(doc.sessions[0].label, "Active Session");
  }

  #[test]
  fn deleted_sessions_are_skipped() {
    let text = "\
# Claude
## [DELETED] Old Label
> uuid=old

### WAIT
stale draft
## Active Session
> uuid=active

### WAIT
";
    let doc = parse(text);
    assert_eq!(doc.sessions.len(), 1);
    assert_eq!(doc.sessions[0].label, "Active Session");
  }

  #[test]
  fn expired_sessions_are_parsed_with_flag() {
    let text = "\
# Claude
## [X] GC'd Session
> uuid=gone
## Active Session
> uuid=active
";
    let doc = parse(text);
    assert_eq!(doc.sessions.len(), 2);
    assert_eq!(doc.sessions[0].label, "GC'd Session");
    assert_eq!(doc.sessions[0].status, SessionStatus::Expired);
    assert_eq!(doc.sessions[1].label, "Active Session");
    assert_eq!(doc.sessions[1].status, SessionStatus::Active);
  }

  #[test]
  fn mark_session_expired_adds_prefix() {
    let text = "\
# Claude
## My Session
> uuid=my-uuid
### WAIT
";
    let doc = parse(text);
    assert_eq!(doc.sessions[0].status, SessionStatus::Active);

    let expired_text = mark_session_expired(text, &doc, "My Session").unwrap();
    assert!(expired_text.contains("## [X] My Session"));
    let doc2 = parse(&expired_text);
    assert_eq!(doc2.sessions[0].status, SessionStatus::Expired);
    assert_eq!(doc2.sessions[0].label, "My Session");

    // Already expired — returns None.
    assert!(mark_session_expired(&expired_text, &doc2, "My Session").is_none());
  }

  #[test]
  fn mark_disabled_session_expired_replaces_prefix() {
    let text = "\
# Claude
## [D] Dormant Session
> uuid=old-uuid
";
    let doc = parse(text);
    assert_eq!(doc.sessions[0].status, SessionStatus::Disabled);

    let expired_text = mark_session_expired(text, &doc, "Dormant Session").unwrap();
    assert!(expired_text.contains("## [X] Dormant Session"));
    assert!(!expired_text.contains("[D]"));
    let doc2 = parse(&expired_text);
    assert_eq!(doc2.sessions[0].status, SessionStatus::Expired);
  }

  #[test]
  fn send_from_wait_selection_outside_body() {
    let text = "\
# Claude
## S
> uuid=s

### Message 0
hello
### WAIT
draft
";
    let doc = parse(text);
    let session = doc.session_by_label("S").unwrap();

    // Selection entirely before the WAIT body.
    assert!(send_from_wait(text, session, 0..5).is_none());
  }

  #[test]
  fn blank_uuid_session() {
    let text = "\
# Claude
## New Session
> uuid=

### WAIT
";
    let doc = parse(text);
    assert_eq!(doc.sessions.len(), 1);
    let session = &doc.sessions[0];
    assert_eq!(session.uuid, "");
    assert_eq!(session.label, "New Session");
  }

  #[test]
  fn update_session_uuid_works() {
    let text = "\
# Claude
## My Session
> uuid=old-uuid

### WAIT
";
    let doc = parse(text);
    let updated = update_session_uuid(text, &doc, "My Session", "new-uuid").unwrap();
    assert!(updated.contains("> uuid=new-uuid"));
    let new_doc = parse(&updated);
    assert_eq!(new_doc.sessions[0].uuid, "new-uuid");
  }
}
