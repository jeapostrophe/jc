use crate::views::code_view::CodeView;
use gpui::*;
use std::path::{Path, PathBuf};

/// TodoView wraps a [`CodeView`] opened on the project's `TODO.md` file,
/// adding save functionality. All editor, file-watching, reload, dirty
/// tracking, and rendering behaviour is delegated to the inner CodeView.
pub struct TodoView {
  code_view: Entity<CodeView>,
  file_path: PathBuf,
}

impl TodoView {
  pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let file_path = project_path.join("TODO.md");
    let open_path = file_path.clone();
    let code_view = cx.new(|cx| {
      let mut cv = CodeView::new(window, cx);
      cv.open_file(open_path, window, cx);
      cv
    });

    Self { code_view, file_path }
  }

  pub fn file_path(&self) -> &Path {
    &self.file_path
  }

  pub fn editor_text(&self, cx: &App) -> String {
    self.code_view.read(cx).editor_text(cx)
  }

  pub fn save(&self, cx: &mut Context<Self>) {
    self.code_view.update(cx, |cv, cx| cv.save(cx));
  }

  pub fn scroll_to_line(&self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
    self.code_view.update(cx, |cv, cx| cv.scroll_to_line(line, window, cx));
  }
}

impl Render for TodoView {
  fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
    div().id("todo-view").size_full().child(self.code_view.clone())
  }
}

impl Focusable for TodoView {
  fn focus_handle(&self, cx: &App) -> FocusHandle {
    self.code_view.read(cx).focus_handle(cx)
  }
}
