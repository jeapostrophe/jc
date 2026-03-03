use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::{Input, InputEvent, InputState};
use notify::{EventKind, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

actions!(todo, [ReloadTodoFromDisk]);

pub fn init(cx: &mut App) {
  cx.bind_keys([KeyBinding::new("cmd-r", ReloadTodoFromDisk, Some("TodoView"))]);
}

pub struct TodoView {
  editor: Entity<InputState>,
  file_path: PathBuf,
  dirty: bool,
  externally_modified: bool,
  saving: Arc<AtomicBool>,
  _subscription: Subscription,
  _watcher: Option<notify::RecommendedWatcher>,
}

impl TodoView {
  pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let editor = cx.new(|cx| {
      InputState::new(window, cx).code_editor("markdown").soft_wrap(true).line_number(false)
    });

    let subscription = cx.subscribe(&editor, |this: &mut Self, _, event: &InputEvent, _cx| {
      if matches!(event, InputEvent::Change) {
        this.dirty = true;
      }
    });

    let file_path = project_path.join("TODO.md");
    let saving = Arc::new(AtomicBool::new(false));

    // Set up file watcher on the parent directory (more reliable on macOS)
    let watcher = if let Some(parent) = file_path.parent() {
      let parent = parent.to_path_buf();
      let saving_clone = saving.clone();
      let (notify_tx, notify_rx) = flume::unbounded::<()>();

      let mut watcher =
        notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
          if let Ok(event) = res {
            match event.kind {
              EventKind::Modify(_) | EventKind::Create(_) => {
                if event.paths.iter().any(|p| p.ends_with("TODO.md"))
                  && !saving_clone.load(Ordering::Relaxed)
                {
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

      // Bridge notify events to GPUI
      cx.spawn(async move |this: WeakEntity<TodoView>, cx: &mut AsyncApp| {
        while notify_rx.recv_async().await.is_ok() {
          // Drain any additional queued notifications
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

      watcher
    } else {
      None
    };

    let mut view = Self {
      editor,
      file_path,
      dirty: false,
      externally_modified: false,
      saving,
      _subscription: subscription,
      _watcher: watcher,
    };
    view.load(window, cx);
    view
  }

  pub fn file_path(&self) -> &Path {
    &self.file_path
  }

  pub fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let content = std::fs::read_to_string(&self.file_path).unwrap_or_default();
    self.editor.update(cx, |state, cx| {
      state.set_value(content, window, cx);
    });
    self.dirty = false;
    self.externally_modified = false;
  }

  pub fn save(&mut self, cx: &mut Context<Self>) {
    self.saving.store(true, Ordering::Relaxed);
    let content = self.editor.read(cx).value();
    if let Err(e) = std::fs::write(&self.file_path, content.as_ref()) {
      eprintln!("Failed to save TODO.md: {e}");
    }
    self.dirty = false;

    // Clear saving flag after a brief delay to suppress self-triggered watcher event
    let saving = self.saving.clone();
    cx.spawn(async move |_this: WeakEntity<TodoView>, _cx: &mut AsyncApp| {
      Timer::after(std::time::Duration::from_millis(200)).await;
      saving.store(false, Ordering::Relaxed);
    })
    .detach();
  }

  fn reload_from_disk(
    &mut self,
    _: &ReloadTodoFromDisk,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.load(window, cx);
    cx.notify();
  }
}

impl TodoView {
  pub fn editor_text(&self, cx: &App) -> String {
    self.editor.read(cx).value().as_ref().to_string()
  }

  pub fn scroll_to_line(&self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
    self.editor.update(cx, |editor, cx| {
      editor.set_cursor_position(gpui_component::input::Position::new(line, 0), window, cx);
    });
  }
}

impl Render for TodoView {
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
      .id("todo-view")
      .key_context("TodoView")
      .track_focus(&self.editor.read(cx).focus_handle(cx))
      .size_full()
      .font_family("Lilex")
      .on_action(cx.listener(Self::reload_from_disk))
      .child(notification)
      .child(Input::new(&self.editor).h_full().appearance(false).bordered(false))
  }
}

impl Focusable for TodoView {
  fn focus_handle(&self, cx: &App) -> FocusHandle {
    self.editor.read(cx).focus_handle(cx)
  }
}
