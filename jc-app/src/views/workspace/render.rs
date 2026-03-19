use crate::views::pane::{Pane, PaneContentKind};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::TitleBar;
use gpui_component::resizable::{h_resizable, resizable_panel};
use gpui_component::tooltip::Tooltip;

use super::Workspace;

impl Workspace {
  fn pane_header_label(&self, pane: &Entity<Pane>, cx: &App) -> String {
    let project = self.active_project();
    match pane.read(cx).content_kind() {
      Some(PaneContentKind::CodeViewer) => {
        let cv = project.code_view.read(cx);
        let dirty = if cv.is_dirty(cx) { " [+]" } else { "" };
        if let Some(path) = cv.file_path() {
          let relative = path.strip_prefix(&project.path).ok().unwrap_or(path);
          format!("Code: {}{dirty}", relative.display())
        } else {
          format!("Code{dirty}")
        }
      }
      Some(PaneContentKind::TodoEditor) => {
        let dirty = if project.todo_view.read(cx).is_dirty(cx) { " [+]" } else { "" };
        format!("TODO{dirty}")
      }
      Some(PaneContentKind::GitDiff) => {
        let dv = project.diff_view.read(cx);
        let reviewed = dv.reviewed_count();
        let total = dv.file_count();
        let source_label = dv.source().label();
        if let Some(name) = dv.current_file_name() {
          format!("Diff [{source_label}]: {name} ({reviewed}/{total})")
        } else {
          format!("Diff [{source_label}] ({reviewed}/{total})")
        }
      }
      Some(kind) => kind.label().to_string(),
      None => "Empty".to_string(),
    }
  }

  fn render_title_bar(&self, cx: &mut Context<Self>) -> TitleBar {
    let theme = cx.theme();
    let project = self.active_project();

    let mut title = project.name.clone();
    if let Some(session) = project.active_session() {
      title = format!("{} > {}", title, session.label);
    }

    let project_problem_count = project.problems.len();
    let current_total =
      project.active_session().map(|s| s.problems.len()).unwrap_or(0) + project_problem_count;

    // Collect active-session + project problems for the title tooltip.
    let active_session_problems: Vec<String> = project
      .active_session()
      .map(|s| s.problems.iter().map(|p| p.description()).collect())
      .unwrap_or_default();
    let active_project_problems: Vec<String> =
      project.problems.iter().map(|p| p.description()).collect();

    // Collect project names that have problems (excluding active session).
    let other_project_names: Vec<String> = self
      .projects
      .iter()
      .enumerate()
      .filter(|(pi, p)| {
        let has_project_problems = !p.problems.is_empty();
        let has_session_problems = p.sessions.iter().any(|(&id, s)| {
          let is_active = *pi == self.active_project_index && p.active_session == Some(id);
          !is_active && !s.problems.is_empty()
        });
        has_project_problems || has_session_problems
      })
      .map(|(_, p)| p.name.clone())
      .collect();
    let other_sessions_with_problems = other_project_names.len();

    let title_el = {
      let el =
        div().id("title-problems").flex().items_center().text_sm().text_color(theme.foreground);
      if current_total > 0 {
        el.child(div().mr_1().text_xs().text_color(theme.red).child("!")).child(title).child(
          div()
            .id("title-problem-count")
            .ml_1()
            .text_xs()
            .text_color(theme.red)
            .child(current_total.to_string())
            .tooltip(move |window, cx| {
              let session_problems = active_session_problems.clone();
              let project_problems = active_project_problems.clone();
              Tooltip::element(move |_window, cx| {
                let theme = cx.theme();
                let fg = theme.foreground;
                let dim = theme.muted_foreground;
                let mut col = div().font_family("Lilex").flex().flex_col().gap_1().text_xs();
                let all_descs: Vec<&String> = session_problems
                  .iter()
                  .chain(project_problems.iter())
                  .collect();
                let total = all_descs.len();
                let limit = 10;
                for desc in all_descs.iter().take(limit) {
                  col = col.child(div().text_color(fg).child((*desc).clone()));
                }
                if total > limit {
                  col = col.child(
                    div()
                      .text_color(dim)
                      .child(format!("…and {} more", total - limit)),
                  );
                }
                col
              })
              .build(window, cx)
            }),
        )
      } else {
        el.child(title)
      }
    };

    let right_el = {
      let mut el = div().flex().items_center().ml_auto().gap_2();
      if other_sessions_with_problems > 0 {
        let names = other_project_names.clone();
        el = el.child(
          div()
            .id("global-problems")
            .text_xs()
            .text_color(theme.yellow)
            .child(other_sessions_with_problems.to_string())
            .tooltip(move |window, cx| {
              let names = names.clone();
              Tooltip::element(move |_window, cx| {
                let theme = cx.theme();
                let fg = theme.foreground;
                let mut col = div().font_family("Lilex").flex().flex_col().gap_1().text_xs();
                for name in &names {
                  col = col.child(div().text_color(fg).child(name.clone()));
                }
                col
              })
              .build(window, cx)
            }),
        );
      }
      el.mr_2()
    };

    TitleBar::new()
      .font_family("Lilex")
      .child(div().flex().items_center().gap_1().mr_auto().child(title_el))
      .child(right_el)
  }
}

