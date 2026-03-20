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
use crate::views::session_state::SessionId;
use crate::views::todo_view::TodoView;
use jc_core::snippets::Snippet;
use jc_terminal::TerminalView;

actions!(
  picker,
  [
    ConfirmPicker,
    CancelPicker,
    SelectNextItem,
    SelectPrevItem,
    SelectPageDown,
    SelectPageUp,
    DeletePickerItem,
    OpenPicker,
    DrillDownPicker,
    ProjectActionsPicker,
    ShowSessionPicker,
    SearchLines,
    ShowSnippetPicker,
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
    KeyBinding::new("pagedown", SelectPageDown, Some("Picker")),
    KeyBinding::new("pageup", SelectPageUp, Some("Picker")),
    KeyBinding::new("cmd-shift-backspace", DeletePickerItem, Some("Picker")),
    KeyBinding::new("cmd-o", OpenPicker, Some("Workspace")),
    KeyBinding::new("cmd-shift-o", DrillDownPicker, Some("Workspace")),
    KeyBinding::new("cmd-f", SearchLines, Some("Workspace")),
    // Also bind in "Input" context so our SearchLines takes precedence over
    // gpui-component's built-in Search action (which opens the editor's find
    // panel). Since this binding is registered after gpui_component::init(),
    // it has a higher index and wins at the same context depth.
    KeyBinding::new("cmd-f", SearchLines, Some("Input")),
  ]);
}

const MAX_VISIBLE_RESULTS: usize = 200;

/// Fixed-width, right-aligned marker column used in session picker rows.
fn picker_marker_base() -> Div {
  div().text_xs().font_weight(FontWeight::BOLD).w(px(22.0)).flex_shrink_0().flex().justify_end()
}

