use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::highlighter::SyntaxHighlighter;
use gpui_component::input::{Input, InputEvent, InputState, Rope};
use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::language::Language;
use crate::outline::{OutlineItem, compute_outline};
use crate::views::code_view::CodeView;
use crate::views::diff_view::{DiffSource, DiffView, GitLogEntry, git_log};
use crate::views::project_state::ProjectState;
use crate::views::reply_view::ReplyView;
use crate::views::todo_view::TodoView;

actions!(
  picker,
  [
    ConfirmPicker,
    CancelPicker,
    SelectNextItem,
    SelectPrevItem,
    OpenFilePicker,
    OpenContextPicker,
    ShowSessionPicker,
    SearchLines,
  ]
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
    KeyBinding::new("cmd-f", SearchLines, Some("Workspace")),
    // Also bind in "Input" context so our SearchLines takes precedence over
    // gpui-component's built-in Search action (which opens the editor's find
    // panel). Since this binding is registered after gpui_component::init(),
    // it has a higher index and wins at the same context depth.
    KeyBinding::new("cmd-f", SearchLines, Some("Input")),
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
    let row = div().px_2().py(px(3.0)).text_sm().font_family("Lilex");
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

  pub fn delegate(&self) -> &D {
    &self.delegate
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
  /// Set of relative paths that have been modified in the git working tree.
  modified_files: HashSet<String>,
  /// Indices of recently opened files (in recency order, most recent first).
  recent_indices: Vec<usize>,
}

impl FilePickerDelegate {
  pub fn new(
    project_path: PathBuf,
    code_view: Entity<CodeView>,
    recent_files: Vec<PathBuf>,
  ) -> Self {
    let (files, modified_files) = list_project_files_and_modified(&project_path);

    // Map recent_files (absolute paths) to indices in the files list.
    let recent_indices: Vec<usize> = recent_files
      .iter()
      .filter_map(|abs_path| {
        let rel = abs_path.strip_prefix(&project_path).ok()?;
        let rel_str = rel.to_str()?;
        files.iter().position(|f| f == rel_str)
      })
      .collect();

    Self { files, code_view, project_path, modified_files, recent_indices }
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

  fn filter(&self, query_lower: &[char]) -> Vec<FilteredItem> {
    let mut result = Vec::new();
    for (index, item) in self.files.iter().enumerate() {
      if let Some(score) = fuzzy_match(query_lower, item) {
        // Boost score for recently opened files so they sort to the top.
        let recency_boost = self
          .recent_indices
          .iter()
          .position(|&ri| ri == index)
          .map(|pos| 1000_i64 - pos as i64)
          .unwrap_or(0);
        result.push(FilteredItem { index, score: score + recency_boost });
      }
    }
    result
  }

  fn render_item(&self, index: usize, selected: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let label = &self.files[index];
    let is_modified = self.modified_files.contains(label);
    let is_recent = self.recent_indices.contains(&index);

    let row = div().px_2().py(px(3.0)).text_sm().font_family("Lilex").flex().items_center().gap_1();
    let row = if selected { row.bg(theme.accent).text_color(theme.accent_foreground) } else { row };

    let marker = if is_modified {
      let color =
        if selected { theme.accent_foreground } else { gpui::hsla(30. / 360., 0.8, 0.5, 1.0) };
      Some(("M", color))
    } else if is_recent {
      let color =
        if selected { theme.accent_foreground } else { gpui::hsla(210. / 360., 0.6, 0.5, 1.0) };
      Some(("R", color))
    } else {
      None
    };

    if let Some((text, color)) = marker {
      row
        .child(div().text_xs().text_color(color).font_weight(FontWeight::BOLD).child(text))
        .child(label.clone())
    } else {
      row.child(div().text_xs().w(px(10.0))).child(label.clone())
    }
  }
}

// ---------------------------------------------------------------------------
// DiffFilePickerDelegate
// ---------------------------------------------------------------------------

pub struct DiffFilePickerDelegate {
  labels: Vec<String>,
  reviewed: Vec<bool>,
  diff_view: Entity<DiffView>,
}

impl DiffFilePickerDelegate {
  pub fn new(diff_view: Entity<DiffView>, cx: &App) -> Self {
    let dv = diff_view.read(cx);
    let labels: Vec<String> = dv.file_diffs().iter().map(|fd| fd.name.clone()).collect();
    let reviewed: Vec<bool> = dv.file_diffs().iter().map(|fd| dv.is_reviewed(&fd.name)).collect();
    Self { labels, reviewed, diff_view }
  }
}

impl PickerDelegate for DiffFilePickerDelegate {
  fn items(&self) -> &[String] {
    &self.labels
  }

  fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<PickerState<Self>>) {
    self.diff_view.update(cx, |v, cx| v.set_file_index(index, window, cx));
  }

  fn filter(&self, query_lower: &[char]) -> Vec<FilteredItem> {
    let mut result = Vec::new();
    for (index, item) in self.labels.iter().enumerate() {
      if let Some(score) = fuzzy_match(query_lower, item) {
        // Push reviewed files to the bottom by subtracting a large bias.
        let bias = if self.reviewed[index] { -10000 } else { 0 };
        result.push(FilteredItem { index, score: score + bias });
      }
    }
    result
  }

  fn render_item(&self, index: usize, selected: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let label = &self.labels[index];
    let is_reviewed = self.reviewed[index];

    let row = div().px_2().py(px(3.0)).text_sm().font_family("Lilex").flex().items_center().gap_1();
    let row = if selected { row.bg(theme.accent).text_color(theme.accent_foreground) } else { row };

    if is_reviewed {
      let marker_color =
        if selected { theme.accent_foreground } else { gpui::hsla(120. / 360., 0.6, 0.4, 1.0) };
      row
        .child(div().text_xs().text_color(marker_color).font_weight(FontWeight::BOLD).child("✓"))
        .child(label.clone())
    } else {
      row.child(div().text_xs().w(px(10.0))).child(label.clone())
    }
  }
}

// ---------------------------------------------------------------------------
// GitLogPickerDelegate
// ---------------------------------------------------------------------------

pub struct GitLogPickerDelegate {
  labels: Vec<String>,
  entries: Vec<GitLogEntry>,
  diff_view: Entity<DiffView>,
}

impl GitLogPickerDelegate {
  pub fn new(diff_view: Entity<DiffView>, cx: &App) -> Self {
    let path = diff_view.read(cx).project_path().to_path_buf();
    let entries = git_log(&path);

    let mut labels = vec!["Working tree".to_string()];
    labels.extend(entries.iter().map(|e| format!("{} {}", e.short_hash, e.summary)));

    Self { labels, entries, diff_view }
  }
}

impl PickerDelegate for GitLogPickerDelegate {
  fn items(&self) -> &[String] {
    &self.labels
  }

  fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<PickerState<Self>>) {
    let source = if index == 0 {
      DiffSource::WorkingTree
    } else {
      let entry = &self.entries[index - 1];
      DiffSource::Commit { oid: entry.oid, summary: entry.summary.clone() }
    };
    self.diff_view.update(cx, |v, cx| v.set_source(source, window, cx));
  }

  fn render_item(&self, index: usize, selected: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let row = div().px_2().py(px(3.0)).text_sm().font_family("Lilex").flex().items_center().gap_1();
    let row = if selected { row.bg(theme.accent).text_color(theme.accent_foreground) } else { row };

    if index == 0 {
      let marker_color =
        if selected { theme.accent_foreground } else { gpui::hsla(30. / 360., 0.8, 0.5, 1.0) };
      row
        .child(div().text_xs().text_color(marker_color).font_weight(FontWeight::BOLD).child("*"))
        .child("Working tree".to_string())
    } else {
      let entry = &self.entries[index - 1];
      let hash_color =
        if selected { theme.accent_foreground } else { gpui::hsla(210. / 360., 0.6, 0.5, 1.0) };
      row
        .child(div().text_xs().text_color(hash_color).child(entry.short_hash.clone()))
        .child(entry.summary.clone())
    }
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
    let outline = compute_outline(&text, Language::Markdown);
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

  fn render_item(&self, index: usize, selected: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let item = &self.outline[index];
    let indent = "  ".repeat(item.depth);

    let row = div().px_2().py(px(3.0)).text_sm().flex().items_center().font_family("Lilex");
    let row = if selected { row.bg(theme.accent).text_color(theme.accent_foreground) } else { row };

    let keyword_color =
      if selected { None } else { theme.highlight_theme.style("keyword").and_then(|s| s.color) };
    let function_color =
      if selected { None } else { theme.highlight_theme.style("function").and_then(|s| s.color) };

    if item.context.is_empty() {
      let name_el = div().child(item.name.clone());
      let name_el =
        if let Some(color) = function_color { name_el.text_color(color) } else { name_el };
      row.child(indent).child(name_el)
    } else {
      let ctx_el = div().child(format!("{} ", item.context));
      let ctx_el = if let Some(color) = keyword_color { ctx_el.text_color(color) } else { ctx_el };
      let name_el = div().child(item.name.clone());
      let name_el =
        if let Some(color) = function_color { name_el.text_color(color) } else { name_el };
      row.child(indent).child(ctx_el).child(name_el)
    }
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

/// List tracked files and modified files from a single repo open.
fn list_project_files_and_modified(path: &Path) -> (Vec<String>, HashSet<String>) {
  let Ok(repo) = git2::Repository::open(path) else {
    return (Vec::new(), HashSet::new());
  };

  let files = repo
    .index()
    .ok()
    .map(|index| {
      index.iter().filter_map(|entry| String::from_utf8(entry.path.clone()).ok()).collect()
    })
    .unwrap_or_default();

  let mut opts = git2::StatusOptions::default();
  opts.include_untracked(true).recurse_untracked_dirs(true);

  let modified = repo
    .statuses(Some(&mut opts))
    .ok()
    .map(|statuses| {
      statuses
        .iter()
        .filter_map(|entry| {
          let status = entry.status();
          let dominated_by_clean =
            status.is_empty() || status == git2::Status::IGNORED || status == git2::Status::CURRENT;
          if dominated_by_clean {
            return None;
          }
          entry.path().map(|p| p.to_string())
        })
        .collect()
    })
    .unwrap_or_default();

  (files, modified)
}

// ---------------------------------------------------------------------------
// ReplyTurnPickerDelegate
// ---------------------------------------------------------------------------

pub struct ReplyTurnPickerDelegate {
  labels: Vec<String>,
  /// Chronological turn indices corresponding to each label (newest first).
  turn_indices: Vec<usize>,
  reply_view: Entity<ReplyView>,
}

impl ReplyTurnPickerDelegate {
  pub fn new(reply_view: Entity<ReplyView>, cx: &App) -> Self {
    let rv = reply_view.read(cx);
    let turns = rv.turns();
    let mut labels = Vec::with_capacity(turns.len());
    let mut turn_indices = Vec::with_capacity(turns.len());

    // Newest first.
    for turn in turns.iter().rev() {
      labels.push(format!("Turn {}: {}", turn.index + 1, turn.label()));
      turn_indices.push(turn.index);
    }

    Self { labels, turn_indices, reply_view }
  }
}

impl PickerDelegate for ReplyTurnPickerDelegate {
  fn items(&self) -> &[String] {
    &self.labels
  }

  fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<PickerState<Self>>) {
    let turn_index = self.turn_indices[index];
    self.reply_view.update(cx, |v, cx| v.set_turn_index(turn_index, window, cx));
  }
}

// ---------------------------------------------------------------------------
// ReplyHeadingPickerDelegate
// ---------------------------------------------------------------------------

pub struct ReplyHeadingPickerDelegate {
  labels: Vec<String>,
  outline: Vec<OutlineItem>,
  reply_view: Entity<ReplyView>,
}

impl ReplyHeadingPickerDelegate {
  pub fn new(reply_view: Entity<ReplyView>, cx: &App) -> Self {
    let text = reply_view.read(cx).editor_text(cx);
    let outline = compute_outline(&text, Language::Markdown);
    let labels = outline_labels(&outline);
    Self { labels, outline, reply_view }
  }
}

impl PickerDelegate for ReplyHeadingPickerDelegate {
  fn items(&self) -> &[String] {
    &self.labels
  }

  fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<PickerState<Self>>) {
    let line = self.outline[index].line;
    self.reply_view.update(cx, |v, cx| v.scroll_to_line(line, window, cx));
  }

  fn filter(&self, query_lower: &[char]) -> Vec<FilteredItem> {
    hierarchy_preserving_filter(&self.outline, &self.labels, query_lower)
  }
}

// ---------------------------------------------------------------------------
// SessionPickerDelegate
// ---------------------------------------------------------------------------

pub struct SessionPickerDelegate {
  labels: Vec<String>,
  /// (project_index, session_index) for each label.
  entries: Vec<(usize, usize)>,
  active_entry: Option<usize>,
  problems_per_entry: Vec<usize>,
  /// Stores the last confirmed entry for the workspace to read.
  confirmed: (usize, usize),
}

impl SessionPickerDelegate {
  pub fn new(projects: &[ProjectState], active_project_index: usize) -> Self {
    let mut labels = Vec::new();
    let mut entries = Vec::new();
    let mut problems_per_entry = Vec::new();
    let mut active_entry = None;

    for (pi, project) in projects.iter().enumerate() {
      for (si, session) in project.sessions.iter().enumerate() {
        let is_active = pi == active_project_index && Some(si) == project.active_session_index;
        if is_active {
          active_entry = Some(labels.len());
        }
        labels.push(format!("{} / {}: {}", project.name, session.slug, session.label));
        entries.push((pi, si));
        problems_per_entry.push(session.problems.len());
      }
    }

    Self { labels, entries, active_entry, problems_per_entry, confirmed: (0, 0) }
  }

  pub fn confirmed_entry(&self) -> (usize, usize) {
    self.confirmed
  }
}

impl PickerDelegate for SessionPickerDelegate {
  fn items(&self) -> &[String] {
    &self.labels
  }

  fn confirm(&mut self, index: usize, _window: &mut Window, _cx: &mut Context<PickerState<Self>>) {
    self.confirmed = self.entries[index];
  }

  fn render_item(&self, index: usize, selected: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let label = &self.labels[index];
    let is_active = self.active_entry == Some(index);
    let has_problems = self.problems_per_entry[index] > 0;

    let row = div().px_2().py(px(3.0)).text_sm().font_family("Lilex").flex().items_center().gap_1();
    let row = if selected { row.bg(theme.accent).text_color(theme.accent_foreground) } else { row };

    // Active session marker
    let marker = if is_active {
      let color =
        if selected { theme.accent_foreground } else { gpui::hsla(120. / 360., 0.6, 0.4, 1.0) };
      div().text_xs().text_color(color).font_weight(FontWeight::BOLD).child(">")
    } else if has_problems {
      let color = if selected { theme.accent_foreground } else { gpui::hsla(0., 0.8, 0.5, 1.0) };
      div().text_xs().text_color(color).font_weight(FontWeight::BOLD).child("!")
    } else {
      div().text_xs().w(px(10.0))
    };

    row.child(marker).child(label.clone())
  }
}

// ---------------------------------------------------------------------------
// LineSearchPickerDelegate
// ---------------------------------------------------------------------------

type ScrollCallback = Box<dyn Fn(u32, &mut Window, &mut App)>;

struct LineEntry {
  /// 1-based line number.
  line_number: u32,
  /// The line content (without trailing newline).
  content: String,
  /// Syntax highlight styles for this line, with byte ranges relative to `content`.
  styles: Vec<(Range<usize>, HighlightStyle)>,
}

pub struct LineSearchPickerDelegate<F: Fn(u32, &mut Window, &mut App) + 'static> {
  labels: Vec<String>,
  entries: Vec<LineEntry>,
  scroll_to_line: F,
}

