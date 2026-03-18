use crate::file_watcher::watch_dir;
use crate::views::comment_panel::{CommentPanel, CommentPanelEvent};
use crate::views::pane::PaneContentKind;
use crate::views::picker::{
  CodeSymbolPickerDelegate, DiffFilePickerDelegate, FilePickerDelegate, GitLogPickerDelegate,
  LineSearchPickerDelegate, OpenContextPicker, OpenFilePicker, PickerEvent, PickerState,
  SearchLines, SessionPickerDelegate,
  SessionPickerResult, ShowSessionPicker, ShowSnippetPicker, ShowViewPicker,
  SnippetPickerDelegate, SnippetTarget, TodoHeaderPickerDelegate,
  ViewPickerDelegate,
};
use gpui::*;
use jc_core::snippets;

use super::{OpenCommentPanel, OpenGitLogPicker, Workspace};

impl Workspace {
  pub(super) fn setup_snippet_watcher(
    window: &Window,
    cx: &mut Context<Self>,
  ) -> Option<notify::RecommendedWatcher> {
    let path = snippets::snippet_file_path();
    let watched_file = path.file_name()?.to_os_string();
    let parent = path.parent()?.to_path_buf();

    watch_dir(
      &parent,
      move |p| p.ends_with(&watched_file),
      None,
      |view, _window, _cx| {
        view.snippets = snippets::load();
      },
      window,
      cx,
    )
  }

  pub(super) fn show_view_picker(
    &mut self,
    _: &ShowViewPicker,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.active_picker.is_some() {
      return;
    }

    let delegate = ViewPickerDelegate::new();
    let picker = cx.new(|cx| PickerState::new(delegate, window, cx));
    self.pre_picker_focus = window.focused(cx);

    let subscription =
      cx.subscribe_in(&picker, window, move |this: &mut Self, picker_entity, event, window, cx| {
        match event {
          PickerEvent::Confirmed => {
            let kind = picker_entity.read(cx).delegate().confirmed_kind();
            this.pre_picker_focus.take();
            this.dismiss_picker();
            if let Some(kind) = kind {
              this.set_active_pane_view(kind, window, cx);
            }
            cx.notify();
          }
          PickerEvent::Dismissed => {
            if let Some(focus) = this.pre_picker_focus.take() {
              focus.focus(window);
            }
            this.dismiss_picker();
            cx.notify();
          }
        }
      });

    self.active_picker = Some(picker.clone().into());
    self._picker_subscription = Some(subscription);

    picker.read(cx).input_focus_handle(cx).focus(window);
    cx.notify();
  }

  pub(super) fn open_session_picker(
    &mut self,
    _: &ShowSessionPicker,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.active_picker.is_some() {
      return;
    }

    let docs = self.todo_documents(cx);
    let delegate = SessionPickerDelegate::new(&self.projects, self.active_project_index, &docs);
    self.show_session_picker(delegate, window, cx);
  }

  pub(super) fn show_session_picker(
    &mut self,
    delegate: SessionPickerDelegate,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let picker = cx.new(|cx| PickerState::new(delegate, window, cx));
    self.pre_picker_focus = window.focused(cx);

    let subscription =
      cx.subscribe_in(&picker, window, move |this: &mut Self, picker_entity, event, window, cx| {
        match event {
          PickerEvent::Confirmed => {
            let Some(result) = picker_entity.read(cx).delegate().confirmed_entry() else {
              return;
            };
            // switch_to_session / init both set focus; drop stale pre_picker_focus.
            this.pre_picker_focus.take();
            this.dismiss_picker();
            match result {
              SessionPickerResult::Session(pi, id) => {
                this.switch_to_session(pi, Some(id), window, cx);
              }
              SessionPickerResult::Adopt(pi, uuid, label) => {
                this.adopt_session(pi, &uuid, &label, window, cx);
              }
              SessionPickerResult::InitProject(pi) => {
                this.init_empty_project(pi, window, cx);
              }
              SessionPickerResult::Removed(pi, id) => {
                this.remove_session(pi, id, window, cx);
              }
            }
            cx.notify();
          }
          PickerEvent::Dismissed => {
            if let Some(focus) = this.pre_picker_focus.take() {
              focus.focus(window);
            }
            this.dismiss_picker();
            cx.notify();
          }
        }
      });

    self.active_picker = Some(picker.clone().into());
    self._picker_subscription = Some(subscription);

    picker.read(cx).input_focus_handle(cx).focus(window);
    cx.notify();
  }

