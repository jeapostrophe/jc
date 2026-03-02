use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::{Input, InputState};
use notify::{EventKind, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};

actions!(code_view, [ReloadCodeFromDisk]);

pub fn init(cx: &mut App) {
  cx.bind_keys([KeyBinding::new("cmd-r", ReloadCodeFromDisk, Some("CodeView"))]);
}

pub struct CodeView {
  editor: Entity<InputState>,
  current_file: Option<PathBuf>,
  project_path: PathBuf,
  externally_modified: bool,
  _watcher: Option<notify::RecommendedWatcher>,
}

impl CodeView {
  pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let editor = cx.new(|cx| {
      InputState::new(window, cx).code_editor("text").soft_wrap(false).line_number(false)
    });
    Self { editor, current_file: None, project_path, externally_modified: false, _watcher: None }
  }

  pub fn file_path(&self) -> Option<&Path> {
    self.current_file.as_deref()
  }

  pub fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
    self.setup_watcher(&path, cx);
    self.current_file = Some(path);
    self.load_current(window, cx);
  }

  fn setup_watcher(&mut self, path: &Path, cx: &mut Context<Self>) {
    let watched_file = match path.file_name() {
      Some(f) => f.to_os_string(),
      None => return,
    };
    let parent = match path.parent() {
      Some(p) => p.to_path_buf(),
      None => return,
    };

    let (notify_tx, notify_rx) = flume::unbounded::<()>();

    let mut watcher =
      notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        if let Ok(event) = res {
          match event.kind {
            EventKind::Modify(_) | EventKind::Create(_) => {
              if event.paths.iter().any(|p| p.ends_with(&watched_file)) {
                let _ = notify_tx.send(());
              }
            }
            _ => {}
          }
        }
      })
      .ok();

    if let Some(ref mut w) = watcher {
      let _ = w.watch(&parent, RecursiveMode::NonRecursive);
    }

    cx.spawn(async move |this: WeakEntity<CodeView>, cx: &mut AsyncApp| {
      while notify_rx.recv_async().await.is_ok() {
        while notify_rx.try_recv().is_ok() {}
        let _ = cx.update(|cx| {
          if let Some(entity) = this.upgrade() {
            entity.update(cx, |view, cx| {
              view.externally_modified = true;
              cx.notify();
            });
          }
        });
      }
    })
    .detach();

    self._watcher = watcher;
  }

  fn load_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(path) = self.current_file.as_ref() else { return };
    let content = std::fs::read_to_string(path).unwrap_or_else(|e| format!("Error: {e}"));
    let language = language_for_extension(path);
    self.editor.update(cx, |state, cx| {
      state.set_highlighter(language, cx);
      state.set_value(content, window, cx);
    });
    self.externally_modified = false;
    cx.notify();
  }

  fn reload_from_disk(
    &mut self,
    _: &ReloadCodeFromDisk,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.load_current(window, cx);
  }
}

impl Focusable for CodeView {
  fn focus_handle(&self, cx: &App) -> FocusHandle {
    self.editor.read(cx).focus_handle(cx)
  }
}

impl Render for CodeView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();

    let notification = if self.externally_modified {
      div()
        .px_2()
        .py_1()
        .bg(theme.warning)
        .text_sm()
        .text_color(theme.warning_foreground)
        .child("File changed on disk \u{2014} press Cmd-R to reload")
        .into_any_element()
    } else {
      div().into_any_element()
    };

    div()
      .id("code-view")
      .key_context("CodeView")
      .track_focus(&self.editor.read(cx).focus_handle(cx))
      .size_full()
      .font_family("Menlo")
      .on_action(cx.listener(Self::reload_from_disk))
      .child(notification)
      .child(Input::new(&self.editor).h_full().appearance(false).bordered(false))
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
