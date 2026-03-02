use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

pub struct OutlineItem {
  pub label: String,
  pub name: String,
  pub line: u32,
  pub depth: usize,
  pub byte_range: std::ops::Range<usize>,
}

pub fn compute_outline(text: &str, language_name: &str) -> Vec<OutlineItem> {
  let Some((ts_language, query_source)) = language_and_query(language_name) else {
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

  // Compute depth via range containment: an item is nested in the nearest
  // prior item whose range fully contains it.
  let mut items: Vec<OutlineItem> = Vec::with_capacity(raw_items.len());
  for i in 0..raw_items.len() {
    let mut depth = 0usize;
    for j in (0..i).rev() {
      if raw_items[j].3.start <= raw_items[i].3.start && raw_items[j].3.end >= raw_items[i].3.end {
        depth = items[j].depth + 1;
        break;
      }
    }

    let (ref name, ref context, line, ref range) = raw_items[i];
    let label = if context.is_empty() { name.clone() } else { format!("{context} {name}") };

    items.push(OutlineItem {
      label,
      name: name.clone(),
      line,
      depth,
      byte_range: range.clone(),
    });
  }

  items
}

fn language_and_query(name: &str) -> Option<(Language, &'static str)> {
  match name {
    "rust" => Some((tree_sitter_rust::LANGUAGE.into(), include_str!("outline_queries/rust.scm"))),
    "markdown" => {
      Some((tree_sitter_md::LANGUAGE.into(), include_str!("outline_queries/markdown.scm")))
    }
    "python" => {
      Some((tree_sitter_python::LANGUAGE.into(), include_str!("outline_queries/python.scm")))
    }
    "go" => Some((tree_sitter_go::LANGUAGE.into(), include_str!("outline_queries/go.scm"))),
    "javascript" => Some((
      tree_sitter_javascript::LANGUAGE.into(),
      include_str!("outline_queries/javascript.scm"),
    )),
    "typescript" => Some((
      tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
      include_str!("outline_queries/typescript.scm"),
    )),
    "tsx" => Some((
      tree_sitter_typescript::LANGUAGE_TSX.into(),
      include_str!("outline_queries/javascript.scm"),
    )),
    _ => None,
  }
}