  // ---------------------------------------------------------------------------
  // Pickers
  // ---------------------------------------------------------------------------

  pub(super) fn open_file_picker(
    &mut self,
    _: &OpenFilePicker,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.active_picker.is_some() {
      return;
    }

    let project = self.active_project();
    let delegate = FilePickerDelegate::new(
      project.path.clone(),
      project.code_view.clone(),
      self.recent_files.clone(),
    );
    self.show_picker_with_confirm(delegate, Some(PaneContentKind::CodeViewer), window, cx);
  }

  pub(super) fn open_context_picker(
    &mut self,
    _: &OpenContextPicker,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.active_picker.is_some() {
      return;
    }

    let kind = self.active_pane_entity().read(cx).content_kind();
    let project = self.active_project();

    match kind {
      Some(PaneContentKind::GitDiff) => {
        self.open_diff_picker(window, cx);
      }
      Some(PaneContentKind::TodoEditor) => {
        let delegate = TodoHeaderPickerDelegate::new(project.todo_view.clone(), cx);
        self.show_picker(delegate, window, cx);
      }
      Some(PaneContentKind::CodeViewer) => {
        let delegate = CodeSymbolPickerDelegate::new(project.code_view.clone(), cx);
        self.show_picker(delegate, window, cx);
      }
      _ => {}
    }
  }

