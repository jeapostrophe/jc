use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::{Input, InputEvent, InputState};
use std::path::{Path, PathBuf};

use crate::views::code_view::CodeView;

actions!(picker, [ConfirmPicker, CancelPicker, SelectNextItem, SelectPrevItem, OpenFilePicker]);

pub fn init(cx: &mut App) {
  cx.bind_keys([
    KeyBinding::new("enter", ConfirmPicker, Some("Picker")),
    KeyBinding::new("escape", CancelPicker, Some("Picker")),
    KeyBinding::new("ctrl-c", CancelPicker, Some("Picker")),
    KeyBinding::new("down", SelectNextItem, Some("Picker")),
    KeyBinding::new("ctrl-n", SelectNextItem, Some("Picker")),
    KeyBinding::new("up", SelectPrevItem, Some("Picker")),
    KeyBinding::new("ctrl-p", SelectPrevItem, Some("Picker")),
    KeyBinding::new("cmd-o", OpenFilePicker, Some("Workspace")),
  ]);
}

const MAX_VISIBLE_RESULTS: usize = 200;

fn fuzzy_match(query_lower: &[char], candidate: &str) -> Option<i64> {
  if query_lower.is_empty() {
    return Some(0);
  }

  let mut qi = 0;
  let mut score: i64 = 0;
  let mut prev_match = false;
  let mut prev_char = '/';

  for (ci, ch) in candidate.chars().enumerate() {
    let ch_lower = ch.to_lowercase().next().unwrap_or(ch);
    if qi < query_lower.len() && ch_lower == query_lower[qi] {
      score += 1;
      if prev_match {
        score += 5;
      }
      if ci == 0 || prev_char == '/' {
        score += 10;
      }
      if ci < 5 {
        score += 3;
      }
      qi += 1;
      prev_match = true;
    } else {
      prev_match = false;
    }
    prev_char = ch;
  }

  if qi == query_lower.len() { Some(score) } else { None }
}

pub trait PickerDelegate: 'static {
  fn items(&self) -> &[String];
  fn confirm(&mut self, selected: &str, window: &mut Window, cx: &mut Context<PickerState<Self>>)
  where
    Self: Sized;
  fn dismiss(&mut self, window: &mut Window, cx: &mut Context<PickerState<Self>>)
  where
    Self: Sized;
}

pub enum PickerEvent {
  Confirmed,
  Dismissed,
}

impl<D: PickerDelegate> EventEmitter<PickerEvent> for PickerState<D> {}

struct FilteredItem {
  index: usize,
  score: i64,
}

pub struct PickerState<D: PickerDelegate> {
  delegate: D,
  query_input: Entity<InputState>,
  filtered: Vec<FilteredItem>,
  selected_index: usize,
  focus: FocusHandle,
  scroll_handle: ScrollHandle,
  _subscription: Subscription,
}

impl<D: PickerDelegate> PickerState<D> {
  pub fn new(delegate: D, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let query_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search..."));

    let subscription = cx.subscribe(&query_input, |this: &mut Self, _, event: &InputEvent, cx| {
      if matches!(event, InputEvent::Change) {
        this.refilter(cx);
        cx.notify();
      }
    });

    let mut state = Self {
      delegate,
      query_input,
      filtered: Vec::new(),
      selected_index: 0,
      focus: cx.focus_handle(),
      scroll_handle: ScrollHandle::default(),
      _subscription: subscription,
    };
    state.refilter(cx);
    state
  }

  pub fn input_focus_handle(&self, cx: &App) -> FocusHandle {
    self.query_input.read(cx).focus_handle(cx)
  }

  fn refilter(&mut self, cx: &App) {
    let query = self.query_input.read(cx).value().as_ref().to_string();
    let query_lower: Vec<char> = query.chars().flat_map(|c| c.to_lowercase()).collect();
    self.filtered.clear();
    for (index, item) in self.delegate.items().iter().enumerate() {
      if let Some(score) = fuzzy_match(&query_lower, item) {
        self.filtered.push(FilteredItem { index, score });
      }
    }
    self.filtered.sort_by(|a, b| b.score.cmp(&a.score));
    self.selected_index = 0;
  }

  fn select_next(&mut self, _: &SelectNextItem, _window: &mut Window, cx: &mut Context<Self>) {
    if !self.filtered.is_empty() {
      self.selected_index = (self.selected_index + 1) % self.filtered.len();
      self.scroll_handle.scroll_to_item(self.selected_index);
      cx.notify();
    }
  }

