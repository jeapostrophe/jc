use crate::views::pane::PaneContentKind;
use crate::views::picker::{
  CodeSymbolPickerDelegate, DrillDownPicker, LineSearchPickerDelegate, OpenPicker,
  OpenPickerDelegate, OpenPickerResult, PickerEvent, PickerState, ProjectActionsPicker,
  ProjectActionsPickerDelegate, ProjectActionsResult, SearchLines, SessionPickerDelegate,
  SessionPickerResult, ShowSessionPicker, TodoHeaderPickerDelegate,
};
use gpui::*;

use super::Workspace;

impl Workspace {
  pub(super) fn open_picker(
    &mut self,
    _: &OpenPicker,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.active_picker.is_some() {
      return;
    }

    let project = self.active_project();
    let Some(code_view) = project.code_view().cloned() else { return };
    let delegate =
      OpenPickerDelegate::new(project.path.clone(), code_view, self.recent_files.clone());
    let picker = cx.new(|cx| PickerState::new(delegate, window, cx));
    self.pre_picker_focus = window.focused(cx);

    let subscription =
      cx.subscribe_in(&picker, window, move |this: &mut Self, picker_entity, event, window, cx| {
        match event {
          PickerEvent::Confirmed => {
            let result = picker_entity.read(cx).delegate().result();
            match result {
              Some(OpenPickerResult::SwitchPane(kind)) => {
                this.pre_picker_focus.take();
                this.dismiss_picker();
                this.set_active_pane_view(*kind, window, cx);
              }
              Some(OpenPickerResult::OpenFile) => {
                // Track the opened file in recent_files
                if let Some(path) = this
                  .active_project()
                  .code_view()
                  .and_then(|cv| cv.read(cx).file_path().map(|p| p.to_path_buf()))
                {
                  this.recent_files.retain(|p| p != &path);
                  this.recent_files.insert(0, path);
                  this.recent_files.truncate(50);
                }
                this.pre_picker_focus.take();
                this.dismiss_picker();
                this.set_active_pane_view(PaneContentKind::CodeViewer, window, cx);
              }
              None => {
                this.dismiss_picker();
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
    let delegate = SessionPickerDelegate::new(&self.projects, self.active_project_index, &docs, cx);
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
              // The row produced no action (an `EmptyProject` row has no heading
              // to address). Close out as a dismissal rather than returning with
              // the picker still up and focus stranded on its input.
              if let Some(focus) = this.pre_picker_focus.take() {
                focus.focus(window);
              }
              this.dismiss_picker();
              cx.notify();
              return;
            };
            // switch_to_session / init both set focus; drop stale pre_picker_focus.
            this.pre_picker_focus.take();
            this.dismiss_picker();
            match result {
              SessionPickerResult::Session(pi, id) => {
                this.switch_to_session(pi, Some(id), window, cx);
              }
              SessionPickerResult::Adopt(pi, key) => {
                this.adopt_session(pi, &key, window, cx);
              }
              SessionPickerResult::InitProject(pi) => {
                this.create_new_session(pi, window, cx);
              }
              SessionPickerResult::ToggleDisabled(pi, key) => {
                this.toggle_session_disabled(pi, &key, window, cx);
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

  pub(super) fn open_drill_down_picker(
    &mut self,
    _: &DrillDownPicker,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.active_picker.is_some() {
      return;
    }

    let kind = self.active_pane_entity().read(cx).content_kind();
    let project = self.active_project();

    match kind {
      Some(PaneContentKind::TodoEditor) => {
        let delegate = TodoHeaderPickerDelegate::new(project.todo_view.clone(), cx);
        self.show_picker(delegate, window, cx);
      }
      Some(PaneContentKind::CodeViewer) => {
        if let Some(cv) = project.code_view() {
          let delegate = CodeSymbolPickerDelegate::new(cv.clone(), cx);
          self.show_picker(delegate, window, cx);
        }
      }
      Some(PaneContentKind::GlobalTodo) => {
        // GlobalTodo is a CodeView; use CodeSymbolPickerDelegate for markdown headers
        let delegate = CodeSymbolPickerDelegate::new(self.global_todo_view.clone(), cx);
        self.show_picker(delegate, window, cx);
      }
      _ => {} // Claude/Terminal: no-op
    }
  }

  pub(super) fn open_project_actions_picker(
    &mut self,
    _: &ProjectActionsPicker,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.active_picker.is_some() {
      return;
    }

    let docs = self.todo_documents(cx);
    let project_path = self.projects[self.active_project_index].path.clone();
    let delegate = ProjectActionsPickerDelegate::new(
      &self.projects,
      self.active_project_index,
      &docs,
      &project_path,
    );

    let picker = cx.new(|cx| PickerState::new(delegate, window, cx));
    self.pre_picker_focus = window.focused(cx);

    let pi = self.active_project_index;
    let subscription =
      cx.subscribe_in(&picker, window, move |this: &mut Self, picker_entity, event, window, cx| {
        match event {
          PickerEvent::Confirmed => {
            let Some(result) = picker_entity.read(cx).delegate().result() else {
              return;
            };
            this.pre_picker_focus.take();
            this.dismiss_picker();
            match result {
              ProjectActionsResult::AdoptTodoSession(pi, key) => {
                this.adopt_session(pi, &key, window, cx);
              }
              ProjectActionsResult::CreateNew => {
                this.create_new_session(pi, window, cx);
              }
              ProjectActionsResult::AdoptJsonlSession(uuid, summary) => {
                // The summary is the transcript's first user message. Two
                // sessions that opened with the same prompt get the same
                // display name, which is harmless: the heading is addressed by
                // the UUID written with it (see `jc_core::todo::SessionKey`).
                let todo_view = this.projects[pi].todo_view.clone();
                todo_view.update(cx, |tv, cx| {
                  tv.insert_session_heading(&uuid, &summary, window, cx);
                  tv.save(cx);
                });
                let key = jc_core::todo::SessionKey::Uuid(uuid);
                this.adopt_session(pi, &key, window, cx);
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
        if let Some(cv) = project.code_view() {
          let delegate = LineSearchPickerDelegate::for_view(cv, cx);
          self.show_picker(delegate, window, cx);
        }
      }
      Some(PaneContentKind::TodoEditor) => {
        let delegate = LineSearchPickerDelegate::for_view(&project.todo_view, cx);
        self.show_picker(delegate, window, cx);
      }
      _ => {}
    }
  }

  fn show_picker<D: crate::views::picker::PickerDelegate>(
    &mut self,
    delegate: D,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let picker = cx.new(|cx| PickerState::new(delegate, window, cx));
    self.pre_picker_focus = window.focused(cx);

    let subscription =
      cx.subscribe_in(&picker, window, move |this: &mut Self, _, event, window, cx| match event {
        PickerEvent::Confirmed => {
          if let Some(path) = this
            .active_project()
            .code_view()
            .and_then(|cv| cv.read(cx).file_path().map(|p| p.to_path_buf()))
          {
            this.recent_files.retain(|p| p != &path);
            this.recent_files.insert(0, path);
            this.recent_files.truncate(50);
          }
          if let Some(focus) = this.pre_picker_focus.take() {
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

  pub(super) fn dismiss_picker(&mut self) {
    self.active_picker = None;
    self._picker_subscription = None;
  }
}