impl<F: Fn(u32, &mut Window, &mut App) + 'static> LineSearchPickerDelegate<F> {
  fn build(text: &str, language: &str, scroll_to_line: F, cx: &App) -> Self {
    let theme = cx.theme();
    let rope = Rope::from(text);
    let mut highlighter = SyntaxHighlighter::new(language);
    highlighter.update(None, &rope);

    let mut entries = Vec::new();
    let mut labels = Vec::new();
    let mut byte_offset: usize = 0;

    for (i, line) in text.split('\n').enumerate() {
      let line_number = (i as u32) + 1;
      let line_byte_start = byte_offset;
      let line_byte_end = byte_offset + line.len();

      // Skip empty/whitespace-only lines.
      if !line.trim().is_empty() {
        let raw_styles =
          highlighter.styles(&(line_byte_start..line_byte_end), &theme.highlight_theme);
        // Adjust ranges to be relative to line start.
        let styles: Vec<(Range<usize>, HighlightStyle)> = raw_styles
          .into_iter()
          .filter_map(|(r, s)| {
            let start = r.start.saturating_sub(line_byte_start);
            let end = r.end.saturating_sub(line_byte_start).min(line.len());
            if start < end { Some((start..end, s)) } else { None }
          })
          .collect();

        labels.push(format!("{line_number}: {line}"));
        entries.push(LineEntry { line_number, content: line.to_string(), styles });
      }

      // +1 for the '\n' separator (or end of string).
      byte_offset = line_byte_end + 1;
    }

    Self { labels, entries, scroll_to_line }
  }
}

