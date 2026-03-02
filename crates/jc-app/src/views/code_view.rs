use gpui::*;
use gpui_component::input::{Input, InputState};
use std::path::{Path, PathBuf};

pub struct CodeView {
  editor: Entity<InputState>,
  current_file: Option<PathBuf>,
}

impl CodeView {
  pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
    let editor = cx.new(|cx| InputState::new(window, cx).code_editor("text").soft_wrap(false));
    Self { editor, current_file: None }
  }

  pub fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
    let language = language_for_extension(&path);
    let content = std::fs::read_to_string(&path).unwrap_or_else(|e| format!("Error: {e}"));
    self.editor.update(cx, |state, cx| {
      state.set_highlighter(language, cx);
      state.set_value(content, window, cx);
    });
    self.current_file = Some(path);
  }
}

impl Focusable for CodeView {
  fn focus_handle(&self, cx: &App) -> FocusHandle {
    self.editor.read(cx).focus_handle(cx)
  }
}

impl Render for CodeView {
  fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
    div().size_full().child(Input::new(&self.editor).h_full().appearance(false).bordered(false))
  }
}

fn language_for_extension(path: &Path) -> &'static str {
  match path.extension().and_then(|ext| ext.to_str()).unwrap_or("") {
    "rs" => "rust",
    "js" => "javascript",
    "ts" => "typescript",
    "tsx" => "tsx",
    "py" => "python",
    "rb" => "ruby",
    "go" => "go",
    "c" | "h" => "c",
    "cpp" | "cc" | "cxx" | "hpp" => "cpp",
    "java" => "java",
    "md" => "markdown",
    "toml" => "toml",
    "json" => "json",
    "yaml" | "yml" => "yaml",
    "html" => "html",
    "css" => "css",
    "sh" | "bash" | "zsh" => "bash",
    _ => "text",
  }
}
