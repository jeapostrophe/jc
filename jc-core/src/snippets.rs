use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct Snippet {
  pub heading: String,
  pub content: String,
}

#[derive(Debug, Default, Clone)]
pub struct SnippetDocument {
  pub items: Vec<Snippet>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn snippet_file_path() -> PathBuf {
  dirs::home_dir().unwrap_or_default().join(".claude").join("jc.md")
}

pub fn ensure_file_exists() {
  let path = snippet_file_path();
  if let Some(parent) = path.parent() {
    let _ = std::fs::create_dir_all(parent);
  }
  if !path.exists() {
    let _ = std::fs::File::create(&path);
  }
}

pub fn load() -> SnippetDocument {
  let path = snippet_file_path();
  match std::fs::read_to_string(&path) {
    Ok(text) => parse(&text),
    Err(_) => SnippetDocument::default(),
  }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

pub fn parse(text: &str) -> SnippetDocument {
  let mut doc = SnippetDocument::default();
  let mut current_heading: Option<String> = None;
  let mut current_lines: Vec<&str> = Vec::new();

  for line in text.lines() {
    if let Some(after) = line.strip_prefix("# ") {
      // Finalize previous snippet.
      if let Some(heading) = current_heading.take() {
        let content = current_lines.join("\n").trim().to_string();
        doc.items.push(Snippet { heading, content });
        current_lines.clear();
      }
      current_heading = Some(after.to_string());
    } else if current_heading.is_some() {
      current_lines.push(line);
    }
  }

  // Finalize last snippet.
  if let Some(heading) = current_heading.take() {
    let content = current_lines.join("\n").trim().to_string();
    doc.items.push(Snippet { heading, content });
  }

  doc
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
    assert!(doc.items.is_empty());
  }

  #[test]
  fn single_snippet() {
    let text = "# Greeting\nHello, world!\n";
    let doc = parse(text);
    assert_eq!(doc.items.len(), 1);
    assert_eq!(doc.items[0].heading, "Greeting");
    assert_eq!(doc.items[0].content, "Hello, world!");
  }

  #[test]
  fn multiple_snippets() {
    let text = "\
# First
content one
# Second
content two
line two
# Third
content three
";
    let doc = parse(text);
    assert_eq!(doc.items.len(), 3);
    assert_eq!(doc.items[0].heading, "First");
    assert_eq!(doc.items[0].content, "content one");
    assert_eq!(doc.items[1].heading, "Second");
    assert_eq!(doc.items[1].content, "content two\nline two");
    assert_eq!(doc.items[2].heading, "Third");
    assert_eq!(doc.items[2].content, "content three");
  }

  #[test]
  fn content_before_first_heading_ignored() {
    let text = "preamble text\n# Actual\nbody\n";
    let doc = parse(text);
    assert_eq!(doc.items.len(), 1);
    assert_eq!(doc.items[0].heading, "Actual");
    assert_eq!(doc.items[0].content, "body");
  }

  #[test]
  fn snippet_with_empty_content() {
    let text = "# Empty\n# Next\nhas content\n";
    let doc = parse(text);
    assert_eq!(doc.items.len(), 2);
    assert_eq!(doc.items[0].heading, "Empty");
    assert_eq!(doc.items[0].content, "");
    assert_eq!(doc.items[1].heading, "Next");
    assert_eq!(doc.items[1].content, "has content");
  }

  #[test]
  fn content_trimmed() {
    let text = "# Padded\n\n  body text  \n\n";
    let doc = parse(text);
    assert_eq!(doc.items[0].content, "body text");
  }

  #[test]
  fn snippet_file_path_is_under_claude_dir() {
    let path = snippet_file_path();
    assert!(path.ends_with(".claude/jc.md"));
  }
}