impl LineSearchPickerDelegate<ScrollCallback> {
  pub fn for_code_view(code_view: &Entity<CodeView>, cx: &App) -> Self {
    let text = code_view.read(cx).editor_text(cx);
    let language = code_view.read(cx).current_language().name();
    let entity = code_view.clone();
    let callback: ScrollCallback = Box::new(move |line, window, cx| {
      entity.update(cx, |v, cx| v.scroll_to_line(line, window, cx));
    });
    Self::build(&text, language, callback, cx)
  }

  pub fn for_todo_view(todo_view: &Entity<TodoView>, cx: &App) -> Self {
    let text = todo_view.read(cx).editor_text(cx);
    let entity = todo_view.clone();
    let callback: ScrollCallback = Box::new(move |line, window, cx| {
      entity.update(cx, |v, cx| v.scroll_to_line(line, window, cx));
    });
    Self::build(&text, "markdown", callback, cx)
  }

  pub fn for_diff_view(diff_view: &Entity<DiffView>, cx: &App) -> Self {
    let text = diff_view.read(cx).editor_text(cx);
    let language = diff_view.read(cx).current_file_language().name();
    let entity = diff_view.clone();
    let callback: ScrollCallback = Box::new(move |line, window, cx| {
      entity.update(cx, |v, cx| v.scroll_to_line(line, window, cx));
    });
    Self::build(&text, language, callback, cx)
  }

