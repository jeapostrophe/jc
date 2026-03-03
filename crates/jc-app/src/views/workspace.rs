use crate::views::code_view::CodeView;
use crate::views::diff_view::DiffView;
use crate::views::pane::{Pane, PaneContent, PaneContentKind};
use crate::views::picker::{
  CodeSymbolPickerDelegate, DiffFilePickerDelegate, FilePickerDelegate, OpenContextPicker,
  OpenFilePicker, PickerEvent, PickerState, TodoHeaderPickerDelegate,
};
use crate::views::todo_view::TodoView;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::TitleBar;
use gpui_component::resizable::{h_resizable, resizable_panel};
use gpui_component::theme::Theme;
use jc_core::config::{AppConfig, AppState};
use jc_core::theme::{Appearance, ThemeConfig};
use jc_terminal::{Palette, TerminalConfig, TerminalView};
use std::ops::DerefMut;
use std::path::PathBuf;

actions!(
  workspace,
  [
    CloseWindow,
    MinimizeWindow,
    Quit,
    FocusLeftPane,
    FocusRightPane,
    ShowClaudeTerminal,
    ShowGeneralTerminal,
    ShowGitDiff,
    ShowCodeViewer,
    ShowTodoEditor,
    OpenInExternalEditor,
    ToggleTheme,
  ]
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePane {
  Left,
  Right,
}

/// Map a GPUI WindowAppearance to our Appearance enum.
fn appearance_from_window(appearance: WindowAppearance) -> Appearance {
  match appearance {
    WindowAppearance::Dark | WindowAppearance::VibrantDark => Appearance::Dark,
    WindowAppearance::Light | WindowAppearance::VibrantLight => Appearance::Light,
  }
}

pub struct Workspace {
  left_pane: Entity<Pane>,
  right_pane: Entity<Pane>,
  active_pane: ActivePane,
  claude_terminal: Entity<TerminalView>,
  general_terminal: Entity<TerminalView>,
  diff_view: Entity<DiffView>,
  code_view: Entity<CodeView>,
  todo_view: Entity<TodoView>,
  state: AppState,
  config: AppConfig,
  focus: FocusHandle,
  active_picker: Option<AnyView>,
  pre_picker_focus: Option<FocusHandle>,
  _picker_subscription: Option<Subscription>,
  _appearance_subscription: Subscription,
}

impl Workspace {
  pub fn new(
    state: AppState,
    config: AppConfig,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let project_path = state
      .projects
      .first()
      .map(|p| p.path.clone())
      .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // Detect the current system appearance and pick the right terminal palette.
    let appearance = appearance_from_window(window.appearance());
    let theme_config = ThemeConfig::for_appearance(appearance);
    let palette = Palette::from(&theme_config.terminal);
    let terminal_config =
      |palette: &Palette| TerminalConfig { palette: Some(palette.clone()), ..Default::default() };

    let claude_terminal =
      cx.new(|cx| TerminalView::new(terminal_config(&palette), None, window, cx));
    let general_terminal =
      cx.new(|cx| TerminalView::new(terminal_config(&palette), None, window, cx));
    let diff_view = cx.new(|cx| DiffView::new(project_path.clone(), window, cx));
    let code_view = cx.new(|cx| CodeView::new(project_path.clone(), window, cx));
    let todo_view = cx.new(|cx| TodoView::new(project_path, window, cx));

    let claude_focus = claude_terminal.read(cx).focus_handle(cx);
    let general_focus = general_terminal.read(cx).focus_handle(cx);

    let left_pane = cx.new(|cx| {
      Pane::with_content(
        PaneContent {
          kind: PaneContentKind::ClaudeTerminal,
          view: claude_terminal.clone().into(),
          focus: claude_focus,
        },
        cx,
      )
    });

    let right_pane = cx.new(|cx| {
      Pane::with_content(
        PaneContent {
          kind: PaneContentKind::GeneralTerminal,
          view: general_terminal.clone().into(),
          focus: general_focus,
        },
        cx,
      )
    });

    left_pane.read(cx).focus_content(window);

    // Observe system appearance changes and update themes accordingly.
    let appearance_subscription =
      cx.observe_window_appearance(window, |this: &mut Self, window, cx| {
        this.apply_appearance(appearance_from_window(window.appearance()), window, cx);
      });

    Self {
      left_pane,
      right_pane,
      active_pane: ActivePane::Left,
      claude_terminal,
      general_terminal,
      diff_view,
      code_view,
      todo_view,
      state,
      config,
      focus: cx.focus_handle(),
      active_picker: None,
      pre_picker_focus: None,
      _picker_subscription: None,
      _appearance_subscription: appearance_subscription,
    }
  }

  /// Apply a new appearance: update the gpui_component theme and terminal palettes.
  fn apply_appearance(
    &mut self,
    appearance: Appearance,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    // Update gpui_component theme (dark/light).
    Theme::sync_system_appearance(Some(window), cx.deref_mut());

    // Update terminal palettes.
    let theme_config = ThemeConfig::for_appearance(appearance);
    let palette = Palette::from(&theme_config.terminal);

    self.claude_terminal.update(cx, |view, _cx| {
      view.set_palette(palette.clone());
    });
    self.general_terminal.update(cx, |view, _cx| {
      view.set_palette(palette);
    });

    cx.notify();
  }

  fn toggle_theme(&mut self, _: &ToggleTheme, window: &mut Window, cx: &mut Context<Self>) {
    // Read current gpui_component theme mode and flip it.
    let current_dark = cx.theme().is_dark();
    let new_appearance = if current_dark { Appearance::Light } else { Appearance::Dark };

    // Use Theme::change to set the gpui_component theme explicitly.
    let mode: gpui_component::theme::ThemeMode = if new_appearance.is_dark() {
      gpui_component::theme::ThemeMode::Dark
    } else {
      gpui_component::theme::ThemeMode::Light
    };
    Theme::change(mode, Some(window), cx.deref_mut());

    // Update terminal palettes.
    let theme_config = ThemeConfig::for_appearance(new_appearance);
    let palette = Palette::from(&theme_config.terminal);

    self.claude_terminal.update(cx, |view, _cx| {
      view.set_palette(palette.clone());
    });
    self.general_terminal.update(cx, |view, _cx| {
      view.set_palette(palette);
    });

    cx.notify();
  }

  fn project_path(&self) -> PathBuf {
    self
      .state
      .projects
      .first()
      .map(|p| p.path.clone())
      .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
  }

  fn close_window(&mut self, _: &CloseWindow, window: &mut Window, _cx: &mut Context<Self>) {
    window.remove_window();
  }

  fn minimize_window(&mut self, _: &MinimizeWindow, window: &mut Window, _cx: &mut Context<Self>) {
    window.minimize_window();
  }

  fn quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
    cx.quit();
  }

  fn focus_left_pane(&mut self, _: &FocusLeftPane, window: &mut Window, cx: &mut Context<Self>) {
    self.active_pane = ActivePane::Left;
    self.left_pane.read(cx).focus_content(window);
    cx.notify();
  }

  fn focus_right_pane(&mut self, _: &FocusRightPane, window: &mut Window, cx: &mut Context<Self>) {
    self.active_pane = ActivePane::Right;
    self.right_pane.read(cx).focus_content(window);
    cx.notify();
  }

  fn active_pane_entity(&self) -> &Entity<Pane> {
    match self.active_pane {
      ActivePane::Left => &self.left_pane,
      ActivePane::Right => &self.right_pane,
    }
  }

  fn set_active_pane_view(
    &mut self,
    kind: PaneContentKind,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let (view, focus): (AnyView, FocusHandle) = match kind {
      PaneContentKind::ClaudeTerminal => {
        let focus = self.claude_terminal.read(cx).focus_handle(cx);
        (self.claude_terminal.clone().into(), focus)
      }
      PaneContentKind::GeneralTerminal => {
        let focus = self.general_terminal.read(cx).focus_handle(cx);
        (self.general_terminal.clone().into(), focus)
      }
      PaneContentKind::GitDiff => {
        self.diff_view.update(cx, |v, cx| v.refresh(window, cx));
        let focus = self.diff_view.read(cx).focus_handle(cx);
        (self.diff_view.clone().into(), focus)
      }
      PaneContentKind::CodeViewer => {
        let focus = self.code_view.read(cx).focus_handle(cx);
        (self.code_view.clone().into(), focus)
      }
      PaneContentKind::TodoEditor => {
        let focus = self.todo_view.read(cx).focus_handle(cx);
        (self.todo_view.clone().into(), focus)
      }
    };

    let pane = self.active_pane_entity().clone();
    pane.update(cx, |p, cx| {
      p.set_content(PaneContent { kind, view, focus: focus.clone() }, cx);
    });
    focus.focus(window);
  }

  fn show_claude_terminal(
    &mut self,
    _: &ShowClaudeTerminal,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.set_active_pane_view(PaneContentKind::ClaudeTerminal, window, cx);
  }

  fn show_general_terminal(
    &mut self,
    _: &ShowGeneralTerminal,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    self.set_active_pane_view(PaneContentKind::GeneralTerminal, window, cx);
  }

  fn show_git_diff(&mut self, _: &ShowGitDiff, window: &mut Window, cx: &mut Context<Self>) {
    self.set_active_pane_view(PaneContentKind::GitDiff, window, cx);
  }

  fn show_code_viewer(&mut self, _: &ShowCodeViewer, window: &mut Window, cx: &mut Context<Self>) {
    self.set_active_pane_view(PaneContentKind::CodeViewer, window, cx);
  }

  fn show_todo_editor(&mut self, _: &ShowTodoEditor, window: &mut Window, cx: &mut Context<Self>) {
    self.set_active_pane_view(PaneContentKind::TodoEditor, window, cx);
  }

  fn open_in_external_editor(
    &mut self,
    _: &OpenInExternalEditor,
    _window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let pane = self.active_pane_entity().clone();
    let kind = pane.read(cx).content_kind();
    let file_path = match kind {
      Some(PaneContentKind::CodeViewer) => {
        self.code_view.read(cx).file_path().map(|p| p.to_path_buf())
      }
      Some(PaneContentKind::TodoEditor) => Some(self.todo_view.read(cx).file_path().to_path_buf()),
      _ => None,
    };
    if let Some(path) = file_path {
      let editor =
        if self.config.editor.is_empty() { "open".to_string() } else { self.config.editor.clone() };
      let _ = std::process::Command::new(&editor).arg(path).spawn();
    }
  }

  fn open_file_picker(&mut self, _: &OpenFilePicker, window: &mut Window, cx: &mut Context<Self>) {
    if self.active_picker.is_some() {
      return;
    }

    let delegate = FilePickerDelegate::new(self.project_path(), self.code_view.clone());
    let picker = cx.new(|cx| PickerState::new(delegate, window, cx));

    self.pre_picker_focus = window.focused(cx);

    let subscription =
      cx.subscribe_in(&picker, window, |this: &mut Self, _, event, window, cx| match event {
        PickerEvent::Confirmed => {
          this.set_active_pane_view(PaneContentKind::CodeViewer, window, cx);
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

  fn open_context_picker(
    &mut self,
    _: &OpenContextPicker,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if self.active_picker.is_some() {
      return;
    }

    let pane = self.active_pane_entity().clone();
    let kind = pane.read(cx).content_kind();

    match kind {
      Some(PaneContentKind::GitDiff) => {
        let delegate = DiffFilePickerDelegate::new(self.diff_view.clone(), cx);
        self.show_picker(delegate, window, cx);
      }
      Some(PaneContentKind::TodoEditor) => {
        let delegate = TodoHeaderPickerDelegate::new(self.todo_view.clone(), cx);
        self.show_picker(delegate, window, cx);
      }
      Some(PaneContentKind::CodeViewer) => {
        let delegate = CodeSymbolPickerDelegate::new(self.code_view.clone(), cx);
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
      cx.subscribe_in(&picker, window, |this: &mut Self, _, event, window, cx| match event {
        PickerEvent::Confirmed | PickerEvent::Dismissed => {
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

  fn dismiss_picker(&mut self) {
    self.active_picker = None;
    self._picker_subscription = None;
  }

  fn pane_header_label(&self, pane: &Entity<Pane>, cx: &App) -> String {
    match pane.read(cx).content_kind() {
      Some(PaneContentKind::CodeViewer) => {
        if let Some(path) = self.code_view.read(cx).file_path() {
          let project_root = self.state.projects.first().map(|p| &p.path);
          let relative = project_root.and_then(|root| path.strip_prefix(root).ok()).unwrap_or(path);
          format!("Code: {}", relative.display())
        } else {
          "Code".to_string()
        }
      }
      Some(kind) => kind.label().to_string(),
      None => "Empty".to_string(),
    }
  }

  fn render_title_bar(&self, cx: &mut Context<Self>) -> TitleBar {
    let theme = cx.theme();

    let project_name =
      self.state.projects.first().map(|p| p.name.clone()).unwrap_or_else(|| "No project".into());

    TitleBar::new()
      .child(
        div()
          .flex()
          .items_center()
          .gap_1()
          .mr_auto()
          .child(div().text_sm().text_color(theme.foreground).child(project_name)),
      )
      .child(
        div()
          .flex()
          .items_center()
          .ml_auto()
          .child(div().text_sm().text_color(theme.muted_foreground).child("Usage")),
      )
  }
}

impl Focusable for Workspace {
  fn focus_handle(&self, _: &App) -> FocusHandle {
    self.focus.clone()
  }
}

impl Render for Workspace {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();
    let active_border = theme.accent;

    let left_active = self.active_pane == ActivePane::Left;
    let right_active = self.active_pane == ActivePane::Right;

    let left_label = self.pane_header_label(&self.left_pane, cx);
    let right_label = self.pane_header_label(&self.right_pane, cx);

    let pane_header = |label: String, active: bool| {
      div()
        .px_2()
        .py_1()
        .text_sm()
        .text_color(if active { theme.foreground } else { theme.muted_foreground })
        .when(active, |d| d.font_weight(FontWeight::SEMIBOLD))
        .border_b_1()
        .border_color(theme.border)
        .child(label)
    };

    let left_wrapper = div()
      .size_full()
      .flex()
      .flex_col()
      .border_l_2()
      .border_color(if left_active { active_border } else { gpui::transparent_black() })
      .child(pane_header(left_label, left_active))
      .child(div().flex_1().min_h_0().overflow_hidden().child(self.left_pane.clone()));

    let right_wrapper = div()
      .size_full()
      .flex()
      .flex_col()
      .border_l_2()
      .border_color(if right_active { active_border } else { gpui::transparent_black() })
      .child(pane_header(right_label, right_active))
      .child(div().flex_1().min_h_0().overflow_hidden().child(self.right_pane.clone()));

    div()
      .id("workspace")
      .key_context("Workspace")
      .track_focus(&self.focus)
      .size_full()
      .bg(theme.background)
      .on_action(cx.listener(Self::close_window))
      .on_action(cx.listener(Self::minimize_window))
      .on_action(cx.listener(Self::quit))
      .on_action(cx.listener(Self::focus_left_pane))
      .on_action(cx.listener(Self::focus_right_pane))
      .on_action(cx.listener(Self::show_claude_terminal))
      .on_action(cx.listener(Self::show_general_terminal))
      .on_action(cx.listener(Self::show_git_diff))
      .on_action(cx.listener(Self::show_code_viewer))
      .on_action(cx.listener(Self::show_todo_editor))
      .on_action(cx.listener(Self::open_in_external_editor))
      .on_action(cx.listener(Self::open_file_picker))
      .on_action(cx.listener(Self::open_context_picker))
      .on_action(cx.listener(Self::toggle_theme))
      .child(self.render_title_bar(cx))
      .child(
        h_resizable("main-split")
          .child(resizable_panel().size(px(600.0)).child(left_wrapper))
          .child(resizable_panel().size(px(600.0)).child(right_wrapper)),
      )
      .when_some(self.active_picker.as_ref(), |el, picker| {
        el.child(
          deferred(
            div()
              .absolute()
              .size_full()
              .top_0()
              .left_0()
              .flex()
              .justify_center()
              .pt(px(80.0))
              .bg(hsla(0., 0., 0., 0.3))
              .on_mouse_down(MouseButton::Left, |_, _, _cx| {})
              .child(picker.clone()),
          )
          .with_priority(1),
        )
      })
  }
}

pub fn init(cx: &mut App) {
  cx.bind_keys([
    KeyBinding::new("cmd-w", CloseWindow, Some("Workspace")),
    KeyBinding::new("cmd-m", MinimizeWindow, Some("Workspace")),
    KeyBinding::new("cmd-q", Quit, Some("Workspace")),
    KeyBinding::new("cmd-[", FocusLeftPane, Some("Workspace")),
    KeyBinding::new("cmd-]", FocusRightPane, Some("Workspace")),
    KeyBinding::new("cmd-1", ShowClaudeTerminal, Some("Workspace")),
    KeyBinding::new("cmd-2", ShowGeneralTerminal, Some("Workspace")),
    KeyBinding::new("cmd-3", ShowGitDiff, Some("Workspace")),
    KeyBinding::new("cmd-4", ShowCodeViewer, Some("Workspace")),
    KeyBinding::new("cmd-5", ShowTodoEditor, Some("Workspace")),
    KeyBinding::new("cmd-shift-e", OpenInExternalEditor, Some("Workspace")),
    KeyBinding::new("cmd-shift-t", ToggleTheme, Some("Workspace")),
  ]);

  cx.bind_keys([
    KeyBinding::new("cmd-[", FocusLeftPane, Some("Input")),
    KeyBinding::new("cmd-]", FocusRightPane, Some("Input")),
  ]);
}
