use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::{Input, InputState};
use jc_core::session::{Turn, discover_latest_session, parse_session};
use notify::{EventKind, RecursiveMode, Watcher};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

pub fn init(_cx: &mut App) {}

pub struct ReplyView {
  editor: Entity<InputState>,
  project_path: PathBuf,
  session_path: Option<PathBuf>,
  turns: Vec<Turn>,
  current_turn_index: usize,
  last_written_hash: u64,
  _watcher: Option<notify::RecommendedWatcher>,
}

impl ReplyView {
  pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let editor = cx.new(|cx| {
      InputState::new(window, cx).code_editor("markdown").soft_wrap(true).line_number(false)
    });

    let replies_dir = project_path.join(".jc/replies");
    let _ = std::fs::create_dir_all(&replies_dir);

    let mut view = Self {
      editor,
      project_path,
      session_path: None,
      turns: Vec::new(),
      current_turn_index: 0,
      last_written_hash: 0,
      _watcher: None,
    };
    view.discover_and_parse();
    view.show_current_turn(window, cx);
    view.setup_watcher(window, cx);
    view
  }

  fn discover_and_parse(&mut self) {
    if let Some((_session_id, path)) = discover_latest_session(&self.project_path) {
      self.turns = parse_session(&path);
      self.session_path = Some(path);
      if !self.turns.is_empty() {
        self.current_turn_index = self.turns.len() - 1;
      }
    } else {
      self.turns.clear();
      self.session_path = None;
      self.current_turn_index = 0;
    }
  }

  pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let prev_turn_count = self.turns.len();
    let was_at_latest = self.current_turn_index + 1 >= prev_turn_count || prev_turn_count == 0;

    self.discover_and_parse();

    // If the user was viewing the latest turn, follow it.
    if was_at_latest && !self.turns.is_empty() {
      self.current_turn_index = self.turns.len() - 1;
    } else if self.current_turn_index >= self.turns.len() {
      self.current_turn_index = self.turns.len().saturating_sub(1);
    }

    self.show_current_turn(window, cx);
  }

  fn show_current_turn(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let content = if self.turns.is_empty() {
      String::default()
    } else {
      let turn = &self.turns[self.current_turn_index];
      let md = turn.render_markdown();
      self.write_turn_file(&md);
      md
    };
    self.editor.update(cx, |state, cx| {
      state.set_value(content, window, cx);
    });
  }

  pub fn set_turn_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
    if index < self.turns.len() {
      self.current_turn_index = index;
      self.show_current_turn(window, cx);
    }
  }

  fn setup_watcher(&mut self, window: &Window, cx: &mut Context<Self>) {
    let Some(session_path) = &self.session_path else { return };
    let watched_file = match session_path.file_name() {
      Some(f) => f.to_os_string(),
      None => return,
    };
    let parent = match session_path.parent() {
      Some(p) => p.to_path_buf(),
      None => return,
    };

    let (notify_tx, notify_rx) = flume::unbounded::<()>();

    let mut watcher =
      notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        if let Ok(event) = res
          && matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_))
          && event.paths.iter().any(|p| p.ends_with(&watched_file))
        {
          let _ = notify_tx.send(());
        }
      })
      .ok();

    if let Some(ref mut w) = watcher {
      let _ = w.watch(&parent, RecursiveMode::NonRecursive);
    }

    cx.spawn_in(window, async move |this: WeakEntity<ReplyView>, cx: &mut AsyncWindowContext| {
      while notify_rx.recv_async().await.is_ok() {
        // Drain queued events.
        while notify_rx.try_recv().is_ok() {}
        let _ = this.update_in(cx, |view, window, cx| {
          view.refresh(window, cx);
        });
      }
    })
    .detach();

    self._watcher = watcher;
  }

  fn write_turn_file(&mut self, content: &str) {
    let mut hasher = DefaultHasher::default();
    content.hash(&mut hasher);
    let hash = hasher.finish();
    if hash == self.last_written_hash {
      return;
    }
    self.last_written_hash = hash;
    let replies_dir = self.project_path.join(".jc/replies");
    let filename = format!("turn_{:04}.md", self.current_turn_index);
    let _ = std::fs::write(replies_dir.join(filename), content);
  }
}

impl ReplyView {
  pub fn turns(&self) -> &[Turn] {
    &self.turns
  }

  pub fn current_turn_index(&self) -> usize {
    self.current_turn_index
  }

  pub fn current_turn_label(&self) -> String {
    if self.turns.is_empty() {
      return "No session".to_string();
    }
    self.turns[self.current_turn_index].label()
  }

  pub fn editor_text(&self, cx: &App) -> String {
    self.editor.read(cx).value().as_ref().to_string()
  }

  pub fn scroll_to_line(&self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
    let line_height = window.line_height();
    let viewport_height = window.viewport_size().height;
    let half_viewport_lines =
      if line_height > px(0.) { (viewport_height / line_height / 2.0).floor() as u32 } else { 15 };

    let pre_line = line.saturating_sub(half_viewport_lines);
    self.editor.update(cx, |editor, cx| {
      editor.set_cursor_position(gpui_component::input::Position::new(pre_line, 0), window, cx);
    });

    let editor = self.editor.clone();
    cx.spawn_in(window, async move |_this: WeakEntity<ReplyView>, cx: &mut AsyncWindowContext| {
      let _ = editor.update_in(cx, |editor, window, cx| {
        editor.set_cursor_position(gpui_component::input::Position::new(line, 0), window, cx);
      });
    })
    .detach();
  }

  pub fn project_path(&self) -> &Path {
    &self.project_path
  }
}

impl Focusable for ReplyView {
  fn focus_handle(&self, cx: &App) -> FocusHandle {
    self.editor.read(cx).focus_handle(cx)
  }
}

impl Render for ReplyView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();

    if self.turns.is_empty() {
      return div()
        .size_full()
        .key_context("ReplyView")
        .flex()
        .items_center()
        .justify_center()
        .child(div().text_color(theme.muted_foreground).child("No session found"));
    }

    div()
      .size_full()
      .key_context("ReplyView")
      .font_family("Lilex")
      .child(Input::new(&self.editor).h_full().appearance(false).bordered(false).disabled(true))
  }
}