/// A small `marker label` pair for footer legends.
fn legend_item(marker: &str, marker_color: Hsla, label: &str, label_color: Hsla) -> Div {
  div().flex().items_center().gap_0p5().children([
    div().text_color(marker_color).font_weight(FontWeight::BOLD).child(marker.to_string()),
    div().text_color(label_color).child(label.to_string()),
  ])
}

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
  fn delete(&mut self, _index: usize, _window: &mut Window, _cx: &mut Context<PickerState<Self>>)
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

  /// Optional footer shown below the results list (e.g. a legend).
  fn render_footer(&self, _cx: &App) -> Option<Div> {
    None
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

  fn select_page_down(&mut self, _: &SelectPageDown, _window: &mut Window, cx: &mut Context<Self>) {
    if !self.filtered.is_empty() {
      self.selected_index = (self.selected_index + 10).min(self.filtered.len() - 1);
      self.scroll_handle.scroll_to_item(self.selected_index);
      cx.notify();
    }
  }

  fn select_page_up(&mut self, _: &SelectPageUp, _window: &mut Window, cx: &mut Context<Self>) {
    if !self.filtered.is_empty() {
      self.selected_index = self.selected_index.saturating_sub(10);
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

  fn delete_selected(&mut self, _: &DeletePickerItem, window: &mut Window, cx: &mut Context<Self>) {
    if let Some(item) = self.filtered.get(self.selected_index) {
      let index = item.index;
      self.delegate.delete(index, window, cx);
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

    let results: Vec<Stateful<Div>> = self
      .filtered
      .iter()
      .enumerate()
      .take(MAX_VISIBLE_RESULTS)
      .map(|(i, fi)| {
        let selected = i == self.selected_index;
        self
          .delegate
          .render_item(fi.index, selected, cx)
          .id(("picker-item", i))
          .cursor_pointer()
          .on_click(cx.listener(move |this, _, window, cx| {
            this.selected_index = i;
            this.confirm_selected(&ConfirmPicker, window, cx);
          }))
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
      .on_action(cx.listener(Self::select_page_down))
      .on_action(cx.listener(Self::select_page_up))
      .on_action(cx.listener(Self::delete_selected))
      .font_family("Lilex")
      .w(px(500.0))
      .max_h_full()
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
      .children(self.delegate.render_footer(cx).map(|footer| {
        footer.border_t_1().border_color(theme.border)
      }))
  }
}

// ---------------------------------------------------------------------------
// OpenPickerDelegate (replaces FilePickerDelegate + ViewPickerDelegate)
// ---------------------------------------------------------------------------

use crate::views::pane::PaneContentKind;

pub enum OpenPickerResult {
  SwitchPane(PaneContentKind),
  OpenFile, // file already opened in code_view by confirm()
}

pub struct OpenPickerDelegate {
  labels: Vec<String>,
  /// First N items are panes (N = PaneContentKind::ALL.len()), rest are files.
  pane_count: usize,
  kinds: Vec<PaneContentKind>,
  code_view: Entity<CodeView>,
  project_path: PathBuf,
  modified_files: HashSet<String>,
  recent_indices: Vec<usize>,
  result: Option<OpenPickerResult>,
}

impl OpenPickerDelegate {
  pub fn new(
    project_path: PathBuf,
    code_view: Entity<CodeView>,
    recent_files: Vec<PathBuf>,
  ) -> Self {
    let kinds: Vec<PaneContentKind> = PaneContentKind::ALL.to_vec();
    let pane_count = kinds.len();

    let mut labels: Vec<String> = kinds.iter().map(|k| k.label().to_string()).collect();

    let (files, modified_files) = list_project_files_and_modified(&project_path);

    // Map recent_files (absolute paths) to indices in the files list (offset by pane_count).
    let recent_indices: Vec<usize> = recent_files
      .iter()
      .filter_map(|abs_path| {
        let rel = abs_path.strip_prefix(&project_path).ok()?;
        let rel_str = rel.to_str()?;
        files.iter().position(|f| f == rel_str).map(|i| i + pane_count)
      })
      .collect();

    // File labels: prefix each file with `/`.
    labels.extend(files.iter().map(|f| format!("/{f}")));

    Self { labels, pane_count, kinds, code_view, project_path, modified_files, recent_indices, result: None }
  }

  pub fn result(&self) -> Option<&OpenPickerResult> {
    self.result.as_ref()
  }
}

impl PickerDelegate for OpenPickerDelegate {
  fn items(&self) -> &[String] {
    &self.labels
  }

  fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<PickerState<Self>>) {
    if index < self.pane_count {
      self.result = Some(OpenPickerResult::SwitchPane(self.kinds[index]));
    } else {
      // File path: strip the leading `/` prefix from the label.
      let rel_path = &self.labels[index][1..];
      let full_path = self.project_path.join(rel_path);
      self.code_view.update(cx, |v, cx| v.open_file(full_path, window, cx));
      self.result = Some(OpenPickerResult::OpenFile);
    }
  }

  fn filter(&self, query_lower: &[char]) -> Vec<FilteredItem> {
    let mut result = Vec::new();
    for (index, item) in self.labels.iter().enumerate() {
      if let Some(score) = fuzzy_match(query_lower, item) {
        let boost = if index < self.pane_count {
          // Pane items get a boost so they rank above files on short queries.
          500
        } else {
          // Recency boost for files.
          self
            .recent_indices
            .iter()
            .position(|&ri| ri == index)
            .map(|pos| 1000_i64 - pos as i64)
            .unwrap_or(0)
        };
        result.push(FilteredItem { index, score: score + boost });
      }
    }
    result
  }

  fn render_item(&self, index: usize, selected: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let label = &self.labels[index];

    let row = div().px_2().py(px(3.0)).text_sm().font_family("Lilex").flex().items_center().gap_1();
    let row = if selected { row.bg(theme.accent).text_color(theme.accent_foreground) } else { row };

    if index < self.pane_count {
      // Pane item: just the label text.
      row.child(div().text_xs().w(px(10.0))).child(label.clone())
    } else {
      // File item: show M/R markers.
      let rel_path = &label[1..]; // strip leading `/`
      let is_modified = self.modified_files.contains(rel_path);
      let is_recent = self.recent_indices.contains(&index);

      let marker = if is_modified {
        let color = if selected { theme.accent_foreground } else { theme.yellow };
        Some(("M", color))
      } else if is_recent {
        let color = if selected { theme.accent_foreground } else { theme.blue };
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
}

// ---------------------------------------------------------------------------
// DiffDrillDownPickerDelegate (replaces DiffFilePickerDelegate + GitLogPickerDelegate)
// ---------------------------------------------------------------------------

enum DiffDrillDownEntry {
  File { name: String, reviewed: bool },
  Commit { entry: GitLogEntry },
  WorkingTree,
}

pub struct DiffDrillDownPickerDelegate {
  labels: Vec<String>,
  entries: Vec<DiffDrillDownEntry>,
  diff_view: Entity<DiffView>,
}

impl DiffDrillDownPickerDelegate {
  pub fn new(diff_view: Entity<DiffView>, cx: &App) -> Self {
    let dv = diff_view.read(cx);
    let mut labels = Vec::new();
    let mut entries = Vec::new();

    // 1. Diff files.
    for fd in dv.file_diffs() {
      let reviewed = dv.is_reviewed(&fd.name);
      labels.push(fd.name.clone());
      entries.push(DiffDrillDownEntry::File { name: fd.name.clone(), reviewed });
    }

    // 2. Git log entries prefixed with `@`.
    // Working tree first.
    labels.push("@* Working tree".to_string());
    entries.push(DiffDrillDownEntry::WorkingTree);

    // Then commits.
    let path = dv.project_path().to_path_buf();
    let log_entries = git_log(&path);
    for e in log_entries {
      labels.push(format!("@{} {}", e.short_hash, e.summary));
      entries.push(DiffDrillDownEntry::Commit { entry: e });
    }

    Self { labels, entries, diff_view }
  }
}

impl PickerDelegate for DiffDrillDownPickerDelegate {
  fn items(&self) -> &[String] {
    &self.labels
  }

  fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<PickerState<Self>>) {
    match &self.entries[index] {
      DiffDrillDownEntry::File { name, .. } => {
        // Find the file index in the diff view's file list.
        let file_idx = {
          let dv = self.diff_view.read(cx);
          dv.file_diffs().iter().position(|fd| fd.name == *name)
        };
        if let Some(idx) = file_idx {
          self.diff_view.update(cx, |v, cx| v.set_file_index(idx, window, cx));
        }
      }
      DiffDrillDownEntry::WorkingTree => {
        self.diff_view.update(cx, |v, cx| v.set_source(DiffSource::WorkingTree, window, cx));
      }
      DiffDrillDownEntry::Commit { entry, .. } => {
        let source = DiffSource::Commit { oid: entry.oid, summary: entry.summary.clone() };
        self.diff_view.update(cx, |v, cx| v.set_source(source, window, cx));
      }
    }
  }

  fn filter(&self, query_lower: &[char]) -> Vec<FilteredItem> {
    let mut result = Vec::new();
    for (index, item) in self.labels.iter().enumerate() {
      if let Some(score) = fuzzy_match(query_lower, item) {
        let bias = match &self.entries[index] {
          DiffDrillDownEntry::File { reviewed: true, .. } => -10000,
          DiffDrillDownEntry::WorkingTree | DiffDrillDownEntry::Commit { .. } => -5000,
          DiffDrillDownEntry::File { reviewed: false, .. } => 0,
        };
        result.push(FilteredItem { index, score: score + bias });
      }
    }
    result
  }

  fn render_item(&self, index: usize, selected: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let row = div().px_2().py(px(3.0)).text_sm().font_family("Lilex").flex().items_center().gap_1();
    let row = if selected { row.bg(theme.accent).text_color(theme.accent_foreground) } else { row };

    match &self.entries[index] {
      DiffDrillDownEntry::File { name, reviewed } => {
        if *reviewed {
          let marker_color = if selected { theme.accent_foreground } else { theme.green };
          row
            .child(div().text_xs().text_color(marker_color).font_weight(FontWeight::BOLD).child("✓"))
            .child(name.clone())
        } else {
          row.child(div().text_xs().w(px(10.0))).child(name.clone())
        }
      }
      DiffDrillDownEntry::WorkingTree => {
        let marker_color = if selected { theme.accent_foreground } else { theme.yellow };
        row
          .child(div().text_xs().text_color(marker_color).font_weight(FontWeight::BOLD).child("*"))
          .child("Working tree".to_string())
      }
      DiffDrillDownEntry::Commit { entry, .. } => {
        let hash_color = if selected { theme.accent_foreground } else { theme.blue };
        row
          .child(div().text_xs().text_color(hash_color).child(entry.short_hash.clone()))
          .child(entry.summary.clone())
      }
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

  let mut files: Vec<String> = repo
    .index()
    .ok()
    .map(|index| {
      index.iter().filter_map(|entry| String::from_utf8(entry.path.clone()).ok()).collect()
    })
    .unwrap_or_default();

  let mut opts = git2::StatusOptions::default();
  opts.include_untracked(true).recurse_untracked_dirs(true);

  let modified: HashSet<String> = repo
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

  // Add untracked (new) files to the file list so they appear in the picker.
  let tracked: HashSet<String> = files.iter().cloned().collect();
  for path in &modified {
    if !tracked.contains(path) {
      files.push(path.clone());
    }
  }

  (files, modified)
}

// ---------------------------------------------------------------------------
// SessionPickerDelegate
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct SessionPickerEntry {
  kind: SessionPickerEntryKind,
  project_index: usize,
  project_name: String,
  label: String,
  /// Whether this label appears on more than one session (needing disambiguation).
  ambiguous_label: bool,
  /// Total problem count (session + project).
  problems: usize,
  /// Minimum problem rank (lower = more urgent). `i8::MAX` if no problems.
  min_rank: i8,
}

#[derive(Clone)]
enum SessionPickerEntryKind {
  /// An adopted session — stores its SessionId.
  Session(SessionId),
  /// A TODO.md session not yet adopted (has uuid + label but no running terminal).
  Unadopted { uuid: String, disabled: bool },
  /// A project with no sessions — selecting it will discover-or-create a session.
  EmptyProject,
}

#[derive(Clone)]
pub enum SessionPickerResult {
  /// Switch to an existing session: (project_index, session_id).
  Session(usize, SessionId),
  /// Adopt a TODO.md session that isn't running yet: (project_index, uuid, label).
  Adopt(usize, String, String),
  /// Initialize a project that has no sessions yet: project_index.
  InitProject(usize),
  /// Toggle disabled state on a session: (project_index, label).
  ToggleDisabled(usize, String),
}

pub struct SessionPickerDelegate {
  /// Labels used for fuzzy filtering (format: "project / label").
  labels: Vec<String>,
  entries: Vec<SessionPickerEntry>,
  active_entry: Option<usize>,
  /// Stores the last confirmed or deleted entry for the workspace to read.
  result: Option<SessionPickerResult>,
}

impl SessionPickerDelegate {
  pub fn new(
    projects: &[ProjectState],
    active_project_index: usize,
    todo_documents: &[&jc_core::todo::TodoDocument],
  ) -> Self {
    use std::collections::{HashMap, HashSet};

    let mut entries = Vec::new();
    let mut active_entry = None;

    for (pi, project) in projects.iter().enumerate() {
      // Collect UUIDs already adopted into running sessions.
      let adopted_uuids: HashSet<&str> = project
        .sessions
        .values()
        .filter_map(|s| s.uuid.as_deref())
        .collect();

      let has_adoptable = todo_documents
        .get(pi)
        .map_or(false, |d| d.sessions.iter().any(|s| !s.uuid.is_empty()));
      if project.sessions.is_empty() && !has_adoptable {
        let min_rank = project.problems.iter().map(|p| p.rank()).min().unwrap_or(i8::MAX);
        entries.push(SessionPickerEntry {
          kind: SessionPickerEntryKind::EmptyProject,
          project_index: pi,
          project_name: project.name.clone(),
          label: String::new(),
          ambiguous_label: false,
          problems: project.problems.len(),
          min_rank,
        });
        continue;
      }

      // Adopted sessions.
      for (&id, session) in &project.sessions {
        let is_active = pi == active_project_index && project.active_session == Some(id);
        if is_active {
          active_entry = Some(entries.len());
        }
        let min_rank = session
          .problems
          .iter()
          .map(|p| p.rank())
          .chain(project.problems.iter().map(|p| p.rank()))
          .min()
          .unwrap_or(i8::MAX);
        entries.push(SessionPickerEntry {
          kind: SessionPickerEntryKind::Session(id),
          project_index: pi,
          project_name: project.name.clone(),
          label: session.label.clone(),
          ambiguous_label: false, // computed below
          problems: session.problems.len() + project.problems.len(),
          min_rank,
        });
      }

      // Unadopted TODO.md sessions: in TODO but no running SessionState.
      let adopted_labels: HashSet<&str> =
        project.sessions.values().map(|s| s.label.as_str()).collect();
      if let Some(doc) = todo_documents.get(pi) {
        for todo_session in &doc.sessions {
          let uuid_adopted = !todo_session.uuid.is_empty()
            && adopted_uuids.contains(todo_session.uuid.as_str());
          let label_adopted = adopted_labels.contains(todo_session.label.as_str());
          // Skip sessions with no UUID — nothing to adopt/resume.
          if todo_session.uuid.is_empty() {
            continue;
          }
          if !uuid_adopted && !label_adopted {
            entries.push(SessionPickerEntry {
              kind: SessionPickerEntryKind::Unadopted {
                uuid: todo_session.uuid.clone(),
                disabled: todo_session.disabled,
              },
              project_index: pi,
              project_name: project.name.clone(),
              label: todo_session.label.clone(),
              ambiguous_label: false,
              problems: 0,
              min_rank: i8::MAX,
            });
          }
        }
      }
    }

    // Detect ambiguous labels.
    let mut label_counts: HashMap<String, usize> = HashMap::default();
    for e in &entries {
      if !matches!(e.kind, SessionPickerEntryKind::EmptyProject) {
        *label_counts.entry(e.label.clone()).or_default() += 1;
      }
    }
    for e in &mut entries {
      if !matches!(e.kind, SessionPickerEntryKind::EmptyProject) {
        e.ambiguous_label = label_counts.get(&e.label).copied().unwrap_or(0) > 1;
      }
    }

    // Sort into groups:
    //   0: this project, attached with problems
    //   1: this project, attached without problems (non-active)
    //   2: other projects, attached (grouped by project)
    //   3: other projects, empty (no sessions)
    //   4: this project, detached (unadopted)
    //   5: other projects, detached
    //   6: current active session
    let sort_group = |e: &SessionPickerEntry, idx: usize| -> (u8, usize) {
      let is_this = e.project_index == active_project_index;
      let is_active = active_entry == Some(idx);
      match &e.kind {
        _ if is_active => (6, e.project_index),
        SessionPickerEntryKind::Session(_) if is_this && e.problems > 0 => (0, 0),
        SessionPickerEntryKind::Session(_) if is_this => (1, 0),
        SessionPickerEntryKind::Session(_) => (2, e.project_index),
        SessionPickerEntryKind::EmptyProject => (3, e.project_index),
        SessionPickerEntryKind::Unadopted { .. } if is_this => (4, 0),
        SessionPickerEntryKind::Unadopted { .. } => (5, e.project_index),
      }
    };

    let mut indices: Vec<usize> = (0..entries.len()).collect();
    indices.sort_by(|&a, &b| {
      let ga = sort_group(&entries[a], a);
      let gb = sort_group(&entries[b], b);
      ga.cmp(&gb)
    });

    let sorted_entries: Vec<_> = indices.iter().map(|&i| entries[i].clone()).collect();
    // Recompute active_entry position after sort.
    let active_entry = active_entry.and_then(|old| indices.iter().position(|&i| i == old));

    // Build fuzzy-filterable labels.
    let labels: Vec<String> = sorted_entries
      .iter()
      .map(|e| match &e.kind {
        SessionPickerEntryKind::Session(_) | SessionPickerEntryKind::Unadopted { .. } => {
          format!("{} / {}", e.project_name, e.label)
        }
        SessionPickerEntryKind::EmptyProject => e.project_name.clone(),
      })
      .collect();

    Self { labels, entries: sorted_entries, active_entry, result: None }
  }

  /// Like `new()` but sorted by urgency: sessions with problems first (lowest rank = most urgent).
  /// The first entry is pre-selected so Enter immediately confirms the neediest session.
  pub fn new_urgency_sorted(
    projects: &[ProjectState],
    active_project_index: usize,
    todo_documents: &[&jc_core::todo::TodoDocument],
  ) -> Self {
    let mut delegate = Self::new(projects, active_project_index, todo_documents);

    // Build a permutation sorted by urgency.
    let mut indices: Vec<usize> = (0..delegate.entries.len()).collect();
    indices.sort_by(|&a, &b| {
      let ea = &delegate.entries[a];
      let eb = &delegate.entries[b];
      // Primary: has_problems DESC (problems first).
      let pa = if ea.problems > 0 { 0 } else { 1 };
      let pb = if eb.problems > 0 { 0 } else { 1 };
      pa.cmp(&pb)
        // Secondary: min_rank ASC (most severe first).
        .then(ea.min_rank.cmp(&eb.min_rank))
        // Tertiary: original order.
        .then(a.cmp(&b))
    });

    let sorted_entries: Vec<_> = indices.iter().map(|&i| delegate.entries[i].clone()).collect();
    let sorted_labels: Vec<_> = indices.iter().map(|&i| delegate.labels[i].clone()).collect();

    delegate.entries = sorted_entries;
    delegate.labels = sorted_labels;
    delegate.active_entry = Some(0);
    delegate
  }

  pub fn confirmed_entry(&self) -> Option<SessionPickerResult> {
    self.result.clone()
  }
}

impl PickerDelegate for SessionPickerDelegate {
  fn items(&self) -> &[String] {
    &self.labels
  }

  fn confirm(&mut self, index: usize, _window: &mut Window, _cx: &mut Context<PickerState<Self>>) {
    let e = &self.entries[index];
    self.result = Some(match &e.kind {
      SessionPickerEntryKind::Session(id) => {
        SessionPickerResult::Session(e.project_index, *id)
      }
      SessionPickerEntryKind::Unadopted { uuid, .. } => {
        SessionPickerResult::Adopt(e.project_index, uuid.clone(), e.label.clone())
      }
      SessionPickerEntryKind::EmptyProject => SessionPickerResult::InitProject(e.project_index),
    });
  }

  fn delete(&mut self, index: usize, _window: &mut Window, cx: &mut Context<PickerState<Self>>) {
    let e = &self.entries[index];
    match &e.kind {
      SessionPickerEntryKind::Session(_) | SessionPickerEntryKind::Unadopted { .. } => {
        self.result = Some(SessionPickerResult::ToggleDisabled(
          e.project_index,
          e.label.clone(),
        ));
        cx.emit(PickerEvent::Confirmed);
      }
      _ => {}
    }
  }

  fn render_item(&self, index: usize, selected: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let entry = &self.entries[index];
    let is_active = self.active_entry == Some(index);
    let has_problems = entry.problems > 0;

    let row = div().px_2().py(px(3.0)).text_sm().font_family("Lilex").flex().items_center().gap_1();
    let row = if selected { row.bg(theme.accent).text_color(theme.accent_foreground) } else { row };

    let marker = match &entry.kind {
      SessionPickerEntryKind::EmptyProject => {
        let color = if selected { theme.accent_foreground } else { theme.blue };
        picker_marker_base().text_color(color).child("+")
      }
      SessionPickerEntryKind::Unadopted { disabled: true, .. } => {
        let color = if selected { theme.accent_foreground } else { theme.muted_foreground };
        picker_marker_base().text_color(color).child("~")
      }
      SessionPickerEntryKind::Unadopted { disabled: false, .. } => {
        let color = if selected { theme.accent_foreground } else { theme.yellow };
        picker_marker_base().text_color(color).child("~")
      }
      SessionPickerEntryKind::Session(_) if has_problems => {
        let color = if selected { theme.accent_foreground } else { theme.red };
        picker_marker_base().text_color(color).child(format!("{}", entry.problems))
      }
      SessionPickerEntryKind::Session(_) if is_active => {
        let color = if selected { theme.accent_foreground } else { theme.green };
        picker_marker_base().text_color(color).child(">")
      }
      SessionPickerEntryKind::Session(_) => picker_marker_base(),
    };

    let main_text = match &entry.kind {
      SessionPickerEntryKind::Session(_) | SessionPickerEntryKind::Unadopted { .. } => {
        format!("{} / {}", entry.project_name, entry.label)
      }
      SessionPickerEntryKind::EmptyProject => entry.project_name.clone(),
    };

    let muted_color = if selected { theme.accent_foreground } else { theme.muted_foreground };
    let right = match &entry.kind {
      SessionPickerEntryKind::EmptyProject => "(no sessions)".to_string(),
      SessionPickerEntryKind::Unadopted { disabled: true, .. } => "(disabled)".to_string(),
      SessionPickerEntryKind::Unadopted { disabled: false, .. } => "(adopt)".to_string(),
      _ => String::new(),
    };
    let right_el = div().ml_auto().text_xs().text_color(muted_color).child(right);

    row.child(marker).child(main_text).child(right_el)
  }

  fn render_footer(&self, cx: &App) -> Option<Div> {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    Some(
      div().px_2().py_1().flex().flex_wrap().gap_x_3().gap_y_0p5().text_xs().text_color(muted).children([
        legend_item(">", theme.green, "active", muted),
        legend_item("~", theme.yellow, "adopt", muted),
        legend_item("~", theme.muted_foreground, "disabled", muted),
        legend_item("+", theme.blue, "new", muted),
        div().child("Cmd-Shift-Bksp: toggle disable"),
      ])
    )
  }
}

// ---------------------------------------------------------------------------
// ProjectActionsPickerDelegate
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub enum ProjectActionsResult {
  SwitchToSession(usize, SessionId),
  AdoptTodoSession(usize, String, String), // pi, uuid, label
  InitProject(usize),
  CreateNew,
  AdoptJsonlSession(String, String), // uuid, label
}

enum ProjectActionsEntry {
  Problem { pi: usize, kind: SessionPickerEntryKind, project_name: String, label: String, problems: usize, min_rank: i8 },
  /// A TODO.md session that isn't currently running.
  Dormant { uuid: String, label: String },
  NewSession,
  /// A JSONL session not referenced in TODO.md.
  Unattached { uuid: String, summary: String },
}

pub struct ProjectActionsPickerDelegate {
  labels: Vec<String>,
  entries: Vec<ProjectActionsEntry>,
  active_project_index: usize,
  result: Option<ProjectActionsResult>,
}

impl ProjectActionsPickerDelegate {
  pub fn new(
    projects: &[ProjectState],
    active_project_index: usize,
    todo_documents: &[&jc_core::todo::TodoDocument],
    project_path: &Path,
  ) -> Self {
    use std::collections::HashSet;

    let mut labels = Vec::new();
    let mut entries = Vec::new();

    // 1. Problem entries: sessions with problems, sorted by urgency.
    {
      struct ProblemCandidate {
        pi: usize,
        kind: SessionPickerEntryKind,
        project_name: String,
        label: String,
        problems: usize,
        min_rank: i8,
      }

      let mut candidates: Vec<ProblemCandidate> = Vec::new();
      let pi = active_project_index;
      let project = &projects[pi];

      if project.sessions.is_empty()
        && todo_documents.get(pi).map_or(true, |d| d.sessions.is_empty())
      {
        if !project.problems.is_empty() {
          let min_rank = project.problems.iter().map(|p| p.rank()).min().unwrap_or(i8::MAX);
          candidates.push(ProblemCandidate {
            pi,
            kind: SessionPickerEntryKind::EmptyProject,
            project_name: project.name.clone(),
            label: String::new(),
            problems: project.problems.len(),
            min_rank,
          });
        }
      } else {
        // Adopted sessions with problems.
        for (&id, session) in &project.sessions {
          let total_problems = session.problems.len() + project.problems.len();
          if total_problems > 0 {
            let min_rank = session
              .problems
              .iter()
              .map(|p| p.rank())
              .chain(project.problems.iter().map(|p| p.rank()))
              .min()
              .unwrap_or(i8::MAX);
            candidates.push(ProblemCandidate {
              pi,
              kind: SessionPickerEntryKind::Session(id),
              project_name: project.name.clone(),
              label: session.label.clone(),
              problems: total_problems,
              min_rank,
            });
          }
        }
      }

      // Sort by urgency: min_rank ASC.
      candidates.sort_by(|a, b| a.min_rank.cmp(&b.min_rank).then(a.pi.cmp(&b.pi)));

      for c in candidates {
        let label_text = match &c.kind {
          SessionPickerEntryKind::EmptyProject => format!("! {}", c.project_name),
          _ => format!("! {}", c.label),
        };
        labels.push(label_text);
        entries.push(ProjectActionsEntry::Problem {
          pi: c.pi,
          kind: c.kind,
          project_name: c.project_name,
          label: c.label,
          problems: c.problems,
          min_rank: c.min_rank,
        });
      }
    }

    // 2. Dormant sessions: in TODO.md but not currently running.
    {
      let adopted_uuids: HashSet<&str> = projects[active_project_index]
        .sessions
        .values()
        .filter_map(|s| s.uuid.as_deref())
        .collect();
      let adopted_labels: HashSet<&str> = projects[active_project_index]
        .sessions
        .values()
        .map(|s| s.label.as_str())
        .collect();

      if let Some(doc) = todo_documents.get(active_project_index) {
        for ts in &doc.sessions {
          if ts.uuid.is_empty() {
            continue;
          }
          let uuid_adopted = adopted_uuids.contains(ts.uuid.as_str());
          let label_adopted = adopted_labels.contains(ts.label.as_str());
          if !uuid_adopted && !label_adopted {
            labels.push(format!("* {}", ts.label));
            entries.push(ProjectActionsEntry::Dormant {
              uuid: ts.uuid.clone(),
              label: ts.label.clone(),
            });
          }
        }
      }
    }

    // 3. New session.
    labels.push("+ New session".to_string());
    entries.push(ProjectActionsEntry::NewSession);

    // 4. Unattached JSONL sessions for current project (not in TODO.md).
    {
      let existing_uuids: HashSet<&str> = todo_documents
        .get(active_project_index)
        .map(|doc| doc.sessions.iter().map(|s| s.uuid.as_str()).collect())
        .unwrap_or_default();

      let encoded = project_path.to_string_lossy().replace('/', "-");
      let home = std::env::var("HOME").unwrap_or_default();
      let session_dir = PathBuf::from(home).join(".claude/projects").join(encoded);

      if let Ok(read_dir) = std::fs::read_dir(&session_dir) {
        let mut discovered: Vec<(String, String, std::time::SystemTime)> = Vec::new();

        for entry in read_dir.flatten() {
          let path = entry.path();
          if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
          }
          let uuid = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
          };
          if existing_uuids.contains(uuid.as_str()) {
            continue;
          }
          let mtime = entry.metadata().ok().and_then(|m| m.modified().ok());
          let summary = extract_first_user_summary(&path).unwrap_or_else(|| uuid[..8].to_string());
          discovered.push((uuid, summary, mtime.unwrap_or(std::time::UNIX_EPOCH)));
        }

        // Sort newest first.
        discovered.sort_by(|a, b| b.2.cmp(&a.2));

        for (uuid, summary, mtime) in discovered {
          let age = format_relative_time(mtime);
          let label_text = format!("~ {summary} ({age})");
          labels.push(label_text);
          entries.push(ProjectActionsEntry::Unattached { uuid, summary });
        }
      }
    }

    Self { labels, entries, active_project_index, result: None }
  }

  pub fn result(&self) -> Option<ProjectActionsResult> {
    self.result.clone()
  }
}

impl PickerDelegate for ProjectActionsPickerDelegate {
  fn items(&self) -> &[String] {
    &self.labels
  }

  fn confirm(&mut self, index: usize, _window: &mut Window, _cx: &mut Context<PickerState<Self>>) {
    let entry = &self.entries[index];
    self.result = Some(match entry {
      ProjectActionsEntry::Problem { pi, kind, label, .. } => match kind {
        SessionPickerEntryKind::Session(id) => ProjectActionsResult::SwitchToSession(*pi, *id),
        SessionPickerEntryKind::Unadopted { uuid, .. } => {
          ProjectActionsResult::AdoptTodoSession(*pi, uuid.clone(), label.clone())
        }
        SessionPickerEntryKind::EmptyProject => ProjectActionsResult::InitProject(*pi),
      },
      ProjectActionsEntry::Dormant { uuid, label } => {
        ProjectActionsResult::AdoptTodoSession(self.active_project_index, uuid.clone(), label.clone())
      }
      ProjectActionsEntry::NewSession => ProjectActionsResult::CreateNew,
      ProjectActionsEntry::Unattached { uuid, summary } => {
        ProjectActionsResult::AdoptJsonlSession(uuid.clone(), summary.clone())
      }
    });
  }

  fn render_item(&self, index: usize, selected: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let row = div().px_2().py(px(3.0)).text_sm().font_family("Lilex").flex().items_center().gap_1();
    let row = if selected { row.bg(theme.accent).text_color(theme.accent_foreground) } else { row };

    match &self.entries[index] {
      ProjectActionsEntry::Problem { project_name, label, problems, kind, .. } => {
        let marker_color = if selected { theme.accent_foreground } else { theme.red };
        let marker = picker_marker_base().text_color(marker_color).child("!");
        let main_text = match kind {
          SessionPickerEntryKind::EmptyProject => project_name.clone(),
          _ => label.clone(),
        };
        let muted_color = if selected { theme.accent_foreground } else { theme.muted_foreground };
        let right_el = div().ml_auto().text_xs().text_color(muted_color).child(format!("{problems}"));
        row.child(marker).child(main_text).child(right_el)
      }
      ProjectActionsEntry::Dormant { label, .. } => {
        let marker_color = if selected { theme.accent_foreground } else { theme.cyan };
        let marker = picker_marker_base().text_color(marker_color).child("*");
        row.child(marker).child(label.clone())
      }
      ProjectActionsEntry::NewSession => {
        let marker_color = if selected { theme.accent_foreground } else { theme.green };
        let marker = picker_marker_base().text_color(marker_color).child("+");
        row.child(marker).child("New session".to_string())
      }
      ProjectActionsEntry::Unattached { summary, .. } => {
        let marker_color = if selected { theme.accent_foreground } else { theme.yellow };
        let marker = picker_marker_base().text_color(marker_color).child("~");
        row.child(marker).child(summary.clone())
      }
    }
  }

  fn render_footer(&self, cx: &App) -> Option<Div> {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    Some(
      div().px_2().py_1().flex().flex_wrap().gap_x_3().gap_y_0p5().text_xs().text_color(muted).children([
        legend_item("!", theme.red, "problem", muted),
        legend_item("*", theme.cyan, "dormant", muted),
        legend_item("+", theme.green, "new", muted),
        legend_item("~", theme.yellow, "unattached", muted),
      ])
    )
  }
}

// ---------------------------------------------------------------------------
// LineSearchPickerDelegate
// ---------------------------------------------------------------------------

type ScrollCallback = Box<dyn Fn(u32, &mut Window, &mut App)>;

struct LineEntry {
  /// 1-based line number.
  line_number: u32,
  /// Byte offset of the line start in the original text.
  byte_start: usize,
  /// The line content (without trailing newline).
  content: String,
}

pub struct LineSearchPickerDelegate {
  labels: Vec<String>,
  entries: Vec<LineEntry>,
  /// Syntax highlighter with the parsed tree — styles are queried on demand in render_item.
  highlighter: SyntaxHighlighter,
  scroll_to_line: ScrollCallback,
}

impl LineSearchPickerDelegate {
  pub fn for_view<V: super::LineSearchable>(view: &Entity<V>, cx: &App) -> Self {
    let text = view.read(cx).editor_text(cx);
    let language = view.read(cx).language_name().name();
    let entity = view.clone();
    let callback: ScrollCallback = Box::new(move |line, window, cx| {
      entity.update(cx, |v, cx| v.scroll_to_line(line, window, cx));
    });

    let rope = Rope::from(text.as_str());
    let mut highlighter = SyntaxHighlighter::new(language);
    highlighter.update(None, &rope);

    let mut entries = Vec::new();
    let mut labels = Vec::new();
    let mut byte_offset: usize = 0;

    for (i, line) in text.split('\n').enumerate() {
      let line_number = (i as u32) + 1;
      let line_byte_start = byte_offset;

      // Skip empty/whitespace-only lines.
      if !line.trim().is_empty() {
        labels.push(format!("{line_number}: {line}"));
        entries.push(LineEntry {
          line_number,
          byte_start: line_byte_start,
          content: line.to_string(),
        });
      }

      // +1 for the '\n' separator (or end of string).
      byte_offset += line.len() + 1;
    }

    Self { labels, entries, highlighter, scroll_to_line: callback }
  }
}

impl PickerDelegate for LineSearchPickerDelegate {
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

    // Build syntax-highlighted line content using StyledText (computed lazily per visible item).
    let line_text: SharedString = entry.content.clone().into();
    let styled = if selected {
      // When selected, use accent foreground — no syntax colors.
      StyledText::new(line_text)
    } else {
      let byte_end = entry.byte_start + entry.content.len();
      let raw_styles =
        self.highlighter.styles(&(entry.byte_start..byte_end), &theme.highlight_theme);
      let styles: Vec<(Range<usize>, HighlightStyle)> = raw_styles
        .into_iter()
        .filter_map(|(r, s)| {
          let start = r.start.saturating_sub(entry.byte_start);
          let end = r.end.saturating_sub(entry.byte_start).min(entry.content.len());
          if start < end { Some((start..end, s)) } else { None }
        })
        .collect();
      let default_style = TextStyle {
        font_family: "Lilex".into(),
        font_size: theme.font_size.into(),
        color: theme.foreground,
        ..Default::default()
      };
      StyledText::new(line_text).with_default_highlights(&default_style, styles)
    };

    row.child(line_num).child(styled)
  }
}

// ---------------------------------------------------------------------------
// SnippetPickerDelegate
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnippetTarget {
  TodoCursor,
  TodoWait,
  ClaudeTerminal,
}

pub struct SnippetPickerDelegate {
  items: Vec<String>,
  snippets: Vec<Snippet>,
  todo_view: Entity<TodoView>,
  active_label: Option<String>,
  claude_terminal: Option<Entity<TerminalView>>,
  insert_target: SnippetTarget,
}

impl SnippetPickerDelegate {
  pub fn new(
    snippets: Vec<Snippet>,
    todo_view: Entity<TodoView>,
    active_label: Option<String>,
    claude_terminal: Option<Entity<TerminalView>>,
    insert_target: SnippetTarget,
  ) -> Self {
    let items: Vec<String> = snippets.iter().map(|s| s.heading.clone()).collect();
    Self { items, snippets, todo_view, active_label, claude_terminal, insert_target }
  }
}

impl PickerDelegate for SnippetPickerDelegate {
  fn items(&self) -> &[String] {
    &self.items
  }

  fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<PickerState<Self>>) {
    let snippet = &self.snippets[index];
    let text = &snippet.content;
    if text.is_empty() {
      return;
    }

    match self.insert_target {
      SnippetTarget::TodoCursor => {
        self.todo_view.update(cx, |tv, cx| {
          tv.insert_at_cursor(text, window, cx);
        });
      }
      SnippetTarget::TodoWait => {
        if let Some(label) = &self.active_label {
          let comment = format!("{text}\n");
          self.todo_view.update(cx, |tv, cx| {
            tv.insert_comment(label, &comment, window, cx);
          });
        }
      }
      SnippetTarget::ClaudeTerminal => {
        if let Some(terminal) = &self.claude_terminal {
          terminal.read(cx).write_text(text);
        }
      }
    }
  }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Extract a short summary from a JSONL file (first informative user message).
fn extract_first_user_summary(path: &Path) -> Option<String> {
  use std::io::{BufRead, BufReader};

  let file = std::fs::File::open(path).ok()?;
  let reader = BufReader::new(file);

  for line in reader.lines().take(200) {
    let line = line.ok()?;
    if !line.contains("\"user\"") {
      continue;
    }
    let entry: serde_json::Value = serde_json::from_str(&line).ok()?;
    if entry.get("type").and_then(|t| t.as_str()) != Some("user") {
      continue;
    }
    let msg = entry.get("message")?;
    if msg.get("role").and_then(|r| r.as_str()) != Some("user") {
      continue;
    }
    let text = match msg.get("content")? {
      serde_json::Value::String(s) => s.clone(),
      serde_json::Value::Array(arr) => arr
        .iter()
        .filter_map(|item| {
          let obj = item.as_object()?;
          if obj.get("type")?.as_str()? == "text" {
            obj.get("text")?.as_str().map(String::from)
          } else {
            None
          }
        })
        .collect::<Vec<_>>()
        .join("\n"),
      _ => continue,
    };
    for l in text.lines() {
      let l = l.trim();
      if l.is_empty() || l.starts_with('<') || l.contains("Implement the following plan") {
        continue;
      }
      let l = l.trim_start_matches('#').trim_start();
      if l.is_empty() {
        continue;
      }
      let truncated = if l.len() > 80 {
        format!("{}...", &l[..l.floor_char_boundary(80)])
      } else {
        l.to_string()
      };
      return Some(truncated);
    }
  }
  None
}

fn format_relative_time(time: std::time::SystemTime) -> String {
  let secs = time.elapsed().unwrap_or_default().as_secs();
  match secs {
    0..60 => "just now".to_string(),
    60..3600 => format!("{}m ago", secs / 60),
    3600..86400 => format!("{}h ago", secs / 3600),
    _ => format!("{}d ago", secs / 86400),
  }
}
