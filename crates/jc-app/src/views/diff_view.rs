use std::path::{Path, PathBuf};

use git2::DiffFormat;
use gpui::*;
use gpui_component::input::{Input, InputState};

pub struct DiffView {
  editor: Entity<InputState>,
  project_path: PathBuf,
}

impl DiffView {
  pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let editor = cx.new(|cx| InputState::new(window, cx).code_editor("diff").soft_wrap(false));
    let mut view = Self { editor, project_path };
    view.refresh(window, cx);
    view
  }

  pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let diff_text = generate_diff(&self.project_path);
    self.editor.update(cx, |state, cx| {
      state.set_value(diff_text, window, cx);
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
      .child(Input::new(&self.editor).h_full().appearance(false).bordered(false).disabled(true))
  }
}

fn generate_diff(path: &Path) -> String {
  match generate_diff_inner(path) {
    Ok(diff) => diff,
    Err(e) => format!("Error generating diff: {e}"),
  }
}

fn generate_diff_inner(path: &Path) -> Result<String, git2::Error> {
  let repo = git2::Repository::open(path)?;
  let head = repo.head()?;
  let tree = head.peel_to_tree()?;
  let diff = repo.diff_tree_to_workdir_with_index(Some(&tree), None)?;

  let mut output = String::new();
  diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
    let prefix = match line.origin() {
      '+' | '-' | ' ' => {
        output.push(line.origin());
        ""
      }
      _ => "",
    };
    output.push_str(prefix);
    if let Ok(content) = std::str::from_utf8(line.content()) {
      output.push_str(content);
    }
    true
  })?;

  Ok(output)
}
