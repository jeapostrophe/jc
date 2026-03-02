use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::{Input, InputEvent, InputState};
use std::path::{Path, PathBuf};

use crate::outline::{OutlineItem, compute_outline};
use crate::views::code_view::CodeView;
use crate::views::diff_view::DiffView;
use crate::views::todo_view::TodoView;

actions!(
  picker,
  [ConfirmPicker, CancelPicker, SelectNextItem, SelectPrevItem, OpenFilePicker, OpenContextPicker]
);

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
    KeyBinding::new("cmd-t", OpenContextPicker, Some("Workspace")),
  ]);
}

const MAX_VISIBLE_RESULTS: usize = 200;

pub fn fuzzy_match(query_lower: &[char], candidate: &str) -> Option<i64> {
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
  fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<PickerState<Self>>)
  where
    Self: Sized;
  fn dismiss(&mut self, _window: &mut Window, _cx: &mut Context<PickerState<Self>>)
  where
    Self: Sized,
  {
  }

  /// Return filtered (index, score) pairs. Default: fuzzy match on items().
  fn filter(&self, query_lower: &[char]) -> Vec<FilteredItem> {
    let mut result = Vec::new();
    for (index, item) in self.items().iter().enumerate() {
      if let Some(score) = fuzzy_match(query_lower, item) {
        result.push(FilteredItem { index, score });
      }
    }
    result
  }

  /// Render a single item row. Override for custom formatting (e.g. indentation).
  fn render_item(&self, index: usize, selected: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let label = &self.items()[index];
    let row = div().px_2().py(px(3.0)).text_sm();
    let row = if selected { row.bg(theme.accent).text_color(theme.accent_foreground) } else { row };
    row.child(label.clone())
  }
}

pub enum PickerEvent {
  Confirmed,
  Dismissed,
}

impl<D: PickerDelegate> EventEmitter<PickerEvent> for PickerState<D> {}

pub struct FilteredItem {
  pub index: usize,
  pub score: i64,
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
    self.filtered = self.delegate.filter(&query_lower);
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
      let index = item.index;
      self.delegate.confirm(index, window, cx);
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

    let results: Vec<Div> = self
      .filtered
      .iter()
      .enumerate()
      .take(MAX_VISIBLE_RESULTS)
      .map(|(i, fi)| {
        let selected = i == self.selected_index;
        self.delegate.render_item(fi.index, selected, cx)
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

  fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<PickerState<Self>>) {
    let full_path = self.project_path.join(&self.files[index]);
    self.code_view.update(cx, |v, cx| v.open_file(full_path, window, cx));
  }
}

// ---------------------------------------------------------------------------
// DiffFilePickerDelegate
// ---------------------------------------------------------------------------

pub struct DiffFilePickerDelegate {
  labels: Vec<String>,
  lines: Vec<u32>,
  diff_view: Entity<DiffView>,
}

impl DiffFilePickerDelegate {
  pub fn new(diff_view: Entity<DiffView>, cx: &App) -> Self {
    let entries = diff_view.read(cx).file_entries();
    let mut labels = Vec::with_capacity(entries.len());
    let mut lines = Vec::with_capacity(entries.len());
    for (name, line) in entries {
      labels.push(name.clone());
      lines.push(*line);
    }
    Self { labels, lines, diff_view }
  }
}

impl PickerDelegate for DiffFilePickerDelegate {
  fn items(&self) -> &[String] {
    &self.labels
  }

  fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<PickerState<Self>>) {
    let line = self.lines[index];
    self.diff_view.update(cx, |v, cx| v.scroll_to_line(line, window, cx));
  }
}

// ---------------------------------------------------------------------------
// TodoHeaderPickerDelegate
// ---------------------------------------------------------------------------

pub struct TodoHeaderPickerDelegate {
  labels: Vec<String>,
  outline: Vec<OutlineItem>,
  todo_view: Entity<TodoView>,
}

impl TodoHeaderPickerDelegate {
  pub fn new(todo_view: Entity<TodoView>, cx: &App) -> Self {
    let text = todo_view.read(cx).editor_text(cx);
    let outline = compute_outline(&text, "markdown");
    let labels = outline_labels(&outline);
    Self { labels, outline, todo_view }
  }
}

impl PickerDelegate for TodoHeaderPickerDelegate {
  fn items(&self) -> &[String] {
    &self.labels
  }

  fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<PickerState<Self>>) {
    let line = self.outline[index].line;
    self.todo_view.update(cx, |v, cx| v.scroll_to_line(line, window, cx));
  }

  fn filter(&self, query_lower: &[char]) -> Vec<FilteredItem> {
    hierarchy_preserving_filter(&self.outline, &self.labels, query_lower)
  }
}

// ---------------------------------------------------------------------------
// CodeSymbolPickerDelegate
// ---------------------------------------------------------------------------

pub struct CodeSymbolPickerDelegate {
  labels: Vec<String>,
  outline: Vec<OutlineItem>,
  code_view: Entity<CodeView>,
}

impl CodeSymbolPickerDelegate {
  pub fn new(code_view: Entity<CodeView>, cx: &App) -> Self {
    let text = code_view.read(cx).editor_text(cx);
    let language = code_view.read(cx).current_language();
    let outline = compute_outline(&text, language);
    let labels = outline_labels(&outline);
    Self { labels, outline, code_view }
  }
}

impl PickerDelegate for CodeSymbolPickerDelegate {
  fn items(&self) -> &[String] {
    &self.labels
  }

  fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<PickerState<Self>>) {
    let line = self.outline[index].line;
    self.code_view.update(cx, |v, cx| v.scroll_to_line(line, window, cx));
  }

  fn filter(&self, query_lower: &[char]) -> Vec<FilteredItem> {
    hierarchy_preserving_filter(&self.outline, &self.labels, query_lower)
  }
}

fn outline_labels(outline: &[OutlineItem]) -> Vec<String> {
  outline
    .iter()
    .map(|item| {
      let indent = "  ".repeat(item.depth);
      format!("{indent}{}", item.label)
    })
    .collect()
}

// ---------------------------------------------------------------------------
// Hierarchy-preserving filter
// ---------------------------------------------------------------------------

/// Filters outline items while preserving ancestor context.
/// When an item matches, all its ancestors (by depth/range containment) are
/// also included so the user sees where the match lives in the hierarchy.
fn hierarchy_preserving_filter(
  outline: &[OutlineItem],
  labels: &[String],
  query_lower: &[char],
) -> Vec<FilteredItem> {
  if query_lower.is_empty() {
    return (0..labels.len()).map(|i| FilteredItem { index: i, score: 0 }).collect();
  }

  // First pass: score each item against query (match on name, not indented label).
  let scores: Vec<Option<i64>> =
    outline.iter().map(|item| fuzzy_match(query_lower, &item.name)).collect();

  // Mark items that matched, then walk parent chains to include ancestors.
  let mut included = vec![false; outline.len()];
  for (i, s) in scores.iter().enumerate() {
    if s.is_some() {
      included[i] = true;
      let mut ancestor = outline[i].parent;
      while let Some(idx) = ancestor {
        if included[idx] {
          break; // ancestors already marked
        }
        included[idx] = true;
        ancestor = outline[idx].parent;
      }
    }
  }

  // Build result preserving original order. Ancestors get score 0 (sort-neutral),
  // matched items keep their score.
  let mut result = Vec::new();
  for (i, inc) in included.iter().enumerate() {
    if *inc {
      result.push(FilteredItem { index: i, score: scores[i].unwrap_or(0) });
    }
  }

  result
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
