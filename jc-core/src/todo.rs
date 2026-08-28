#[cfg(test)]
use chrono::NaiveDate;
use chrono::{NaiveDateTime, NaiveTime, Timelike};
use std::borrow::Cow;
use std::ops::Range;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct TodoDocument {
  pub claude_section_line: Option<u32>,
  pub sessions: Vec<TodoSession>,
}

/// How to locate a session's heading in TODO.md.
///
/// **This is the canonical note on session addressing; other sites point here.**
///
/// **A label is never an address.** TODO.md is a file the user edits by hand, so
/// two headings can share a label at any instant — paste a second
/// `## New Session` block and nothing has run in between. Every label lookup
/// takes the FIRST heading that matches, so keying a *write* on one silently
/// redirects a send, or a scheduled `@jc(...)` delivery, to the wrong session.
/// Every text operation ([`insert_wait_section`], [`fire_scheduled`],
/// the WAIT-cursor, highlight and send paths in the app) therefore keys on the
/// UUID, which is unique.
///
/// The one heading a UUID cannot address is one an older jc created and never
/// bound: its `> uuid=` is empty, and the empty string names every such heading
/// at once. That case is addressed by POSITION, and the position is *verified*
/// at write time — [`TodoDocument::index_of`] re-checks that the heading at that
/// index is still unbound and still carries the label the key was taken with,
/// and refuses otherwise. That check reads the live document, so it holds
/// continuously rather than resting on a one-shot repair pass; it is also what
/// makes the key survive the window between a picker snapshot and its confirm.
///
/// So this type is *not* the universal address. An operation that can only act
/// on a bound heading takes the UUID directly as a `&str`; `SessionKey` exists
/// for the two picker-originated operations that may name an unbound heading
/// (`Workspace::adopt_session` and `Workspace::toggle_session_disabled`), and
/// `Unbound` has no meaning outside them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionKey {
  /// A bound heading, addressed by its (unique) UUID.
  Uuid(String),
  /// A heading with an empty `> uuid=`, addressed by its position in
  /// [`TodoDocument::sessions`] plus the label it carried when the key was taken.
  Unbound { index: usize, label: String },
}

impl SessionKey {
  /// The address of the heading at `index`: its UUID, or its verified position
  /// when the heading is unbound.
  pub fn new(uuid: &str, index: usize, label: &str) -> Self {
    if uuid.is_empty() {
      Self::Unbound { index, label: label.to_string() }
    } else {
      Self::Uuid(uuid.to_string())
    }
  }
}

/// Session lifecycle state as marked in TODO.md headings.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
  /// Normal active session (no prefix).
  #[default]
  Active,
  /// Disabled/dormant — `[D]` prefix. Present but should not auto-attach.
  Disabled,
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
  /// Unix timestamp (seconds) of the last TODO submit, parsed from `> last=`.
  pub last_active: Option<u64>,
  /// Byte range of the entire `> last=TIMESTAMP` line (for replacement).
  pub last_active_line_range: Option<Range<usize>>,
  /// True if the session has a `> dangerous` metadata line — spawn `claude`
  /// with `--dangerously-skip-permissions`.
  pub dangerous: bool,
  pub messages: Vec<TodoMessage>,
  pub wait: Option<TodoWait>,
}

#[derive(Debug, Default, Clone)]
pub struct TodoMessage {
  pub index: usize,
  pub line: u32,
  pub heading_byte_range: Range<usize>,
  /// Byte range of the message body (text after the heading line up to the next
  /// heading). `0..0` until finalized by the parser.
  pub body_byte_range: Range<usize>,
  /// Set when the heading carries a pending `@jc(<datetime>)` scheduled-send
  /// marker. `None` once delivered (marker dropped) or for a normal message.
  pub schedule: Option<NaiveDateTime>,
}

#[derive(Debug, Default, Clone)]
pub struct TodoWait {
  pub line: u32,
  pub heading_byte_range: Range<usize>,
  pub body_byte_range: Range<usize>,
}

impl TodoWait {
  /// 0-based line number of the last line within the WAIT body.
  ///
  /// Backs up past a trailing newline so the result points at the last body
  /// line rather than the next heading.
  pub fn body_end_line(&self, text: &str) -> u32 {
    let mut end = self.body_byte_range.end.min(text.len());
    if end > self.body_byte_range.start && text.as_bytes()[end - 1] == b'\n' {
      end -= 1;
    }
    text[..end].bytes().filter(|&b| b == b'\n').count() as u32
  }
}

impl TodoSession {
  /// How to address this heading, given its position in
  /// [`TodoDocument::sessions`] — its UUID, or that verified position when it is
  /// unbound. See [`SessionKey`].
  pub fn key(&self, index: usize) -> SessionKey {
    SessionKey::new(&self.uuid, index, &self.label)
  }

  /// Whether the heading carries the `[D]` (disabled/dormant) prefix — or the
  /// legacy `[X]`, which [`parse`] reads as the same state and
  /// [`toggle_session_disabled_at`] normalises away on the next toggle.
  pub fn is_disabled(&self) -> bool {
    self.status == SessionStatus::Disabled
  }

  /// The session's pending scheduled message, if any (a `### Message N`
  /// heading carrying an undelivered `@jc(<datetime>)` marker). At most one
  /// exists at a time — sends are blocked while one is pending.
  pub fn pending_scheduled(&self) -> Option<&TodoMessage> {
    self.messages.iter().find(|m| m.schedule.is_some())
  }
}

// ---------------------------------------------------------------------------
// TodoDocument methods
// ---------------------------------------------------------------------------

impl TodoDocument {
  pub fn first_session(&self) -> Option<&TodoSession> {
    self.sessions.first()
  }

  /// The session bound to `uuid`. An empty `uuid` names no session — see
  /// [`SessionKey`].
  pub fn session_by_uuid(&self, uuid: &str) -> Option<&TodoSession> {
    self.sessions.get(self.index_by_uuid(uuid)?)
  }

  /// Position of the session owning `uuid` in [`Self::sessions`], for the
  /// `*_at` writers. UUIDs are unique where labels are not, so anything
  /// rewriting one specific session should address it this way.
  ///
  /// An empty `uuid` is not an address — several legacy headings can share it —
  /// so it never matches. Use [`Self::index_of`] with a [`SessionKey`] when the
  /// heading may be unbound.
  pub fn index_by_uuid(&self, uuid: &str) -> Option<usize> {
    if uuid.is_empty() {
      return None;
    }
    self.sessions.iter().position(|s| s.uuid == uuid)
  }

  /// Position of the session `key` names, or `None` if the document has moved
  /// out from under an [`SessionKey::Unbound`] key — see [`SessionKey`].
  pub fn index_of(&self, key: &SessionKey) -> Option<usize> {
    match key {
      SessionKey::Uuid(uuid) => self.index_by_uuid(uuid),
      SessionKey::Unbound { index, label } => {
        let session = self.sessions.get(*index)?;
        (session.uuid.is_empty() && session.label == *label).then_some(*index)
      }
    }
  }

  /// The session `key` names.
  pub fn session_of(&self, key: &SessionKey) -> Option<&TodoSession> {
    self.sessions.get(self.index_of(key)?)
  }

  pub fn session_uuids(&self) -> Vec<&str> {
    self.sessions.iter().map(|s| s.uuid.as_str()).collect()
  }

  /// Byte offset where the session at `index` ends (the start of the next
  /// session heading, or end of document).
  fn session_end_offset(&self, index: usize, text_len: usize) -> usize {
    self.sessions.get(index + 1).map_or(text_len, |s| s.heading_byte_range.start)
  }
}

/// Fixture lookup for tests. **Not an address** — two headings can share a label
/// and this takes the first (see [`SessionKey`]); production code keys on the
/// UUID.
#[cfg(test)]
impl TodoDocument {
  pub fn session_by_label(&self, label: &str) -> Option<&TodoSession> {
    self.sessions.iter().find(|s| s.label == label)
  }
}

/// Advance past a single `\n` or `\r\n` at `offset`, returning the new offset.
fn skip_newline(bytes: &[u8], offset: usize) -> usize {
  if offset < bytes.len() && bytes[offset] == b'\n' {
    offset + 1
  } else if offset < bytes.len() && bytes[offset] == b'\r' {
    let past_cr = offset + 1;
    if past_cr < bytes.len() && bytes[past_cr] == b'\n' { past_cr + 1 } else { past_cr }
  } else {
    offset
  }
}