impl Render for Workspace {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();
    let active_border = theme.accent;
    let visible = self.visible_pane_count();

    let pane_header = |label: String, active: bool| {
      div()
        .px_2()
        .py_1()
        .text_sm()
        .text_color(if active { theme.foreground } else { theme.muted_foreground })
        .when(active, |d| d.font_weight(FontWeight::SEMIBOLD))
        .border_b_1()
        .border_color(theme.border)
        .truncate()
        .child(label)
    };

    let build_pane_wrapper = |i: usize, pane: &Entity<Pane>| {
      let active = self.active_pane_index == i;
      let label = self.pane_header_label(pane, cx);
      div()
        .size_full()
        .flex()
        .flex_col()
        .border_l_2()
        .border_color(if active { active_border } else { gpui::transparent_black() })
        .child(pane_header(label, active))
        .child(div().flex_1().min_h_0().overflow_hidden().child(pane.clone()))
    };

    // Build the pane area: single full-width pane, or h_resizable with 2-3 panels.
    let pane_area = if visible == 1 {
      div().flex_1().min_h_0().child(build_pane_wrapper(0, &self.panes[0])).into_any_element()
    } else {
      let mut resizable = h_resizable(("main-split", self.split_generation));
      for i in 0..visible {
        resizable = resizable
          .child(resizable_panel().size(px(600.0)).child(build_pane_wrapper(i, &self.panes[i])));
      }
      resizable.into_any_element()
    };

    div()
      .id("workspace")
      .key_context("Workspace")
      .track_focus(&self.focus)
      .size_full()
      .bg(theme.background)
      .on_action(cx.listener(Self::close_window))
      .on_action(cx.listener(Self::minimize_window))
      .on_action(cx.listener(Self::quit))
      .on_action(cx.listener(Self::focus_prev_pane))
      .on_action(cx.listener(Self::focus_next_pane))
      .on_action(cx.listener(Self::set_layout_one))
      .on_action(cx.listener(Self::set_layout_two))
      .on_action(cx.listener(Self::set_layout_three))
      .on_action(cx.listener(Self::show_claude_terminal))
      .on_action(cx.listener(Self::show_general_terminal))
      .on_action(cx.listener(Self::show_git_diff))
      .on_action(cx.listener(Self::show_code_viewer))
      .on_action(cx.listener(Self::show_todo_editor))
      .on_action(cx.listener(Self::open_in_external_editor))
      .on_action(cx.listener(Self::open_picker))
      .on_action(cx.listener(Self::open_drill_down_picker))
      .on_action(cx.listener(Self::open_project_actions_picker))
      .on_action(cx.listener(Self::open_session_picker))
      .on_action(cx.listener(Self::search_lines))
      .on_action(cx.listener(Self::open_comment_panel))
      .on_action(cx.listener(Self::save_file))
      .on_action(cx.listener(Self::send_to_terminal))
      .on_action(cx.listener(Self::copy_reply))
      .on_action(cx.listener(Self::jump_to_wait))
      .on_action(cx.listener(Self::next_problem))
      .on_action(cx.listener(Self::rotate_next_project))
      .on_action(cx.listener(Self::toggle_keybinding_help))
      .on_action(cx.listener(Self::show_snippet_picker))
      .on_action(cx.listener(Self::scroll_other_up))
      .on_action(cx.listener(Self::scroll_other_down))
      .on_action(cx.listener(Self::scroll_other_page_up))
      .on_action(cx.listener(Self::scroll_other_page_down))
      .child(self.render_title_bar(cx))
      .child(pane_area)
      .when_some(self.active_picker.as_ref(), |el, v| el.child(modal_overlay(v)))
      .when_some(self.active_comment_panel.as_ref(), |el, v| el.child(modal_overlay(v)))
      .when_some(self.keybinding_help.as_ref(), |el, (v, _)| el.child(modal_overlay(v)))
      .when_some(self.close_confirm.as_ref(), |el, (v, _)| el.child(modal_overlay(v)))
  }
}

fn modal_overlay(content: &AnyView) -> Deferred {
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
      .child(content.clone()),
  )
  .with_priority(1)
}
