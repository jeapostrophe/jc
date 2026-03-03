use crate::language::Language;
use tree_sitter::{Language as TsLanguage, Parser, Query, QueryCursor, StreamingIterator};

pub struct OutlineItem {
  pub label: String,
  pub name: String,
  pub context: String,
  pub line: u32,
  pub depth: usize,
  pub parent: Option<usize>,
  pub byte_range: std::ops::Range<usize>,
}

pub fn compute_outline(text: &str, language: Language) -> Vec<OutlineItem> {
  let Some((ts_language, query_source)) = language_and_query(language) else {
    return Vec::new();
  };

  let mut parser = Parser::new();
  if parser.set_language(&ts_language).is_err() {
    return Vec::new();
  }

  let Some(tree) = parser.parse(text, None) else {
    return Vec::new();
  };

  let Ok(query) = Query::new(&ts_language, query_source) else {
    return Vec::new();
  };

  let name_idx = query.capture_index_for_name("name");
  let context_idx = query.capture_index_for_name("context");
  let item_idx = query.capture_index_for_name("item");

  let mut cursor = QueryCursor::new();
  let mut matches = cursor.matches(&query, tree.root_node(), text.as_bytes());

  let mut raw_items: Vec<(String, String, u32, std::ops::Range<usize>)> = Vec::new();

  while let Some(m) = {
    matches.advance();
    matches.get()
  } {
    let mut name_parts = Vec::new();
    let mut context_parts = Vec::new();
    let mut item_range: Option<std::ops::Range<usize>> = None;
    let mut item_line: Option<u32> = None;

    for capture in m.captures {
      let text_slice = capture.node.utf8_text(text.as_bytes()).unwrap_or("");
      if Some(capture.index) == item_idx {
        item_range = Some(capture.node.byte_range());
        item_line = Some(capture.node.start_position().row as u32);
      } else if Some(capture.index) == name_idx {
        name_parts.push(text_slice.to_string());
      } else if Some(capture.index) == context_idx {
        context_parts.push(text_slice.to_string());
      }
    }

    let Some(range) = item_range else { continue };
    let line = item_line.unwrap_or(0);
    let name = name_parts.join(" ");
    let context = context_parts.join(" ");

    if !name.is_empty() {
      raw_items.push((name, context, line, range));
    }
  }

  // Compute depth and parent via a stack. Items arrive in document order,
  // so we maintain a stack of ancestors whose ranges contain the current item.
  let mut items: Vec<OutlineItem> = Vec::with_capacity(raw_items.len());
  let mut stack: Vec<usize> = Vec::new(); // indices into `items`

  for (i, (name, context, line, range)) in raw_items.iter().enumerate() {
    // Pop stack entries that don't contain the current item.
    while let Some(&top) = stack.last() {
      if items[top].byte_range.end < range.end {
        stack.pop();
      } else {
        break;
      }
    }

    let parent = stack.last().copied();
    let depth = stack.len();
    let label = if context.is_empty() { name.clone() } else { format!("{context} {name}") };

    items.push(OutlineItem {
      label,
      name: name.clone(),
      context: context.clone(),
      line: *line,
      depth,
      parent,
      byte_range: range.clone(),
    });

    stack.push(i);
  }

  items
}

fn language_and_query(language: Language) -> Option<(TsLanguage, &'static str)> {
  match language {
    Language::Rust => {
      Some((tree_sitter_rust::LANGUAGE.into(), include_str!("outline_queries/rust.scm")))
    }
    Language::Markdown => {
      Some((tree_sitter_md::LANGUAGE.into(), include_str!("outline_queries/markdown.scm")))
    }
    Language::Python => {
      Some((tree_sitter_python::LANGUAGE.into(), include_str!("outline_queries/python.scm")))
    }
    Language::Go => Some((tree_sitter_go::LANGUAGE.into(), include_str!("outline_queries/go.scm"))),
    Language::JavaScript => Some((
      tree_sitter_javascript::LANGUAGE.into(),
      include_str!("outline_queries/javascript.scm"),
    )),
    Language::TypeScript => Some((
      tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
      include_str!("outline_queries/typescript.scm"),
    )),
    Language::Tsx => Some((
      tree_sitter_typescript::LANGUAGE_TSX.into(),
      include_str!("outline_queries/javascript.scm"),
    )),
    _ => None,
  }
}
