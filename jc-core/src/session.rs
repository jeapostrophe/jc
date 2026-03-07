use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Deserialized JSONL types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawMessage {
  role: String,
  id: Option<String>,
  content: serde_json::Value,
}

#[derive(Deserialize)]
struct RawEntry {
  #[serde(rename = "type")]
  entry_type: String,
  #[serde(default)]
  message: Option<RawMessage>,
  #[serde(rename = "isMeta")]
  #[serde(default)]
  is_meta: bool,
  #[serde(default)]
  timestamp: Option<String>,
  #[serde(default)]
  slug: Option<String>,
}

// ---------------------------------------------------------------------------
// Parsed turn types
// ---------------------------------------------------------------------------

pub struct UserMessage {
  pub text: String,
  pub timestamp: Option<String>,
}

pub struct AssistantResponse {
  pub message_id: String,
  pub text_blocks: Vec<String>,
}

pub struct Turn {
  pub index: usize,
  pub user: UserMessage,
  pub responses: Vec<AssistantResponse>,
}

impl Turn {
  pub fn render_markdown(&self) -> String {
    let mut out = String::default();
    out.push_str("# Request\n\n");
    out.push_str(&self.user.text);
    out.push_str("\n\n# Reply\n\n");

    for resp in &self.responses {
      for block in &resp.text_blocks {
        out.push_str(block);
        out.push('\n');
      }
    }
    out
  }

  pub fn label(&self) -> String {
    let text = self.user.text.trim();
    let first_line = text.lines().next().unwrap_or(text);
    truncate_utf8(first_line, 80)
  }
}

fn truncate_utf8(s: &str, max_bytes: usize) -> String {
  if s.len() <= max_bytes {
    return s.to_string();
  }
  let truncate_at = s
    .char_indices()
    .take_while(|(i, _)| *i <= max_bytes)
    .last()
    .map(|(i, c)| i + c.len_utf8())
    .unwrap_or(s.len());
  let mut result = s[..truncate_at].to_string();
  result.push_str("...");
  result
}

// ---------------------------------------------------------------------------
// Path encoding + session discovery
// ---------------------------------------------------------------------------

pub fn encode_project_path(path: &Path) -> String {
  let s = path.to_string_lossy();
  s.replace('/', "-")
}

pub fn session_dir(project_path: &Path) -> PathBuf {
  let encoded = encode_project_path(project_path);
  dirs::home_dir()
    .expect("could not determine home directory")
    .join(".claude/projects")
    .join(encoded)
}

/// A group of JSONL files sharing the same slug, sorted newest-first by mtime.
pub struct SessionGroup {
  pub slug: String,
  /// JSONL file paths, sorted newest-first by modification time.
  pub files: Vec<PathBuf>,
  /// Modification time of the most recent file in the group.
  pub latest_mtime: SystemTime,
}

impl SessionGroup {
  /// The most recently modified file in the group (the "active" session file).
  pub fn latest_file(&self) -> &Path {
    &self.files[0]
  }

  /// The session UUID from the most recent file (for `--resume`).
  pub fn latest_session_id(&self) -> Option<String> {
    self.files[0].file_stem().map(|s| s.to_string_lossy().into_owned())
  }

  /// A short human-readable summary extracted from the first informative user
  /// message in the session group.
  pub fn summary(&self) -> Option<String> {
    // Try the oldest file first (the original session start), then fall back
    // to newer files which may be forks with generic openers.
    for path in self.files.iter().rev() {
      if let Some(s) = extract_first_user_summary(path) {
        return Some(s);
      }
    }
    None
  }
}

/// Extract a short summary from a JSONL file by finding the first informative
/// user message.  Reads at most 200 lines — the interesting content (plan
/// headings, task descriptions) appears early.
pub fn extract_first_user_summary(path: &Path) -> Option<String> {
  use std::io::{BufRead, BufReader};
  let file = std::fs::File::open(path).ok()?;
  let reader = BufReader::new(file);

  for line in reader.lines().take(200) {
    let line = line.ok()?;
    // Fast-path: skip lines that aren't user messages.
    if !line.contains("\"user\"") {
      continue;
    }
    let entry: RawEntry = match serde_json::from_str(&line) {
      Ok(e) => e,
      Err(_) => continue,
    };
    if entry.entry_type != "user" || entry.is_meta {
      continue;
    }
    let Some(msg) = entry.message else { continue };
    if msg.role != "user" {
      continue;
    }
    let text = extract_user_text(&msg.content);
    for l in text.lines() {
      let l = l.trim();
      if l.is_empty()
        || l.starts_with('<')
        || l == "Review @README.md"
        || l == "[Request interrupted by user for tool use]"
        || l.contains("Implement the following plan")
      {
        continue;
      }
      // Strip leading markdown heading markers.
      let l = l.trim_start_matches('#').trim_start();
      if l.is_empty() {
        continue;
      }
      return Some(truncate_utf8(l, 80));
    }
  }
  None
}

