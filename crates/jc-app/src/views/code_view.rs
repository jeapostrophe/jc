use crate::language::Language;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::{Input, InputEvent, InputState};
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
  dirty: bool,
  externally_modified: bool,
  _subscription: Subscription,
  _watcher: Option<notify::RecommendedWatcher>,
}

impl CodeView {
  pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let editor = cx.new(|cx| {
      InputState::new(window, cx)
        .code_editor(Language::default().name())
        .soft_wrap(true)
        .line_number(false)
    });

    let subscription = cx.subscribe(&editor, |this: &mut Self, _, event: &InputEvent, _cx| {
      if matches!(event, InputEvent::Change) {
        this.dirty = true;
      }
    });

    Self {
      editor,
      current_file: None,
      project_path,
      dirty: false,
      externally_modified: false,
      _subscription: subscription,
      _watcher: None,
    }
  }

  pub fn file_path(&self) -> Option<&Path> {
    self.current_file.as_deref()
  }

  pub fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
    self.setup_watcher(&path, window, cx);
    self.current_file = Some(path);
    self.load_current(window, cx);
  }

  fn setup_watcher(&mut self, path: &Path, window: &Window, cx: &mut Context<Self>) {
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

    cx.spawn_in(window, async move |this: WeakEntity<CodeView>, cx: &mut AsyncWindowContext| {
      while notify_rx.recv_async().await.is_ok() {
        while notify_rx.try_recv().is_ok() {}
        let _ = this.update_in(cx, |view, window, cx| {
          if view.dirty {
            view.externally_modified = true;
            cx.notify();
          } else {
            view.load_current(window, cx);
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
    let language = Language::from_path(path);
    self.editor.update(cx, |state, cx| {
      state.set_highlighter(language.name(), cx);
      state.set_value(content, window, cx);
    });
    self.dirty = false;
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

impl CodeView {
  pub fn editor_text(&self, cx: &App) -> String {
    self.editor.read(cx).value().as_ref().to_string()
  }

  pub fn current_language(&self) -> Language {
    self.current_file.as_deref().map(Language::from_path).unwrap_or_default()
  }

  pub fn scroll_to_line(&self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
    // Estimate half the viewport in lines for centering.
    let line_height = window.line_height();
    let viewport_height = window.viewport_size().height;
    let half_viewport_lines =
      if line_height > px(0.) { (viewport_height / line_height / 2.0).floor() as u32 } else { 15 };

    // Position cursor above the target so the viewport scrolls to show that line
    // at the top. After the next layout, the target line will be roughly centered.
    let pre_line = line.saturating_sub(half_viewport_lines);
    self.editor.update(cx, |editor, cx| {
      editor.set_cursor_position(gpui_component::input::Position::new(pre_line, 0), window, cx);
    });

    // On the next frame, move the cursor to the actual target line. Since it is
    // already visible in the viewport, the editor will not adjust the scroll
    // offset, leaving the target approximately centered.
    let editor = self.editor.clone();
    cx.spawn_in(window, async move |_this: WeakEntity<CodeView>, cx: &mut AsyncWindowContext| {
      let _ = editor.update_in(cx, |editor, window, cx| {
        editor.set_cursor_position(gpui_component::input::Position::new(line, 0), window, cx);
      });
    })
    .detach();
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
