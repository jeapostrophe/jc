use std::path::{Path, PathBuf};

use git2::DiffFormat;
use gpui::*;
use gpui_component::input::{Input, InputState, Position};

pub struct DiffView {
  editor: Entity<InputState>,
  project_path: PathBuf,
  file_entries: Vec<(String, u32)>,
}

impl DiffView {
  pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let editor = cx
      .new(|cx| InputState::new(window, cx).code_editor("diff").soft_wrap(true).line_number(false));
    let mut view = Self { editor, project_path, file_entries: Vec::new() };
    view.refresh(window, cx);
    view
  }

  pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let diff_text = generate_diff(&self.project_path);

    self.file_entries = parse_file_entries(&diff_text);

    self.editor.update(cx, |state, cx| {
      state.set_value(diff_text, window, cx);
    });
  }
}

impl DiffView {
  pub fn file_entries(&self) -> &[(String, u32)] {
    &self.file_entries
  }

  pub fn scroll_to_line(&self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
    self.editor.update(cx, |editor, cx| {
      editor.set_cursor_position(Position::new(line, 0), window, cx);
    });
  }
}

impl Focusable for DiffView {
  fn focus_handle(&self, cx: &App) -> FocusHandle {
    self.editor.read(cx).focus_handle(cx)
  }
}

impl Render for DiffView {
  fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
    div()
      .size_full()
      .font_family("Menlo")
      .child(Input::new(&self.editor).h_full().appearance(false).bordered(false).disabled(true))
  }
}

fn generate_diff(path: &Path) -> String {
  match generate_diff_inner(path) {
    Ok(result) => result,
    Err(e) => format!("Error generating diff: {e}"),
  }
}

fn generate_diff_inner(path: &Path) -> Result<String, git2::Error> {
  let repo = git2::Repository::open(path)?;
  let head = repo.head()?;
  let tree = head.peel_to_tree()?;
  let diff = repo.diff_tree_to_workdir_with_index(Some(&tree), None)?;

  let mut output = String::default();
  diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
    match line.origin() {
      '+' | '-' | ' ' => output.push(line.origin()),
      _ => {}
    }
    let content = std::str::from_utf8(line.content()).unwrap_or("");
    output.push_str(content);
    true
  })?;

  Ok(output)
}

fn parse_file_entries(diff_text: &str) -> Vec<(String, u32)> {
  let mut entries = Vec::new();
  for (line_num, line) in diff_text.lines().enumerate() {
    if let Some(rest) = line.strip_prefix("diff --git a/") {
      let name = rest.split(" b/").next().unwrap_or(rest);
      entries.push((name.to_string(), line_num as u32));
    }
  }
  entries
}