// ---------------------------------------------------------------------------
// Scheduled-send markers
// ---------------------------------------------------------------------------

/// Maximum number of `### Message N` entries kept per session.
///
/// Each send drops the oldest beyond this, so a long-running session settles
/// into a sliding window -- `### Message 76` through `### Message 100`. Indices
/// are never renumbered, so a message keeps the number it was sent under for as
/// long as it is in the log.
pub const MAX_MESSAGES: usize = 25;

/// Format used to store a resolved schedule on a `### Message N @jc(...)` heading.
const SCHEDULE_FMT: &str = "%Y-%m-%d %H:%M";

/// Render a resolved schedule for a Message heading marker, e.g. `2026-07-13 07:30`.
pub fn format_schedule(dt: NaiveDateTime) -> String {
  dt.format(SCHEDULE_FMT).to_string()
}

/// Parse a stored `@jc(<datetime>)` marker value back into a datetime.
pub fn parse_schedule_datetime(s: &str) -> Option<NaiveDateTime> {
  NaiveDateTime::parse_from_str(s, SCHEDULE_FMT).ok()
}

/// Detect a leading `@jc(HH:MM)` schedule marker (24h) at the start of `s`
/// (ignoring leading whitespace). Returns `(hour, minute, token_end)` where
/// `token_end` is the byte offset in `s` just past the closing `)`.
pub fn parse_schedule_prefix(s: &str) -> Option<(u32, u32, usize)> {
  let lead = s.len() - s.trim_start().len();
  let inner = s[lead..].strip_prefix("@jc(")?;
  let close = inner.find(')')?;
  let (hh, mm) = inner[..close].split_once(':')?;
  let hour: u32 = hh.trim().parse().ok()?;
  let minute: u32 = mm.trim().parse().ok()?;
  if hour > 23 || minute > 59 {
    return None;
  }
  let token_end = lead + "@jc(".len() + close + 1;
  Some((hour, minute, token_end))
}

/// Resolve an `HH:MM` time to the next wall-clock occurrence at minute
/// granularity: today if this minute hasn't yet passed (so `@jc(07:30)` typed at
/// 07:30:40 still fires today, not ~24h later), otherwise tomorrow.
pub fn resolve_schedule(hour: u32, minute: u32, now: NaiveDateTime) -> Option<NaiveDateTime> {
  let time = NaiveTime::from_hms_opt(hour, minute, 0)?;
  let candidate = NaiveDateTime::new(now.date(), time);
  // Compare against `now` truncated to the minute so the current minute counts
  // as "not yet passed".
  let now_minute = NaiveDateTime::new(now.date(), now.time().with_second(0)?.with_nanosecond(0)?);
  if candidate >= now_minute {
    Some(candidate)
  } else {
    Some(NaiveDateTime::new(now.date().succ_opt()?, time))
  }
}

/// Parse the text after `### Message ` into `(index, schedule)`. A malformed
/// `@jc(...)` marker is ignored (yielding `schedule = None`) rather than
/// dropping the message, so body text is never misattributed.
fn parse_message_heading(rest: &str) -> Option<(usize, Option<NaiveDateTime>)> {
  let rest = rest.trim_end();
  let (num_part, schedule) = match rest.split_once(" @jc(") {
    Some((num, after)) => {
      let dt = after.strip_suffix(')').and_then(|v| parse_schedule_datetime(v.trim()));
      (num.trim(), dt)
    }
    None => (rest.trim(), None),
  };
  let n = num_part.parse::<usize>().ok()?;
  Some((n, schedule))
}

/// Rewrite a scheduled message's heading to drop its `@jc(...)` marker, turning
/// `### Message N @jc(<datetime>)` into a plain `### Message N` on delivery.
pub fn drop_schedule_marker(text: &str, message: &TodoMessage) -> String {
  let mut new_text = String::with_capacity(text.len());
  new_text.push_str(&text[..message.heading_byte_range.start]);
  new_text.push_str(&format!("### Message {}", message.index));
  new_text.push_str(&text[message.heading_byte_range.end..]);
  new_text
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
  // State: we're inside a session's metadata block (after `> uuid=`); each line
  // starting with `> ` is parsed as metadata until we hit a non-metadata line.
  let mut in_metadata = false;
  // Only create sessions for `##` headings inside a `# Claude` section.
  let mut in_claude_section = false;

  for (line_idx, line) in text.lines().enumerate() {
    let line_num = line_idx as u32 + 1;
    let line_start = byte_offset;
    let line_end = line_start + line.len();

    // Inside a session's metadata block, parse `> key[=value]` lines.
    if in_metadata
      && let Some(ref mut session) = current_session
      && let Some(rest) = line.strip_prefix("> ")
    {
      if let Some(ts_str) = rest.strip_prefix("last=") {
        if let Ok(ts) = ts_str.parse::<u64>() {
          session.last_active = Some(ts);
        }
        session.last_active_line_range = Some(line_start..line_end);
      } else if rest == "dangerous" {
        session.dangerous = true;
      }
      // Unknown metadata keys are silently consumed so future additions
      // don't break older parsers.
      byte_offset = skip_newline(text.as_bytes(), line_end);
      continue;
    }
    in_metadata = false;

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
        in_metadata = true;
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
        // `[D]` marks a dormant session. `[X]` is a retired marker jc used to
        // write when Claude garbage-collected a transcript; `--resume` revives
        // such a UUID, so those sessions are merely dormant, not gone. Read the
        // legacy marker as `[D]` and never write it again.
        let (label, status) = match after_h2.strip_prefix("[D] ").or(after_h2.strip_prefix("[X] "))
        {
          Some(rest) => (rest.to_string(), SessionStatus::Disabled),
          None => (after_h2.to_string(), SessionStatus::Active),
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
          // Close any open Message body and any previous WAIT body range.
          finalize_message_body(session, line_start);
          finalize_wait_body(session, line_start);

          session.wait = Some(TodoWait {
            line: line_num,
            heading_byte_range: line_start..line_end,
            body_byte_range: 0..0, // will be finalized later
          });
        }
      } else if let Some(rest) = after_h3.strip_prefix("Message ")
        && let Some((n, schedule)) = parse_message_heading(rest)
        && let Some(ref mut session) = current_session
        // Everything below `### WAIT` is draft, not log. A `### Message N` line
        // there is the user quoting an old message, and reading it as a real
        // message of this session makes the next send take its index, lets a
        // quoted `@jc(...)` marker block sends outright, and puts draft text
        // inside the range the log truncator is allowed to delete.
        && session.wait.is_none()
      {
        // Close the previous message's body before starting a new one.
        finalize_message_body(session, line_start);
        session.messages.push(TodoMessage {
          index: n,
          line: line_num,
          heading_byte_range: line_start..line_end,
          body_byte_range: 0..0, // will be finalized later
          schedule,
        });
      }
    }

    byte_offset = skip_newline(text.as_bytes(), line_end);
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
    finalize_message_body(&mut session, boundary);
    finalize_wait_body(&mut session, boundary);
    doc.sessions.push(session);
  }
}