  pub(super) fn open_diff_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.active_picker.is_some() {
      return;
    }
    let delegate = DiffFilePickerDelegate::new(self.active_project().diff_view.clone(), cx);
    self.show_picker(delegate, window, cx);
  }

  pub(super) fn open_git_log_picker(
    &mut self,
    _: &OpenGitLogPicker,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.active_picker.is_some() {
      return;
    }

    let delegate = GitLogPickerDelegate::new(self.active_project().diff_view.clone(), cx);
    self.show_picker_with_confirm(delegate, Some(PaneContentKind::GitDiff), window, cx);
  }

  pub(super) fn search_lines(
    &mut self,
    _: &SearchLines,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.active_picker.is_some() {
      return;
    }

    let kind = self.active_pane_entity().read(cx).content_kind();
    let project = self.active_project();

    match kind {
      Some(PaneContentKind::CodeViewer) => {
        let delegate = LineSearchPickerDelegate::for_view(&project.code_view, cx);
        self.show_picker(delegate, window, cx);
      }
      Some(PaneContentKind::TodoEditor) => {
        let delegate = LineSearchPickerDelegate::for_view(&project.todo_view, cx);
        self.show_picker(delegate, window, cx);
      }
      Some(PaneContentKind::GitDiff) => {
        let delegate = LineSearchPickerDelegate::for_view(&project.diff_view, cx);
        self.show_picker(delegate, window, cx);
      }
      _ => {}
    }
  }

  fn show_picker_with_confirm<D: crate::views::picker::PickerDelegate>(
    &mut self,
    delegate: D,
    switch_pane: Option<PaneContentKind>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let picker = cx.new(|cx| PickerState::new(delegate, window, cx));
    self.pre_picker_focus = window.focused(cx);

    let subscription =
      cx.subscribe_in(&picker, window, move |this: &mut Self, _, event, window, cx| match event {
        PickerEvent::Confirmed => {
          if let Some(path) = this.active_project().code_view.read(cx).file_path() {
            let path = path.to_path_buf();
            this.recent_files.retain(|p| p != &path);
            this.recent_files.insert(0, path);
            this.recent_files.truncate(50);
          }
          if let Some(kind) = switch_pane {
            this.pre_picker_focus.take();
            this.set_active_pane_view(kind, window, cx);
          } else if let Some(focus) = this.pre_picker_focus.take() {
            focus.focus(window);
          }
          this.dismiss_picker();
          cx.notify();
        }
        PickerEvent::Dismissed => {
          if let Some(focus) = this.pre_picker_focus.take() {
            focus.focus(window);
          }
          this.dismiss_picker();
          cx.notify();
        }
      });

    self.active_picker = Some(picker.clone().into());
    self._picker_subscription = Some(subscription);

    picker.read(cx).input_focus_handle(cx).focus(window);
    cx.notify();
  }

  fn show_picker<D: crate::views::picker::PickerDelegate>(
    &mut self,
    delegate: D,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.show_picker_with_confirm(delegate, None, window, cx);
  }

  pub(super) fn dismiss_picker(&mut self) {
    self.active_picker = None;
    self._picker_subscription = None;
  }

  // ---------------------------------------------------------------------------
  // Comment panel
  // ---------------------------------------------------------------------------

  pub(super) fn open_comment_panel(
    &mut self,
    _: &OpenCommentPanel,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.active_comment_panel.is_some() || self.active_picker.is_some() {
      return;
    }

    let kind = self.active_pane_entity().read(cx).content_kind();
    let project = self.active_project();

    let context = match kind {
      Some(PaneContentKind::CodeViewer) => {
        project.code_view.read(cx).comment_context(&project.path, cx)
      }
      Some(PaneContentKind::GitDiff) => project.diff_view.read(cx).comment_context(cx),
      _ => None,
    };

    let Some(context) = context else { return };

    // Save focus before creating the panel — CommentPanel::new calls
    // set_cursor_position which steals focus to the panel's input.
    self.pre_comment_focus = window.focused(cx);
    let panel = cx.new(|cx| CommentPanel::new(context, window, cx));

    let subscription = cx.subscribe_in(&panel, window, |this: &mut Self, _, event, window, cx| {
      if let CommentPanelEvent::Confirmed(text) = event {
        // Insert comment into active session's WAIT section.
        let project = &this.projects[this.active_project_index];
        if let Some(session) = project.active_session() {
          let comment = format!("{text}\n");
          let label = session.label.clone();
          project.todo_view.update(cx, |tv, cx| {
            tv.insert_comment(&label, &comment, window, cx);
            tv.save(cx);
          });
        }
      }
      if let Some(focus) = this.pre_comment_focus.take() {
        focus.focus(window);
      }
      this.dismiss_comment_panel();
      cx.notify();
    });

    self.active_comment_panel = Some(panel.clone().into());
    self._comment_subscription = Some(subscription);

    panel.read(cx).input_focus_handle(cx).focus(window);
    cx.notify();
  }

  fn dismiss_comment_panel(&mut self) {
    self.active_comment_panel = None;
    self._comment_subscription = None;
  }

  // ---------------------------------------------------------------------------
  // Snippet picker
  // ---------------------------------------------------------------------------

  pub(super) fn show_snippet_picker(
    &mut self,
    _: &ShowSnippetPicker,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.active_picker.is_some() || self.snippets.items.is_empty() {
      return;
    }

    let kind = self.active_pane_entity().read(cx).content_kind();
    let project = self.active_project();

    let insert_target = match kind {
      Some(PaneContentKind::TodoEditor) => SnippetTarget::TodoCursor,
      Some(PaneContentKind::ClaudeTerminal) => SnippetTarget::ClaudeTerminal,
      _ => SnippetTarget::TodoWait,
    };

    let active_label = project.active_label().map(str::to_string);
    let claude_terminal = project.active_session().map(|s| s.claude_terminal.clone());

    let delegate = SnippetPickerDelegate::new(
      self.snippets.items.clone(),
      project.todo_view.clone(),
      active_label,
      claude_terminal,
      insert_target,
    );
    self.show_picker(delegate, window, cx);
  }
}