/// Look up the slug for a session UUID by finding the corresponding JSONL file.
pub fn session_id_to_slug(project_path: &Path, session_id: &str) -> Option<String> {
  let target = session_dir(project_path).join(format!("{session_id}.jsonl"));
  if target.exists() { extract_slug(&target) } else { None }
}

/// Extract the slug from a JSONL file by reading until we find one.
/// Only reads the first few KB — slugs appear in early entries.
fn extract_slug(path: &Path) -> Option<String> {
  use std::io::{BufRead, BufReader};
  let file = std::fs::File::open(path).ok()?;
  let reader = BufReader::new(file);
  for line in reader.lines().take(20) {
    let line = line.ok()?;
    // Fast path: skip lines without "slug" to avoid full JSON parse.
    if !line.contains("\"slug\"") {
      continue;
    }
    if let Ok(entry) = serde_json::from_str::<RawEntry>(&line)
      && let Some(slug) = entry.slug
    {
      return Some(slug);
    }
  }
  None
}

/// Collect all JSONL files in the session dir with their mtimes and slugs.
fn collect_session_files(project_path: &Path) -> Vec<(PathBuf, SystemTime, String)> {
  let dir = session_dir(project_path);
  let entries = match std::fs::read_dir(&dir) {
    Ok(e) => e,
    Err(_) => return Vec::new(),
  };

  entries
    .filter_map(|e| e.ok())
    .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
    .filter_map(|e| {
      let mtime = e.metadata().ok()?.modified().ok()?;
      let slug = extract_slug(&e.path())?;
      Some((e.path(), mtime, slug))
    })
    .collect()
}

/// Sort entries newest-first by mtime and build a `SessionGroup`.
/// Returns `None` if `entries` is empty.
fn build_session_group(
  slug: String,
  mut entries: Vec<(PathBuf, SystemTime)>,
) -> Option<SessionGroup> {
  if entries.is_empty() {
    return None;
  }
  entries.sort_by(|a, b| b.1.cmp(&a.1));
  let latest_mtime = entries[0].1;
  Some(SessionGroup { slug, files: entries.into_iter().map(|(p, _)| p).collect(), latest_mtime })
}

/// Build all session groups from the project's session directory.
pub fn discover_session_groups(project_path: &Path) -> Vec<SessionGroup> {
  let files = collect_session_files(project_path);

  let mut slug_groups: HashMap<String, Vec<(PathBuf, SystemTime)>> = HashMap::default();
  for (path, mtime, slug) in files {
    slug_groups.entry(slug).or_default().push((path, mtime));
  }

  let mut groups: Vec<SessionGroup> = slug_groups
    .into_iter()
    .filter_map(|(slug, entries)| build_session_group(slug, entries))
    .collect();

  groups.sort_by(|a, b| b.latest_mtime.cmp(&a.latest_mtime));
  groups
}

/// Find all JSONL files belonging to a specific slug.
pub fn discover_session_group(project_path: &Path, slug: &str) -> Option<SessionGroup> {
  let files = collect_session_files(project_path);
  let matching = files.into_iter().filter(|(_, _, s)| s == slug).map(|(p, m, _)| (p, m)).collect();
  build_session_group(slug.to_string(), matching)
}

/// Discover the most recently active session group for a project.
pub fn discover_latest_session_group(project_path: &Path) -> Option<SessionGroup> {
  let files = collect_session_files(project_path);
  let (_, _, latest_slug) = files.iter().max_by_key(|(_, mtime, _)| mtime)?;
  let latest_slug = latest_slug.clone();
  let matching =
    files.into_iter().filter(|(_, _, s)| *s == latest_slug).map(|(p, m, _)| (p, m)).collect();
  build_session_group(latest_slug, matching)
}

pub fn format_relative_time(time: SystemTime) -> String {
  let secs = time.elapsed().unwrap_or_default().as_secs();
  match secs {
    0..60 => "just now".to_string(),
    60..3600 => format!("{}m ago", secs / 60),
    3600..86400 => format!("{}h ago", secs / 3600),
    _ => format!("{}d ago", secs / 86400),
  }
}