/// Close the most recent message's body range (from the end of its heading line
/// to `boundary`) if it hasn't been finalized yet.
fn finalize_message_body(session: &mut TodoSession, boundary: usize) {
  if let Some(msg) = session.messages.last_mut()
    && msg.body_byte_range == (0..0)
  {
    msg.body_byte_range = msg.heading_byte_range.end..boundary;
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
pub fn insert_wait_section(text: &str, doc: &TodoDocument, uuid: &str) -> Option<String> {
  let index = doc.index_by_uuid(uuid)?;
  let session = &doc.sessions[index];
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
  let end = doc.session_end_offset(index, text.len());
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

/// A display name not already used by any session in `doc`: `base`, else
/// `base 2`, `base 3`, ...
///
/// **Cosmetic only.** Nothing addresses a session by label (see [`SessionKey`]),
/// so a duplicate is harmless to correctness — this exists so the session picker
/// doesn't show several identical rows. Never reintroduce an invariant that
/// depends on it.
pub fn unique_label(doc: &TodoDocument, base: &str) -> String {
  let taken = |candidate: &str| doc.sessions.iter().any(|s| s.label == candidate);
  if !taken(base) {
    return base.to_string();
  }
  (2..).map(|n| format!("{base} {n}")).find(|c| !taken(c)).unwrap()
}

/// Build new text with a `## <label>\n> uuid=<uuid>\n\n### WAIT\n` heading inserted.
/// If a `# Claude` section exists, the heading goes right after it. Otherwise
/// a `# Claude` section is appended at the end of the text.
///
/// `label` need not be unique: it is a display name, not an address (see
/// [`SessionKey`]).
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
// Session disable toggle
// ---------------------------------------------------------------------------

/// `text` with `range` replaced by `replacement`.
fn splice(text: &str, range: Range<usize>, replacement: &str) -> String {
  let mut out = text.to_string();
  out.replace_range(range, replacement);
  out
}

/// Toggle the `[D]` (disabled/dormant) prefix on the session at `index`.
/// Returns the modified text, or `None` if the index is out of range.
pub fn toggle_session_disabled_at(text: &str, doc: &TodoDocument, index: usize) -> Option<String> {
  let session = doc.sessions.get(index)?;
  let label = &session.label;
  // Rebuild the heading from the parsed label rather than patching the existing
  // text, so a legacy `## [X] Label` normalises to `## Label` / `## [D] Label`.
  let new_heading =
    if session.is_disabled() { format!("## {label}") } else { format!("## [D] {label}") };
  Some(splice(text, session.heading_byte_range.clone(), &new_heading))
}

// ---------------------------------------------------------------------------
// UUID update
// ---------------------------------------------------------------------------

/// Write `new_uuid` onto the session at `index` in `doc.sessions`. Positional
/// rather than label-keyed — see [`SessionKey`]. Callers holding a UUID resolve
/// it with [`TodoDocument::index_by_uuid`].
pub fn update_session_uuid_at(
  text: &str,
  doc: &TodoDocument,
  index: usize,
  new_uuid: &str,
) -> Option<String> {
  let session = doc.sessions.get(index)?;
  // A hand-written heading may have no `> uuid=` line at all; give it one
  // directly below the heading rather than dropping the UUID on the floor.
  if session.uuid_byte_range == (0..0) {
    let at = session.heading_byte_range.end;
    return Some(splice(text, at..at, &format!("\n> uuid={new_uuid}")));
  }
  Some(splice(text, session.uuid_byte_range.clone(), new_uuid))
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
  /// When the effective WAIT text began with a `@jc(HH:MM)` marker, the resolved
  /// delivery time. The message was recorded as `### Message N @jc(<datetime>)`
  /// and must NOT be delivered until this instant; `None` for an immediate send.
  pub schedule: Option<NaiveDateTime>,
}

/// Extract text from the WAIT section and turn it into a new `### Message N`.
///
/// `selection` is a byte range in the full document. If it's empty (collapsed
/// cursor), everything before the cursor in the WAIT body is sent (or the
/// entire body if the cursor is outside/at the start of the body). Returns
/// `None` if there's no WAIT section or the effective text is empty.
///
/// If the effective text begins with a `@jc(HH:MM)` marker, the message is
/// recorded as a pending scheduled `### Message N @jc(<datetime>)` (resolved
/// against `now_local`) with the marker stripped from the body, and `> last=`
/// is left untouched (it's set at delivery time, not here).
pub fn send_from_wait(
  text: &str,
  session: &TodoSession,
  selection: Range<usize>,
  timestamp: Option<u64>,
  now_local: NaiveDateTime,
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

  // A leading `@jc(HH:MM)` turns this into a scheduled send: strip the marker
  // from the message body and resolve the delivery time.
  let (body, schedule) = match parse_schedule_prefix(selected_text)
    .and_then(|(h, m, tok_end)| resolve_schedule(h, m, now_local).map(|dt| (dt, tok_end)))
  {
    Some((dt, tok_end)) => (selected_text[tok_end..].trim(), Some(dt)),
    None => (selected_text, None),
  };
  if body.is_empty() {
    return None;
  }
  let message_text = body.to_string();

  // Compute next message index.
  let message_index = session.messages.iter().map(|m| m.index + 1).max().unwrap_or(0);

  // Build remaining body (parts of the body before and after the effective range).
  let before_sel = &text[body_range.start..effective.start];
  let after_sel = &text[effective.end..body_range.end];
  let remaining = format!("{}{}", before_sel, after_sel);

  // Rebuild the document:
  //   everything before WAIT heading (with optional `> last=` update)
  //   + ### Message N\n{text}\n
  //   + ### WAIT\n{remaining}
  //   + everything after body end
  // Bound the log. The span to drop is a function of the pre-edit document and
  // lies entirely above the WAIT heading, so cutting it here leaves every header
  // offset used below (the `> last=` line, the `> uuid=` insert point) valid,
  // and `wait_body_offset` is measured against the already-shortened text.
  // `keep` is one short of the bound to leave room for the message being added.
  let before_wait = &text[..wait.heading_byte_range.start];
  let before_wait = match stale_log_span(session, MAX_MESSAGES.saturating_sub(1)) {
    Some(stale) if stale.end <= before_wait.len() => {
      Cow::Owned(format!("{}{}", &before_wait[..stale.start], &before_wait[stale.end..]))
    }
    _ => Cow::Borrowed(before_wait),
  };
  let after_body = &text[body_range.end..];

  // A scheduled send does not touch `> last=` — that's stamped on delivery.
  let effective_ts = if schedule.is_some() { None } else { timestamp };

  let mut new_text = String::with_capacity(text.len() + message_text.len() + 32);
  if let Some(ts) = effective_ts {
    let ts_line = format!("> last={}", ts);
    if let Some(ref range) = session.last_active_line_range {
      // Replace existing `> last=` line within before_wait.
      new_text.push_str(&before_wait[..range.start]);
      new_text.push_str(&ts_line);
      new_text.push_str(&before_wait[range.end..]);
    } else if !session.uuid.is_empty() {
      // Insert after `> uuid=` line.
      let insert_at = skip_newline(text.as_bytes(), session.uuid_byte_range.end);
      new_text.push_str(&before_wait[..insert_at]);
      new_text.push_str(&ts_line);
      new_text.push('\n');
      new_text.push_str(&before_wait[insert_at..]);
    } else {
      new_text.push_str(&before_wait);
    }
  } else {
    new_text.push_str(&before_wait);
  }
  match schedule {
    Some(dt) => {
      new_text.push_str(&format!("### Message {} @jc({})\n", message_index, format_schedule(dt)))
    }
    None => new_text.push_str(&format!("### Message {}\n", message_index)),
  }
  new_text.push_str(&message_text);
  new_text.push('\n');
  new_text.push_str("### WAIT\n");
  let wait_body_offset = new_text.len();
  new_text.push_str(&remaining);
  new_text.push_str(after_body);

  Some(SendResult { new_text, message_text, message_index, wait_body_offset, schedule })
}

/// Bound every session's message log in `text` to its most recent `keep`
/// entries, returning the rewritten document or `None` if nothing was dropped.
///
/// Sends bound only the session being written to, which leaves a session that
/// has gone quiet holding whatever it had accumulated. This is the sweep for
/// those, meant for startup, before anything else reads the document.
pub fn truncate_all_sessions(text: &str, keep: usize) -> Option<String> {
  let doc = parse(text);
  // `parse` yields sessions in document order and each span sits inside its own
  // session's log, so the spans come out ascending and disjoint.
  let spans: Vec<Range<usize>> =
    doc.sessions.iter().filter_map(|session| stale_log_span(session, keep)).collect();
  if spans.is_empty() {
    return None;
  }

  let mut out = String::with_capacity(text.len());
  let mut at = 0;
  for span in spans {
    debug_assert!(span.start >= at, "stale spans out of order or overlapping");
    out.push_str(&text[at..span.start]);
    at = span.end;
  }
  out.push_str(&text[at..]);
  Some(out)
}

/// Byte span of the oldest `### Message N` entries of `session` to drop so that
/// at most `keep` remain in its log, or `None` when nothing needs dropping.
///
/// Offsets are in the document `session` was parsed from. Only entries above the
/// session's `### WAIT` heading count. [`parse`] no longer puts any below it --
/// a `### Message N` line in the draft body is draft text -- so this is
/// unreachable by construction; it stays because the alternative to being wrong
/// here is deleting the user's unsent draft, and a parser change should not be
/// able to cause that.
///
/// A message still carrying an undelivered `@jc(...)` marker bounds the span, so
/// dropping one can never silently cancel a scheduled send. Callers gate sends
/// on [`TodoSession::pending_scheduled`], which makes that unreachable today;
/// it is kept so a change to that gate cannot turn into lost work.
fn stale_log_span(session: &TodoSession, keep: usize) -> Option<Range<usize>> {
  let log_end = session.wait.as_ref().map_or(usize::MAX, |w| w.heading_byte_range.start);
  let logged = session.messages.iter().take_while(|m| m.body_byte_range.end <= log_end).count();
  let log = &session.messages[..logged];

  let mut drop_count = log.len().checked_sub(keep)?;
  if let Some(pending) = log.iter().position(|m| m.schedule.is_some()) {
    drop_count = drop_count.min(pending);
  }
  if drop_count == 0 {
    return None;
  }

  // Message bodies run to the next heading, so the dropped entries are one
  // contiguous span from the first heading to the last body's end.
  Some(log[0].heading_byte_range.start..log[drop_count - 1].body_byte_range.end)
}

/// Outcome of evaluating a session's pending scheduled send at time `now_local`.
/// All text rewriting is done here; the caller only applies `new_text` (if any)
/// to the editor and acts on the variant.
pub enum FireOutcome {
  /// The scheduled time arrived. Write `new_text` (marker dropped, `> last=`
  /// stamped) and deliver `body` to the terminal.
  Deliver { new_text: String, body: String },
  /// The marker's time was edited into the future — re-arm for this instant.
  /// No text change.
  Reschedule(NaiveDateTime),
  /// Nothing to deliver. `new_text` is `Some` when the marker was dropped
  /// (empty body → cancel, but unblock the session) and must be written,
  /// `None` when there was no pending marker to begin with.
  Cancelled { new_text: Option<String> },
}

/// Evaluate the pending scheduled send of the session bound to `uuid` against
/// `now_local`, producing the rewritten document text and what the caller should
/// do. Pure: parses `text` once and owns the marker-drop + `> last=` stamp so
/// callers never reason about byte-range validity across intermediate strings.
pub fn fire_scheduled(
  text: &str,
  uuid: &str,
  now_local: NaiveDateTime,
  now_secs: u64,
) -> FireOutcome {
  let doc = parse(text);
  let Some(session) = doc.session_by_uuid(uuid) else {
    return FireOutcome::Cancelled { new_text: None };
  };
  let Some(pending) = session.pending_scheduled() else {
    return FireOutcome::Cancelled { new_text: None };
  };
  if let Some(when) = pending.schedule
    && when > now_local
  {
    return FireOutcome::Reschedule(when);
  }

  let body = text[pending.body_byte_range.clone()].trim().to_string();
  // Drop the marker unconditionally so the session isn't left blocked; an empty
  // body cancels the send but must still clear the marker.
  let dropped = drop_schedule_marker(text, pending);
  if body.is_empty() {
    FireOutcome::Cancelled { new_text: Some(dropped) }
  } else {
    // `session`'s metadata byte ranges precede the heading `drop_schedule_marker`
    // shortened, so they stay valid against `dropped`.
    let new_text = update_last_active(&dropped, session, now_secs);
    FireOutcome::Deliver { new_text, body }
  }
}

/// Update (or insert) the `> last=TIMESTAMP` line for a session.
///
/// If the session already has a `> last=` line, it is replaced in-place.
/// Otherwise a new line is inserted right after the `> uuid=` line.
/// Returns the updated document text.
pub fn update_last_active(text: &str, session: &TodoSession, timestamp: u64) -> String {
  let new_line = format!("> last={}", timestamp);

  if let Some(ref range) = session.last_active_line_range {
    // Replace existing `> last=` line.
    let mut result = String::with_capacity(text.len());
    result.push_str(&text[..range.start]);
    result.push_str(&new_line);
    result.push_str(&text[range.end..]);
    result
  } else if !session.uuid.is_empty() {
    // Insert after the `> uuid=` line.  The uuid_byte_range covers just the
    // value; we need to find the end of the full `> uuid=...` line + its newline.
    let uuid_line_end = session.uuid_byte_range.end;
    // Skip past the newline after the uuid line.
    let insert_at = if uuid_line_end < text.len() && text.as_bytes()[uuid_line_end] == b'\n' {
      uuid_line_end + 1
    } else {
      uuid_line_end
    };
    let mut result = String::with_capacity(text.len() + new_line.len() + 1);
    result.push_str(&text[..insert_at]);
    result.push_str(&new_line);
    result.push('\n');
    result.push_str(&text[insert_at..]);
    result
  } else {
    // No uuid — can't attach metadata. Return unchanged.
    text.to_string()
  }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;

  /// Fixed local "now" for send tests that don't exercise scheduling.
  fn test_now() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 7, 13).unwrap().and_hms_opt(9, 0, 0).unwrap()
  }

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
  fn uuid_byte_range_covers_uuid_text() {
    let text = "# Claude\n## My Label\n> uuid=my-uuid\n";
    let doc = parse(text);
    let session = &doc.sessions[0];
    assert_eq!(&text[session.uuid_byte_range.clone()], "my-uuid");
  }

  #[test]
  fn last_active_parsed() {
    let text = "# Claude\n## S\n> uuid=abc\n> last=1700000000\n\n### WAIT\n";
    let doc = parse(text);
    let session = &doc.sessions[0];
    assert_eq!(session.last_active, Some(1700000000));
    let range = session.last_active_line_range.as_ref().unwrap();
    assert_eq!(&text[range.clone()], "> last=1700000000");
  }

  #[test]
  fn last_active_missing() {
    let text = "# Claude\n## S\n> uuid=abc\n\n### WAIT\n";
    let doc = parse(text);
    let session = &doc.sessions[0];
    assert_eq!(session.last_active, None);
    assert!(session.last_active_line_range.is_none());
  }

  #[test]
  fn dangerous_flag_parsed() {
    let text = "# Claude\n## S\n> uuid=abc\n> dangerous\n\n### WAIT\n";
    let doc = parse(text);
    assert!(doc.sessions[0].dangerous);
  }

  #[test]
  fn dangerous_flag_default_false() {
    let text = "# Claude\n## S\n> uuid=abc\n\n### WAIT\n";
    let doc = parse(text);
    assert!(!doc.sessions[0].dangerous);
  }

  #[test]
  fn dangerous_flag_with_last_active_any_order() {
    // dangerous before last
    let text1 = "# Claude\n## S\n> uuid=abc\n> dangerous\n> last=42\n\n### WAIT\n";
    let s1 = &parse(text1).sessions[0];
    assert!(s1.dangerous);
    assert_eq!(s1.last_active, Some(42));
    // last before dangerous
    let text2 = "# Claude\n## S\n> uuid=abc\n> last=42\n> dangerous\n\n### WAIT\n";
    let s2 = &parse(text2).sessions[0];
    assert!(s2.dangerous);
    assert_eq!(s2.last_active, Some(42));
  }

  #[test]
  fn unknown_metadata_keys_ignored() {
    // Unknown keys are silently consumed; subsequent known keys still parse.
    let text = "# Claude\n## S\n> uuid=abc\n> futureKey=xyz\n> dangerous\n\n### WAIT\n";
    let doc = parse(text);
    assert!(doc.sessions[0].dangerous);
  }

  #[test]
  fn last_active_does_not_disrupt_body() {
    let text = "\
# Claude
## S
> uuid=abc
> last=1700000000

### WAIT
body here
";
    let doc = parse(text);
    let session = &doc.sessions[0];
    assert_eq!(session.last_active, Some(1700000000));
    let body = &text[session.wait.as_ref().unwrap().body_byte_range.clone()];
    assert!(body.contains("body here"));
  }

  #[test]
  fn update_last_active_inserts() {
    let text = "# Claude\n## S\n> uuid=abc\n\n### WAIT\n";
    let doc = parse(text);
    let session = &doc.sessions[0];
    let updated = update_last_active(text, session, 1700000000);
    assert!(updated.contains("> last=1700000000\n"));
    // Re-parse to verify round-trip.
    let doc2 = parse(&updated);
    assert_eq!(doc2.sessions[0].last_active, Some(1700000000));
    // WAIT body should still be parseable.
    assert!(doc2.sessions[0].wait.is_some());
  }

  #[test]
  fn update_last_active_replaces() {
    let text = "# Claude\n## S\n> uuid=abc\n> last=1000\n\n### WAIT\n";
    let doc = parse(text);
    let session = &doc.sessions[0];
    assert_eq!(session.last_active, Some(1000));
    let updated = update_last_active(text, session, 2000);
    assert!(updated.contains("> last=2000"));
    assert!(!updated.contains("> last=1000"));
    let doc2 = parse(&updated);
    assert_eq!(doc2.sessions[0].last_active, Some(2000));
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
  fn body_end_line_lands_on_last_content_line() {
    // Body has content followed by next heading — cursor should land on the
    // last body line ("three"), not on "## More".
    let text = "\
# Claude
## S
> uuid=s

### WAIT
one
two
three
## More
";
    let doc = parse(text);
    let wait = doc.session_by_label("S").unwrap().wait.as_ref().unwrap();
    let line = wait.body_end_line(text);
    // "three" is on line 7 (0-indexed).
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[line as usize], "three");
  }

  #[test]
  fn body_end_line_with_trailing_blank_line() {
    // Body has a blank line before the next heading — cursor should land on
    // that blank line, not on "## More".
    let text = "\
# Claude
## S
> uuid=s

### WAIT
one
two
three

## More
";
    let doc = parse(text);
    let wait = doc.session_by_label("S").unwrap().wait.as_ref().unwrap();
    let line = wait.body_end_line(text);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[line as usize], "");
    // And the line after is the next heading.
    assert_eq!(lines[line as usize + 1], "## More");
  }

  #[test]
  fn body_end_line_at_end_of_document() {
    // WAIT body extends to end of document (no following heading).
    let text = "\
# Claude
## S
> uuid=s

### WAIT
only line
";
    let doc = parse(text);
    let wait = doc.session_by_label("S").unwrap().wait.as_ref().unwrap();
    let line = wait.body_end_line(text);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[line as usize], "only line");
  }

  #[test]
  fn body_end_line_empty_body() {
    // Empty WAIT body — cursor should land on the WAIT heading line itself.
    let text = "\
# Claude
## S
> uuid=s

### WAIT
## Next
";
    let doc = parse(text);
    let wait = doc.session_by_label("S").unwrap().wait.as_ref().unwrap();
    let line = wait.body_end_line(text);
    let lines: Vec<&str> = text.lines().collect();
    // With empty body, body_start == body_end (after backing up), so we
    // get the WAIT heading line or the line right after it.
    assert!(line <= 5, "line {} should be at or near WAIT heading", line);
    // Should NOT be on "## Next".
    assert_ne!(lines[line as usize], "## Next");
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

    let result = send_from_wait(text, session, sel_start..sel_end, None, test_now()).unwrap();
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
    let result = send_from_wait(text, session, 0..0, None, test_now()).unwrap();
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

    let result = send_from_wait(text, session, sel_start..sel_end, None, test_now()).unwrap();
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
    let result = send_from_wait(text, session, cursor..cursor, None, test_now()).unwrap();
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
    let result = send_from_wait(text, session, cursor..cursor, None, test_now()).unwrap();
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
    assert!(send_from_wait(text, session, 0..0, None, test_now()).is_none());
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
    assert!(send_from_wait(text, session, 0..0, None, test_now()).is_none());
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
    let result = send_from_wait(text, session, 0..0, None, test_now()).unwrap();
    assert_eq!(result.message_index, 2);
    assert_eq!(result.message_text, "third");
  }

  /// Build a session whose log holds `count` messages, indices `0..count`.
  /// `pinned` marks one of them as an undelivered scheduled send, and `draft`
  /// is appended inside the WAIT body.
  fn session_log(count: usize, pinned: Option<usize>, draft: &str) -> String {
    let mut text = String::from("# Claude\n## S\n> uuid=s\n\n");
    for i in 0..count {
      match pinned {
        Some(p) if p == i => text.push_str(&format!("### Message {i} @jc(2026-07-14 07:30)\n")),
        _ => text.push_str(&format!("### Message {i}\n")),
      }
      text.push_str(&format!("body {i}\n"));
    }
    text.push_str("### WAIT\n");
    text.push_str(draft);
    text
  }

  fn parsed_session(text: &str) -> TodoSession {
    parse(text).session_by_label("S").unwrap().clone()
  }

  /// Everything below `### WAIT` is draft. Quoting an old message there is
  /// ordinary drafting, and must not register as a message of the session.
  #[test]
  fn wait_draft_message_lines_are_not_messages() {
    let text = "\
# Claude
## S
> uuid=s

### Message 0
real
### WAIT
as I said in
### Message 99
quoted text
please do it
";
    let session = parsed_session(text);

    assert_eq!(session.messages.iter().map(|m| m.index).collect::<Vec<_>>(), vec![0]);

    // Ask `send_from_wait` itself rather than retyping how it picks an index.
    let result = send_from_wait(text, &session, 0..0, None, test_now()).unwrap();
    assert_eq!(result.message_index, 1, "next send must not take the quoted index");
    // And the quoted lines are draft, so they are what gets sent.
    assert!(result.message_text.contains("### Message 99"));
    assert!(result.message_text.contains("please do it"));
  }

  /// A quoted `@jc(...)` marker in the draft must not read as a pending
  /// scheduled send -- callers gate on this, so it would lock the session out
  /// of sending with no way to see why.
  #[test]
  fn wait_draft_schedule_marker_does_not_block_sends() {
    let text = "\
# Claude
## S
> uuid=s

### Message 0
real
### WAIT
remember
### Message 3 @jc(2026-07-14 07:30)
quoted
";
    let session = parsed_session(text);
    assert!(session.pending_scheduled().is_none());
  }

  /// Defence in depth for the truncator: even handed a session whose messages
  /// run past the WAIT heading, the span stops above it. The parser no longer
  /// produces such a session, so build one directly.
  #[test]
  fn stale_log_span_ignores_messages_below_wait() {
    let mut session = TodoSession { label: "S".into(), ..Default::default() };
    let mut at = 0;
    let push = |session: &mut TodoSession, at: &mut usize| {
      session.messages.push(TodoMessage {
        index: session.messages.len(),
        heading_byte_range: *at..*at + 10,
        body_byte_range: *at + 10..*at + 20,
        ..Default::default()
      });
      *at += 20;
    };
    for _ in 0..3 {
      push(&mut session, &mut at);
    }
    let wait_start = at;
    session.wait = Some(TodoWait {
      heading_byte_range: wait_start..wait_start + 8,
      body_byte_range: wait_start + 8..wait_start + 100,
      ..Default::default()
    });
    at = wait_start + 8;
    for _ in 0..5 {
      push(&mut session, &mut at);
    }

    let span = stale_log_span(&session, 1).unwrap();
    assert!(span.end <= wait_start, "span {span:?} reaches past WAIT at {wait_start}");
  }

  /// The startup sweep bounds every session in the document at once, and
  /// leaves a document that is already within the bound untouched.
  #[test]
  fn truncate_all_sessions_bounds_every_session() {
    let mut text = String::from("# Claude\n");
    let counts = [40usize, 3, 60];
    for (n, count) in counts.iter().enumerate() {
      text.push_str(&format!("## S{n}\n> uuid=u{n}\n\n"));
      for i in 0..*count {
        text.push_str(&format!("### Message {i}\nbody {n}-{i}\n"));
      }
      // Distinct per session, so a draft spliced from the wrong one shows up.
      text.push_str(&format!("### WAIT\ndraft for session {n}\n"));
    }

    let out = truncate_all_sessions(&text, MAX_MESSAGES).unwrap();
    let before = parse(&text);
    let doc = parse(&out);

    assert_eq!(doc.sessions.len(), counts.len());
    for ((n, session), count) in doc.sessions.iter().enumerate().zip(counts.iter()) {
      assert_eq!(session.messages.len(), (*count).min(MAX_MESSAGES));
      // Newest kept, indices intact.
      assert_eq!(session.messages.last().unwrap().index, count - 1);
      // Identity and draft belong to THIS session and survive byte-for-byte.
      assert_eq!(session.uuid, format!("u{n}"));
      assert_eq!(session.label, format!("S{n}"));
      let draft = &out[session.wait.as_ref().unwrap().body_byte_range.clone()];
      let was = &text[before.sessions[n].wait.as_ref().unwrap().body_byte_range.clone()];
      assert_eq!(draft, was);
      // The surviving bodies are this session's, not a neighbour's.
      let oldest = session.messages.first().unwrap();
      let body = &out[oldest.body_byte_range.clone()];
      assert!(body.ends_with(&format!("body {n}-{}\n", oldest.index)), "body was {body:?}");
    }

    // Idempotent: a second sweep finds nothing to do.
    assert!(truncate_all_sessions(&out, MAX_MESSAGES).is_none());
  }

  /// A pending `@jc(...)` marker pins everything from it onward, and the pin
  /// converges: the first sweep clears what is above the marker, after which the
  /// marker sits at index 0 and no later sweep can drop anything. The session
  /// stays above the bound until the scheduled send fires. Documented in
  /// ARCH.md and README.md, so pin it.
  #[test]
  fn truncate_all_sessions_converges_while_a_send_is_pending() {
    let pinned = 3;
    let total = MAX_MESSAGES + 20;
    let text = session_log(total, Some(pinned), "draft\n");

    let first = truncate_all_sessions(&text, MAX_MESSAGES).unwrap();
    let after = parse(&first);
    let session = after.session_by_label("S").unwrap();

    // Exactly the entries above the marker went; the marker leads what remains.
    assert_eq!(session.messages.len(), total - pinned);
    assert_eq!(session.messages[0].index, pinned);
    assert!(session.messages[0].schedule.is_some());
    assert!(session.messages.len() > MAX_MESSAGES, "still above the bound, by design");

    // And it is now stuck there until the send fires.
    assert!(truncate_all_sessions(&first, MAX_MESSAGES).is_none());
  }

  #[test]
  fn stale_log_span_leaves_short_logs_alone() {
    for count in [0, 1, 2, 3] {
      let text = session_log(count, None, "");
      assert!(
        stale_log_span(&parsed_session(&text), 3).is_none(),
        "{count} messages under a keep of 3 should not be truncated"
      );
    }
  }

  /// The span covers exactly the oldest entries, ending where the first
  /// survivor's heading begins.
  #[test]
  fn stale_log_span_covers_the_oldest_entries() {
    let text = session_log(5, None, "");
    let session = parsed_session(&text);
    let span = stale_log_span(&session, 2).unwrap();

    assert_eq!(span.start, session.messages[0].heading_byte_range.start);
    assert_eq!(span.end, session.messages[3].heading_byte_range.start);
    // Cutting it leaves precisely the newest two, indices intact.
    let cut = format!("{}{}", &text[..span.start], &text[span.end..]);
    let after = parsed_session(&cut);
    assert_eq!(after.messages.iter().map(|m| m.index).collect::<Vec<_>>(), vec![3, 4]);
  }

  /// An undelivered `@jc(...)` marker bounds the span: entries older than it are
  /// still dropped, but it and everything after it stay.
  #[test]
  fn stale_log_span_stops_at_a_pending_schedule() {
    let pinned = 1;
    let text = session_log(6, Some(pinned), "");
    let session = parsed_session(&text);
    assert!(session.messages[pinned].schedule.is_some(), "fixture must have a pending marker");

    let span = stale_log_span(&session, 2).unwrap();
    assert_eq!(span.end, session.messages[pinned].heading_byte_range.start);

    // Nothing to do at all when the marker sits on the oldest entry.
    let text = session_log(6, Some(0), "");
    assert!(stale_log_span(&parsed_session(&text), 2).is_none());
  }

  /// A send bounds the log to the most recent `MAX_MESSAGES`, dropping the
  /// oldest without renumbering the survivors.
  #[test]
  fn send_from_wait_bounds_message_log() {
    let existing = MAX_MESSAGES + 10;
    let text = session_log(existing, None, "next up\n");
    let doc = parse(&text);
    let session = doc.session_by_label("S").unwrap();

    let result = send_from_wait(&text, session, 0..0, None, test_now()).unwrap();
    let after = parse(&result.new_text);
    let session = after.session_by_label("S").unwrap();

    assert_eq!(session.messages.len(), MAX_MESSAGES);
    // Indices are preserved: the window ends at the message just sent.
    let newest = existing;
    let expected: Vec<usize> = (newest + 1 - MAX_MESSAGES..=newest).collect();
    assert_eq!(session.messages.iter().map(|m| m.index).collect::<Vec<_>>(), expected);
    assert_eq!(result.message_index, newest);
    assert!(!result.new_text.contains("### Message 0\n"));
  }

  /// The cut happens above the WAIT heading, so the offset the caller puts the
  /// cursor at must still land at the start of the WAIT body.
  #[test]
  fn send_from_wait_offset_survives_truncation() {
    let text = session_log(MAX_MESSAGES + 10, None, "next up\n");
    let doc = parse(&text);
    let session = doc.session_by_label("S").unwrap();

    let result = send_from_wait(&text, session, 0..0, None, test_now()).unwrap();

    // Derive the expectation from the truncated document rather than counting
    // bytes by hand.
    let after = parse(&result.new_text);
    let wait = after.session_by_label("S").unwrap().wait.as_ref().unwrap();
    let body_start = skip_newline(result.new_text.as_bytes(), wait.heading_byte_range.end);
    assert_eq!(result.wait_body_offset, body_start);
  }

  /// Truncation must not disturb the `> last=` rewrite that happens in the same
  /// pass, nor be disturbed by it.
  #[test]
  fn send_from_wait_bounds_log_and_stamps_timestamp() {
    let text = session_log(MAX_MESSAGES + 4, None, "next up\n");
    let doc = parse(&text);
    let session = doc.session_by_label("S").unwrap();

    let result = send_from_wait(&text, session, 0..0, Some(1700000000), test_now()).unwrap();
    let after = parse(&result.new_text);
    let session = after.session_by_label("S").unwrap();

    assert_eq!(session.last_active, Some(1700000000));
    assert_eq!(session.messages.len(), MAX_MESSAGES);
    assert_eq!(session.uuid, "s");
  }

  #[test]
  fn send_from_wait_stamps_timestamp() {
    let text = "# Claude\n## S\n> uuid=abc\n\n### WAIT\nhello\n";
    let doc = parse(text);
    let session = doc.session_by_label("S").unwrap();
    let result = send_from_wait(text, session, 0..0, Some(1700000000), test_now()).unwrap();
    assert!(result.new_text.contains("> last=1700000000\n"));
    let doc2 = parse(&result.new_text);
    assert_eq!(doc2.sessions[0].last_active, Some(1700000000));
  }

  #[test]
  fn send_from_wait_updates_existing_timestamp() {
    let text = "# Claude\n## S\n> uuid=abc\n> last=1000\n\n### WAIT\nhello\n";
    let doc = parse(text);
    let session = doc.session_by_label("S").unwrap();
    assert_eq!(session.last_active, Some(1000));
    let result = send_from_wait(text, session, 0..0, Some(2000), test_now()).unwrap();
    assert!(result.new_text.contains("> last=2000"));
    assert!(!result.new_text.contains("> last=1000"));
    let doc2 = parse(&result.new_text);
    assert_eq!(doc2.sessions[0].last_active, Some(2000));
  }

  // -------------------------------------------------------------------------
  // Scheduled sends
  // -------------------------------------------------------------------------

  #[test]
  fn parse_schedule_prefix_valid() {
    assert_eq!(parse_schedule_prefix("@jc(07:30) do the thing"), Some((7, 30, 10)));
    assert_eq!(parse_schedule_prefix("  @jc(23:05)rest"), Some((23, 5, 12)));
  }

  #[test]
  fn parse_schedule_prefix_rejects_bad() {
    assert_eq!(parse_schedule_prefix("hello @jc(07:30)"), None); // not leading
    assert_eq!(parse_schedule_prefix("@jc(24:00)"), None); // hour out of range
    assert_eq!(parse_schedule_prefix("@jc(07:60)"), None); // minute out of range
    assert_eq!(parse_schedule_prefix("@jc(0730)"), None); // no colon
    assert_eq!(parse_schedule_prefix("@jc(07:30"), None); // unclosed
  }

  #[test]
  fn resolve_schedule_today_vs_tomorrow() {
    let now = NaiveDate::from_ymd_opt(2026, 7, 13).unwrap().and_hms_opt(9, 0, 0).unwrap();
    // Later today.
    let later = resolve_schedule(17, 30, now).unwrap();
    assert_eq!(
      later,
      NaiveDate::from_ymd_opt(2026, 7, 13).unwrap().and_hms_opt(17, 30, 0).unwrap()
    );
    // Already passed → tomorrow.
    let next = resolve_schedule(7, 30, now).unwrap();
    assert_eq!(next, NaiveDate::from_ymd_opt(2026, 7, 14).unwrap().and_hms_opt(7, 30, 0).unwrap());
  }

  #[test]
  fn resolve_schedule_same_minute_fires_today() {
    // 07:30:40 with @jc(07:30): the minute hasn't fully passed, so fire today.
    let now = NaiveDate::from_ymd_opt(2026, 7, 13).unwrap().and_hms_opt(7, 30, 40).unwrap();
    let when = resolve_schedule(7, 30, now).unwrap();
    assert_eq!(when, NaiveDate::from_ymd_opt(2026, 7, 13).unwrap().and_hms_opt(7, 30, 0).unwrap());
    // But a minute already fully elapsed rolls to tomorrow.
    let when2 = resolve_schedule(7, 29, now).unwrap();
    assert_eq!(when2, NaiveDate::from_ymd_opt(2026, 7, 14).unwrap().and_hms_opt(7, 29, 0).unwrap());
  }

  #[test]
  fn schedule_roundtrips_through_marker() {
    let dt = NaiveDate::from_ymd_opt(2026, 7, 13).unwrap().and_hms_opt(7, 30, 0).unwrap();
    let s = format_schedule(dt);
    assert_eq!(s, "2026-07-13 07:30");
    assert_eq!(parse_schedule_datetime(&s), Some(dt));
  }

  #[test]
  fn send_from_wait_schedules_message() {
    let text = "# Claude\n## S\n> uuid=abc\n\n### WAIT\n@jc(07:30) fix the parser\n";
    let doc = parse(text);
    let session = doc.session_by_label("S").unwrap();
    let now = NaiveDate::from_ymd_opt(2026, 7, 13).unwrap().and_hms_opt(9, 0, 0).unwrap();
    let result = send_from_wait(text, session, 0..0, Some(1700000000), now).unwrap();

    // Marker stripped from the message body; resolved time is tomorrow.
    assert_eq!(result.message_text, "fix the parser");
    assert_eq!(
      result.schedule,
      Some(NaiveDate::from_ymd_opt(2026, 7, 14).unwrap().and_hms_opt(7, 30, 0).unwrap())
    );
    // Heading carries the resolved marker; `> last=` is NOT stamped.
    assert!(result.new_text.contains("### Message 0 @jc(2026-07-14 07:30)\n"));
    assert!(!result.new_text.contains("> last="));

    // Re-parsing surfaces the pending schedule on the message.
    let doc2 = parse(&result.new_text);
    let session2 = doc2.session_by_label("S").unwrap();
    let pending = session2.pending_scheduled().unwrap();
    assert_eq!(pending.index, 0);
    assert_eq!(
      pending.schedule,
      Some(NaiveDate::from_ymd_opt(2026, 7, 14).unwrap().and_hms_opt(7, 30, 0).unwrap())
    );
  }

  #[test]
  fn scheduled_message_body_is_readable_and_editable() {
    let text = "# Claude\n## S\n> uuid=abc\n\n### Message 0 @jc(2026-07-14 07:30)\nfix the parser\n### WAIT\n";
    let doc = parse(text);
    let session = doc.session_by_label("S").unwrap();
    let pending = session.pending_scheduled().unwrap();
    let body = text[pending.body_byte_range.clone()].trim();
    assert_eq!(body, "fix the parser");
  }

  #[test]
  fn empty_schedule_marker_does_not_send() {
    let text = "# Claude\n## S\n> uuid=abc\n\n### WAIT\n@jc(07:30)\n";
    let doc = parse(text);
    let session = doc.session_by_label("S").unwrap();
    let now = NaiveDate::from_ymd_opt(2026, 7, 13).unwrap().and_hms_opt(9, 0, 0).unwrap();
    assert!(send_from_wait(text, session, 0..0, Some(1), now).is_none());
  }

  #[test]
  fn fire_scheduled_delivers_when_due() {
    let text = "# Claude\n## S\n> uuid=abc\n\n### Message 0 @jc(2026-07-13 07:30)\nfix the parser\n### WAIT\n";
    let now = NaiveDate::from_ymd_opt(2026, 7, 13).unwrap().and_hms_opt(7, 31, 0).unwrap();
    match fire_scheduled(text, "abc", now, 1700000000) {
      FireOutcome::Deliver { new_text, body } => {
        assert_eq!(body, "fix the parser");
        assert!(new_text.contains("### Message 0\n")); // marker dropped
        assert!(!new_text.contains("@jc("));
        assert!(new_text.contains("> last=1700000000")); // stamped on delivery
      }
      _ => panic!("expected Deliver"),
    }
  }

  #[test]
  fn fire_scheduled_reschedules_when_future() {
    let text =
      "# Claude\n## S\n> uuid=abc\n\n### Message 0 @jc(2026-07-13 09:00)\nbody\n### WAIT\n";
    let now = NaiveDate::from_ymd_opt(2026, 7, 13).unwrap().and_hms_opt(7, 0, 0).unwrap();
    match fire_scheduled(text, "abc", now, 1) {
      FireOutcome::Reschedule(when) => {
        assert_eq!(
          when,
          NaiveDate::from_ymd_opt(2026, 7, 13).unwrap().and_hms_opt(9, 0, 0).unwrap()
        );
      }
      _ => panic!("expected Reschedule"),
    }
  }

  #[test]
  fn fire_scheduled_empty_body_cancels_but_drops_marker() {
    let text = "# Claude\n## S\n> uuid=abc\n\n### Message 0 @jc(2026-07-13 07:30)\n\n### WAIT\n";
    let now = NaiveDate::from_ymd_opt(2026, 7, 13).unwrap().and_hms_opt(7, 31, 0).unwrap();
    match fire_scheduled(text, "abc", now, 1) {
      FireOutcome::Cancelled { new_text: Some(nt) } => {
        assert!(!nt.contains("@jc(")); // marker dropped so session isn't locked
        assert!(!nt.contains("> last=")); // nothing delivered → no stamp
      }
      _ => panic!("expected Cancelled with dropped marker"),
    }
  }

  #[test]
  fn fire_scheduled_no_marker_is_noop() {
    let text = "# Claude\n## S\n> uuid=abc\n\n### Message 0\nbody\n### WAIT\n";
    let now = NaiveDate::from_ymd_opt(2026, 7, 13).unwrap().and_hms_opt(7, 31, 0).unwrap();
    assert!(matches!(
      fire_scheduled(text, "abc", now, 1),
      FireOutcome::Cancelled { new_text: None }
    ));
  }

  #[test]
  fn drop_schedule_marker_delivers() {
    let text = "# Claude\n## S\n> uuid=abc\n\n### Message 0 @jc(2026-07-14 07:30)\nfix the parser\n### WAIT\n";
    let doc = parse(text);
    let session = doc.session_by_label("S").unwrap();
    let pending = session.pending_scheduled().unwrap();
    let delivered = drop_schedule_marker(text, pending);
    assert!(delivered.contains("### Message 0\n"));
    assert!(!delivered.contains("@jc("));
    // Now no longer pending, body preserved.
    let doc2 = parse(&delivered);
    let session2 = doc2.session_by_label("S").unwrap();
    assert!(session2.pending_scheduled().is_none());
    assert_eq!(session2.messages[0].index, 0);
  }

  #[test]
  fn malformed_heading_marker_kept_as_plain_message() {
    let text = "# Claude\n## S\n> uuid=abc\n\n### Message 0 @jc(not-a-date)\nbody\n### WAIT\n";
    let doc = parse(text);
    let session = doc.session_by_label("S").unwrap();
    // Message still parsed (not dropped), just with no schedule.
    assert_eq!(session.messages.len(), 1);
    assert!(session.messages[0].schedule.is_none());
    assert!(session.pending_scheduled().is_none());
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
    let disabled_text = toggle_session_disabled_at(text, &doc, 0).unwrap();
    assert!(disabled_text.contains("## [D] My Session"));
    let doc2 = parse(&disabled_text);
    assert_eq!(doc2.sessions[0].label, "My Session");
    assert!(doc2.sessions[0].status == SessionStatus::Disabled);

    // Re-enable it.
    let enabled_text = toggle_session_disabled_at(&disabled_text, &doc2, 0).unwrap();
    assert!(enabled_text.contains("## My Session"));
    assert!(!enabled_text.contains("[D]"));
    let doc3 = parse(&enabled_text);
    assert_ne!(doc3.sessions[0].status, SessionStatus::Disabled);
  }

  #[test]
  fn toggle_session_disabled_at_targets_the_right_duplicate_label() {
    // Two running sessions an older jc left identically labelled. Disabling the
    // second must not mark the first — a label-keyed toggle would, taking down a
    // live session and leaving the intended one impossible to disable.
    let text = "\
# Claude
## New Session
> uuid=first
## New Session
> uuid=second
";
    let doc = parse(text);
    let toggled = toggle_session_disabled_at(text, &doc, 1).unwrap();
    let after = parse(&toggled);
    assert_eq!(after.sessions[0].uuid, "first");
    assert_eq!(after.sessions[0].status, SessionStatus::Active, "first session untouched");
    assert_eq!(after.sessions[1].uuid, "second");
    assert_eq!(after.sessions[1].status, SessionStatus::Disabled, "second session disabled");
  }

  #[test]
  fn unique_label_suffixes_only_on_collision() {
    let text = "\
# Claude
## New Session
> uuid=a
## New Session 2
> uuid=b
";
    let doc = parse(text);
    assert_eq!(unique_label(&doc, "Fresh"), "Fresh");
    // `New Session` and `New Session 2` are taken, so the next free one is 3.
    assert_eq!(unique_label(&doc, "New Session"), "New Session 3");
  }

  #[test]
  fn empty_uuid_is_not_an_address() {
    // Two legacy headings an older jc never bound, sharing a label. Neither the
    // empty UUID nor the label distinguishes them, so only position can.
    let text = "\
# Claude
## S

### WAIT
## S

### WAIT
";
    let doc = parse(text);
    assert_eq!(doc.sessions.len(), 2);
    assert!(doc.sessions.iter().all(|s| s.uuid.is_empty()), "both headings unbound");
    assert_eq!(doc.index_by_uuid(""), None, "an empty uuid names no session");

    let second = SessionKey::new("", 1, &doc.sessions[1].label);
    assert_eq!(doc.index_of(&second), Some(1), "an unbound heading is addressed by position");

    // The document moved under the key: refuse rather than write to whatever
    // now sits at that index. Each case re-parses a document the user could
    // have produced between a picker snapshot and its confirm.
    let renamed = parse("# Claude\n## S\n\n### WAIT\n## T\n\n### WAIT\n");
    assert_eq!(renamed.index_of(&second), None, "the label at index 1 changed");
    let bound = parse("# Claude\n## S\n\n### WAIT\n## S\n> uuid=bbb\n\n### WAIT\n");
    assert_eq!(bound.index_of(&second), None, "the heading at index 1 is bound now");
    let shorter = parse("# Claude\n## S\n\n### WAIT\n");
    assert_eq!(shorter.index_of(&second), None, "index 1 no longer exists");
  }

  // Two headings sharing a label is a shape a hand-edit produces at any instant
  // -- paste a `## New Session` block mid-run and every text operation on that
  // label resolves to the first heading until the next restart. The three tests
  // below pin each such operation to the UUID instead.

  /// Fixture: two `## S` headings, the second bound to `bbb`.
  fn two_same_labelled_headings() -> &'static str {
    "\
# Claude
## S
> uuid=aaa