  fn select_prev(&mut self, _: &SelectPrevItem, _window: &mut Window, cx: &mut Context<Self>) {
    if !self.filtered.is_empty() {
      self.selected_index =
        if self.selected_index == 0 { self.filtered.len() - 1 } else { self.selected_index - 1 };
      self.scroll_handle.scroll_to_item(self.selected_index);
      cx.notify();
    }
  }

  fn confirm_selected(&mut self, _: &ConfirmPicker, window: &mut Window, cx: &mut Context<Self>) {
    if let Some(item) = self.filtered.get(self.selected_index) {
      let selected = self.delegate.items()[item.index].clone();
      self.delegate.confirm(&selected, window, cx);
      cx.emit(PickerEvent::Confirmed);
    }
  }

  fn cancel(&mut self, _: &CancelPicker, window: &mut Window, cx: &mut Context<Self>) {
    self.delegate.dismiss(window, cx);
    cx.emit(PickerEvent::Dismissed);
  }
}

impl<D: PickerDelegate> Focusable for PickerState<D> {
  fn focus_handle(&self, _: &App) -> FocusHandle {
    self.focus.clone()
  }
}

impl<D: PickerDelegate> Render for PickerState<D> {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();

    let items = self.delegate.items();

    let results: Vec<Div> = self
      .filtered
      .iter()
      .enumerate()
      .take(MAX_VISIBLE_RESULTS)
      .map(|(i, fi)| {
        let selected = i == self.selected_index;
        let label = items[fi.index].clone();
        let row = div().px_2().py(px(3.0)).text_sm();
        let row = if selected {
          row.bg(theme.accent).text_color(theme.accent_foreground)
        } else {
          row.text_color(theme.foreground)
        };
        row.child(label)
      })
      .collect();

    div()
      .id("picker")
      .key_context("Picker")
      .track_focus(&self.focus)
      .on_action(cx.listener(Self::confirm_selected))
      .on_action(cx.listener(Self::cancel))
      .on_action(cx.listener(Self::select_next))
      .on_action(cx.listener(Self::select_prev))
      .w(px(500.0))
      .max_h(px(400.0))
      .bg(theme.background)
      .border_1()
      .border_color(theme.border)
      .rounded_md()
      .overflow_hidden()
      .flex()
      .flex_col()
      .shadow_lg()
      .child(
        div()
          .p_2()
          .border_b_1()
          .border_color(theme.border)
          .child(Input::new(&self.query_input).appearance(false)),
      )
      .child({
        let list = div()
          .id("picker-results")
          .flex_1()
          .overflow_y_scroll()
          .track_scroll(&self.scroll_handle)
          .children(results);
        if self.filtered.is_empty() {
          list.child(
            div().px_2().py_1().text_sm().text_color(theme.muted_foreground).child("No matches"),
          )
        } else {
          list
        }
      })
  }
}

// ---------------------------------------------------------------------------
// FilePickerDelegate
// ---------------------------------------------------------------------------

pub struct FilePickerDelegate {
  files: Vec<String>,
  code_view: Entity<CodeView>,
  project_path: PathBuf,
}

impl FilePickerDelegate {
  pub fn new(project_path: PathBuf, code_view: Entity<CodeView>) -> Self {
    let files = list_project_files(&project_path);
    Self { files, code_view, project_path }
  }
}

impl PickerDelegate for FilePickerDelegate {
  fn items(&self) -> &[String] {
    &self.files
  }

  fn confirm(&mut self, selected: &str, window: &mut Window, cx: &mut Context<PickerState<Self>>) {
    let full_path = self.project_path.join(selected);
    self.code_view.update(cx, |v, cx| v.open_file(full_path, window, cx));
  }

  fn dismiss(&mut self, _window: &mut Window, _cx: &mut Context<PickerState<Self>>) {
    // Nothing extra needed; workspace handles focus restoration
  }
}

// ---------------------------------------------------------------------------
// File listing via git index
// ---------------------------------------------------------------------------

fn list_project_files(path: &Path) -> Vec<String> {
  let Ok(repo) = git2::Repository::open(path) else {
    return Vec::new();
  };
  let Ok(index) = repo.index() else {
    return Vec::new();
  };
  index.iter().filter_map(|entry| String::from_utf8(entry.path.clone()).ok()).collect()
}