/// Parse all JSONL files in a slug group into a unified list of turns.
pub fn parse_session_group(group: &SessionGroup) -> Vec<Turn> {
  let mut acc = SessionAccumulator::default();
  // Process files oldest-first so turns are in chronological order.
  for path in group.files.iter().rev() {
    acc.ingest(path);
  }
  acc.into_turns()
}

// ---------------------------------------------------------------------------
// JSONL parsing
// ---------------------------------------------------------------------------

/// Accumulator for parsing one or more JSONL session files into turns.
#[derive(Default)]
struct SessionAccumulator {
  user_messages: Vec<UserMessage>,
  assistant_entries: Vec<AssistantResponse>,
  /// Map message_id -> index in assistant_entries for O(1) dedup lookups.
  assistant_index: HashMap<String, usize>,
}

impl SessionAccumulator {
  /// Parse a single JSONL file, appending results to the accumulators.
  fn ingest(&mut self, path: &Path) {
    let content = match std::fs::read_to_string(path) {
      Ok(c) => c,
      Err(_) => return,
    };

    for line in content.lines() {
      let entry: RawEntry = match serde_json::from_str(line) {
        Ok(e) => e,
        Err(_) => continue,
      };

      match entry.entry_type.as_str() {
        "user" => {
          if entry.is_meta {
            continue;
          }
          let Some(msg) = entry.message else { continue };
          if msg.role != "user" {
            continue;
          }
          let text = extract_user_text(&msg.content);
          if text.is_empty() {
            continue;
          }
          self.user_messages.push(UserMessage { text, timestamp: entry.timestamp });
        }
        "assistant" => {
          let Some(msg) = entry.message else { continue };
          if msg.role != "assistant" {
            continue;
          }
          let message_id = msg.id.unwrap_or_default();
          let text_blocks = extract_assistant_text_blocks(&msg.content);
          if text_blocks.is_empty() {
            continue;
          }
          // Deduplicate streaming chunks sharing the same message id.
          // Skip dedup for entries with no message id (empty string).
          if !message_id.is_empty() {
            if let Some(&idx) = self.assistant_index.get(&message_id) {
              let existing = &mut self.assistant_entries[idx];
              let existing_len: usize = existing.text_blocks.iter().map(|b| b.len()).sum();
              let new_len: usize = text_blocks.iter().map(|b| b.len()).sum();
              if new_len > existing_len {
                existing.text_blocks = text_blocks;
              }
              continue;
            }
            self.assistant_index.insert(message_id.clone(), self.assistant_entries.len());
          }
          self.assistant_entries.push(AssistantResponse { message_id, text_blocks });
        }
        _ => continue,
      }
    }
  }

  fn into_turns(self) -> Vec<Turn> {
    group_into_turns(self.user_messages, self.assistant_entries)
  }
}

pub fn parse_session(path: &Path) -> Vec<Turn> {
  let mut acc = SessionAccumulator::default();
  acc.ingest(path);
  acc.into_turns()
}

fn extract_user_text(content: &serde_json::Value) -> String {
  match content {
    serde_json::Value::String(s) => s.clone(),
    serde_json::Value::Array(arr) => {
      let mut texts = Vec::new();
      for item in arr {
        if let Some(obj) = item.as_object()
          && obj.get("type").and_then(|t| t.as_str()) == Some("text")
          && let Some(text) = obj.get("text").and_then(|t| t.as_str())
        {
          texts.push(text.to_string());
        }
      }
      texts.join("\n")
    }
    _ => String::default(),
  }
}

fn extract_assistant_text_blocks(content: &serde_json::Value) -> Vec<String> {
  let serde_json::Value::Array(arr) = content else {
    return Vec::new();
  };

  let mut blocks = Vec::new();
  for item in arr {
    if let Some(obj) = item.as_object()
      && obj.get("type").and_then(|t| t.as_str()) == Some("text")
      && let Some(text) = obj.get("text").and_then(|t| t.as_str())
      && !text.is_empty()
    {
      blocks.push(text.to_string());
    }
  }
  blocks
}

fn group_into_turns(
  user_messages: Vec<UserMessage>,
  assistant_entries: Vec<AssistantResponse>,
) -> Vec<Turn> {
  if user_messages.is_empty() {
    return Vec::new();
  }

  let mut turns = Vec::new();
  let mut assistant_iter = assistant_entries.into_iter().peekable();

  for (i, user) in user_messages.into_iter().enumerate() {
    let mut responses = Vec::new();
    if let Some(resp) = assistant_iter.next() {
      responses.push(resp);
    }
    turns.push(Turn { index: i, user, responses });
  }

  if let Some(last_turn) = turns.last_mut() {
    for resp in assistant_iter {
      last_turn.responses.push(resp);
    }
  }

  turns
}