### WAIT
one
## S
> uuid=bbb

### WAIT
two
three
"
  }

  #[test]
  fn fire_scheduled_addresses_the_second_of_two_same_labelled_headings() {
    let text = "\
# Claude
## S
> uuid=aaa

### WAIT
## S
> uuid=bbb

### Message 0 @jc(2026-07-13 08:00)
hello

### WAIT
";
    let doc = parse(text);
    assert!(doc.sessions[0].pending_scheduled().is_none(), "no marker on the first heading");
    assert!(doc.sessions[1].pending_scheduled().is_some(), "the marker is on the second");
    match fire_scheduled(text, &doc.sessions[1].uuid, test_now(), 1700000000) {
      FireOutcome::Deliver { body, .. } => assert_eq!(body, "hello"),
      _ => panic!("the second heading's scheduled send was not delivered"),
    }
  }

  #[test]
  fn insert_wait_section_addresses_the_second_of_two_same_labelled_headings() {
    let text = "\
# Claude
## S
> uuid=aaa

### WAIT
one
## S
> uuid=bbb
";
    let doc = parse(text);
    assert!(doc.sessions[0].wait.is_some(), "the first heading already has a WAIT");
    assert!(doc.sessions[1].wait.is_none(), "only the second heading lacks one");

    let new_text = insert_wait_section(text, &doc, &doc.sessions[1].uuid)
      .expect("the second heading needs a WAIT inserted");
    let after = parse(&new_text);
    assert!(after.sessions[1].wait.is_some(), "the second heading got a WAIT");
    // Everything above the second heading must be byte-identical: both spans are
    // taken from the parse, so nothing here restates how the insert works.
    assert_eq!(
      &new_text[..after.sessions[1].heading_byte_range.start],
      &text[..doc.sessions[1].heading_byte_range.start],
      "the write landed on the first heading",
    );
  }

  #[test]
  fn wait_cursor_addresses_the_second_of_two_same_labelled_headings() {
    let text = two_same_labelled_headings();
    let doc = parse(text);
    // `TodoView::wait_line` is exactly this composition: resolve the session by
    // UUID, then ask its WAIT where the body ends.
    let wait = doc
      .session_by_uuid(&doc.sessions[1].uuid)
      .expect("the second heading is bound")
      .wait
      .as_ref()
      .expect("the second heading has a WAIT");
    let line = wait.body_end_line(text);
    // The expected line is derived from the parsed second body, not counted by
    // hand: the cursor must land on that body's last non-blank line.
    let last = text[wait.body_byte_range.clone()].lines().rfind(|l| !l.trim().is_empty());
    assert_eq!(
      text.lines().nth(line as usize),
      last,
      "the cursor landed outside the second heading's WAIT body",
    );
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
  fn legacy_expired_marker_reads_as_disabled() {
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
    assert_eq!(doc.sessions[0].status, SessionStatus::Disabled);
    assert_eq!(doc.sessions[1].label, "Active Session");
    assert_eq!(doc.sessions[1].status, SessionStatus::Active);
  }

  #[test]
  fn enabling_a_legacy_expired_session_normalises_its_heading() {
    let text = "\
# Claude
## [X] GC'd Session
> uuid=gone
";
    let doc = parse(text);
    let enabled = toggle_session_disabled_at(text, &doc, 0).unwrap();
    assert!(enabled.contains("## GC'd Session"));
    assert!(!enabled.contains("[X]"));
    let doc2 = parse(&enabled);
    assert_eq!(doc2.sessions[0].status, SessionStatus::Active);
    assert_eq!(doc2.sessions[0].label, "GC'd Session");
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
    assert!(send_from_wait(text, session, 0..5, None, test_now()).is_none());
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
    let updated = update_session_uuid_at(text, &doc, 0, "new-uuid").unwrap();
    assert!(updated.contains("> uuid=new-uuid"));
    let new_doc = parse(&updated);
    assert_eq!(new_doc.sessions[0].uuid, "new-uuid");
  }

  #[test]
  fn update_session_uuid_adds_a_missing_uuid_line() {
    // A hand-written heading with no `> uuid=` line: the UUID must land on the
    // session rather than being silently dropped.
    let text = "\
# Claude
## My Session

### WAIT
";
    let doc = parse(text);
    assert_eq!(doc.sessions[0].uuid, "");
    let updated = update_session_uuid_at(text, &doc, 0, "fresh-uuid").unwrap();
    let new_doc = parse(&updated);
    assert_eq!(new_doc.sessions.len(), 1);
    assert_eq!(new_doc.sessions[0].label, "My Session");
    assert_eq!(new_doc.sessions[0].uuid, "fresh-uuid");
    assert!(new_doc.sessions[0].wait.is_some(), "WAIT survived the insert");
  }

  #[test]
  fn update_session_uuid_at_targets_the_right_duplicate_label() {
    // Two sessions an older jc wrote with the same auto-label and no UUID yet.
    // A label-keyed write hits the first heading twice and strands the second;
    // the index-keyed write must give each its own UUID.
    let text = "\
# Claude
## New Session
> uuid=
## New Session
> uuid=
";
    let doc = parse(text);
    assert_eq!(doc.sessions.len(), 2);

    let once = update_session_uuid_at(text, &doc, 0, "uuid-a").unwrap();
    let doc = parse(&once);
    let twice = update_session_uuid_at(&once, &doc, 1, "uuid-b").unwrap();

    let final_doc = parse(&twice);
    let uuids: Vec<&str> = final_doc.sessions.iter().map(|s| s.uuid.as_str()).collect();
    assert_eq!(uuids, vec!["uuid-a", "uuid-b"], "each session keeps its own UUID");
  }

  #[test]
  fn update_session_uuid_fills_a_blank_uuid_value() {
    let text = "\
# Claude
## My Session
> uuid=

### WAIT
";
    let doc = parse(text);
    assert_eq!(doc.sessions[0].uuid, "");
    let updated = update_session_uuid_at(text, &doc, 0, "fresh-uuid").unwrap();
    assert_eq!(parse(&updated).sessions[0].uuid, "fresh-uuid");
  }
}