  pub fn for_reply_view(reply_view: &Entity<ReplyView>, cx: &App) -> Self {
    let text = reply_view.read(cx).editor_text(cx);
    let entity = reply_view.clone();
    let callback: ScrollCallback = Box::new(move |line, window, cx| {
      entity.update(cx, |v, cx| v.scroll_to_line(line, window, cx));
    });
    Self::build(&text, "markdown", callback, cx)
  }
}

impl<F: Fn(u32, &mut Window, &mut App) + 'static> PickerDelegate for LineSearchPickerDelegate<F> {
  fn items(&self) -> &[String] {
    &self.labels
  }

  fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<PickerState<Self>>) {
    let line = self.entries[index].line_number;
    // line_number is 1-based; scroll_to_line expects 0-based line index.
    let line_0 = line.saturating_sub(1);
    (self.scroll_to_line)(line_0, window, cx);
  }

  fn render_item(&self, index: usize, selected: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let entry = &self.entries[index];

    let row = div().px_2().py(px(3.0)).text_sm().font_family("Lilex").flex().items_center();
    let row = if selected { row.bg(theme.accent).text_color(theme.accent_foreground) } else { row };

    // Line number in muted/comment color.
    let line_num_color = if selected {
      theme.accent_foreground
    } else {
      theme.highlight_theme.style("comment").and_then(|s| s.color).unwrap_or(theme.muted_foreground)
    };
    let line_num =
      div().text_xs().text_color(line_num_color).mr_1().child(format!("{}:", entry.line_number));

    // Build syntax-highlighted line content using StyledText.
    let line_text: SharedString = entry.content.clone().into();
    let styled = if selected {
      // When selected, use accent foreground — no syntax colors.
      StyledText::new(line_text)
    } else {
      let adjusted_styles: Vec<(Range<usize>, HighlightStyle)> = entry.styles.clone();
      let default_style = TextStyle {
        font_family: "Lilex".into(),
        font_size: theme.font_size.into(),
        color: theme.foreground,
        ..Default::default()
      };
      StyledText::new(line_text).with_default_highlights(&default_style, adjusted_styles)
    };

    row.child(line_num).child(styled)
  }
}
