use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    if first_line.len() > 80 {
      // Find a char boundary at or before byte 80 to avoid panicking on multi-byte UTF-8.
      let truncate_at = first_line
        .char_indices()
        .take_while(|(i, _)| *i <= 80)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(first_line.len());
      let mut label = first_line[..truncate_at].to_string();
      label.push_str("...");
      label
    } else {
      first_line.to_string()
    }
  }
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

pub fn discover_latest_session(project_path: &Path) -> Option<(String, PathBuf)> {
  let dir = session_dir(project_path);
  let entries = std::fs::read_dir(&dir).ok()?;

  let (path, _) = entries
    .filter_map(|e| e.ok())
    .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
    .filter_map(|e| {
      let mtime = e.metadata().ok()?.modified().ok()?;
      Some((e.path(), mtime))
    })
    .max_by_key(|(_, mtime)| *mtime)?;
  let session_id = path.file_stem()?.to_string_lossy().to_string();
  Some((session_id, path))
}

// ---------------------------------------------------------------------------
// JSONL parsing
// ---------------------------------------------------------------------------

pub fn parse_session(path: &Path) -> Vec<Turn> {
  let content = match std::fs::read_to_string(path) {
    Ok(c) => c,
    Err(_) => return Vec::new(),
  };

  let mut user_messages: Vec<UserMessage> = Vec::new();
  let mut assistant_entries: Vec<(String, AssistantResponse)> = Vec::new();
  // Map message_id -> index in assistant_entries for O(1) dedup lookups.
  let mut assistant_index: HashMap<String, usize> = HashMap::default();

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
        // Content can be a plain string or an array of content blocks.
        let text = extract_user_text(&msg.content);
        if text.is_empty() {
          continue;
        }
        user_messages.push(UserMessage { text, timestamp: entry.timestamp });
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
          if let Some(&idx) = assistant_index.get(&message_id) {
            let existing = &mut assistant_entries[idx].1;
            let existing_len: usize = existing.text_blocks.iter().map(|b| b.len()).sum();
            let new_len: usize = text_blocks.iter().map(|b| b.len()).sum();
            if new_len > existing_len {
              existing.text_blocks = text_blocks;
            }
            continue;
          }
          assistant_index.insert(message_id.clone(), assistant_entries.len());
        }
        assistant_entries.push((message_id.clone(), AssistantResponse { message_id, text_blocks }));
      }
      _ => continue,
    }
  }

  // Group into turns: each user message starts a new turn, followed by
  // assistant responses until the next user message.
  // We assign assistant responses based on ordering in the JSONL file.
  group_into_turns(user_messages, assistant_entries)
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
  assistant_entries: Vec<(String, AssistantResponse)>,
) -> Vec<Turn> {
  if user_messages.is_empty() {
    return Vec::new();
  }

  let mut turns = Vec::new();
  let mut assistant_iter = assistant_entries.into_iter().peekable();

  for (i, user) in user_messages.into_iter().enumerate() {
    let mut responses = Vec::new();
    // Take one assistant response per user message. The last user message
    // gets all remaining responses (appended below).
    if let Some((_, resp)) = assistant_iter.next() {
      responses.push(resp);
    }
    turns.push(Turn { index: i, user, responses });
  }

  // If there are leftover assistant responses, append them to the last turn.
  if let Some(last_turn) = turns.last_mut() {
    for (_, resp) in assistant_iter {
      last_turn.responses.push(resp);
    }
  }

  turns
}
