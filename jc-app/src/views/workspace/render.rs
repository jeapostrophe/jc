use crate::views::pane::{Pane, PaneContentKind};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::TitleBar;
use gpui_component::resizable::{h_resizable, resizable_panel};
use gpui_component::tooltip::Tooltip;

use super::Workspace;

struct PaneHeader {
  title: String,
  breadcrumbs: Vec<String>,
}

/// Elide middle breadcrumb items so the total char count stays within budget.
/// Always keeps the first and last items, replacing the middle with "\u{2026}".
fn elide_breadcrumbs(crumbs: &[String], char_budget: usize) -> Vec<String> {
  if crumbs.is_empty() {
    return Vec::new();
  }
  let sep_len = 3; // " > "
  let total: usize = crumbs.iter().map(|c| c.len()).sum::<usize>()
    + crumbs.len().saturating_sub(1) * sep_len;
  if total <= char_budget || crumbs.len() <= 2 {
    return crumbs.to_vec();
  }
  // Keep first and last, try adding from the end until we exceed budget.
  let first = &crumbs[0];
  let last = &crumbs[crumbs.len() - 1];
  let ellipsis = "\u{2026}";
  let base_len = first.len() + sep_len + ellipsis.len() + sep_len + last.len();
  if base_len >= char_budget {
    return vec![first.clone(), ellipsis.to_string(), last.clone()];
  }
  // Try to keep items from the end (nearest to cursor = most useful).
  let mut kept_end: Vec<&String> = Vec::new();
  let mut used = base_len;
  for c in crumbs[1..crumbs.len() - 1].iter().rev() {
    let cost = c.len() + sep_len;
    if used + cost > char_budget {
      break;
    }
    used += cost;
    kept_end.push(c);
  }
  kept_end.reverse();
  let elided_middle = crumbs.len() - 2 - kept_end.len();
  let mut result = vec![first.clone()];
  if elided_middle > 0 {
    result.push(ellipsis.to_string());
  }
  for c in kept_end {
    result.push(c.clone());
  }
  result.push(last.clone());
  result
}

impl Workspace {
  fn pane_header_info(&self, pane: &Entity<Pane>, cx: &App) -> PaneHeader {
    let project = self.active_project();
    match pane.read(cx).content_kind() {
      Some(PaneContentKind::CodeViewer) => {
        if let Some(cv) = project.code_view() {
          let cv = cv.read(cx);
          let dirty = if cv.is_dirty(cx) { " [+]" } else { "" };
          let title = if let Some(path) = cv.file_path() {
            let relative = path.strip_prefix(&project.path).ok().unwrap_or(path);
            format!("Code: {}{dirty}", relative.display())
          } else {
            format!("Code{dirty}")
          };
          PaneHeader { title, breadcrumbs: cv.breadcrumb().to_vec() }
        } else {
          PaneHeader { title: "Code".to_string(), breadcrumbs: Vec::new() }
        }
      }
      Some(PaneContentKind::TodoEditor) => {
        let tv = project.todo_view.read(cx);
        let dirty = if tv.is_dirty(cx) { " [+]" } else { "" };
        let title = format!("TODO{dirty}");
        let breadcrumbs = tv.code_view().read(cx).breadcrumb().to_vec();
        PaneHeader { title, breadcrumbs }
      }
      Some(PaneContentKind::GitDiff) => {
        let dv = project.diff_view.read(cx);
        let reviewed = dv.reviewed_count();
        let total = dv.file_count();
        let source_label = dv.source().label();
        let title = if let Some(name) = dv.current_file_name() {
          format!("Diff [{source_label}]: {name} ({reviewed}/{total})")
        } else {
          format!("Diff [{source_label}] ({reviewed}/{total})")
        };
        PaneHeader { title, breadcrumbs: Vec::new() }
      }
      Some(PaneContentKind::GlobalTodo) => {
        let cv = self.global_todo_view.read(cx);
        let dirty = if cv.is_dirty(cx) { " [+]" } else { "" };
        let title = format!("Global TODO{dirty}");
        PaneHeader { title, breadcrumbs: cv.breadcrumb().to_vec() }
      }
      Some(kind) => PaneHeader { title: kind.label().to_string(), breadcrumbs: Vec::new() },
      None => PaneHeader { title: "Empty".to_string(), breadcrumbs: Vec::new() },
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

    // Per-layer session labels for corner indicator.
    let layer_sessions = self.layer_problem_sessions(cx);

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
      let has_any = layer_sessions.iter().any(|s| !s.is_empty());
      if has_any {
        let layer_colors = [theme.red, theme.yellow, theme.blue, theme.muted_foreground];
        let mut segments = div()
          .id("global-problems")
          .flex()
          .items_center()
          .gap_0p5()
          .text_xs();
        let mut first = true;
        for (i, sessions) in layer_sessions.iter().enumerate() {
          if sessions.is_empty() {
            continue;
          }
          if !first {
            segments = segments.child(
              div().text_color(theme.muted_foreground).child(" / "),
            );
          }
          first = false;
          segments = segments.child(
            div().text_color(layer_colors[i]).child(sessions.len().to_string()),
          );
        }

        let sessions_for_tooltip = layer_sessions.clone();
        segments = segments.tooltip(move |window, cx| {
          let sessions = sessions_for_tooltip.clone();
          Tooltip::element(move |_window, cx| {
            let theme = cx.theme();
            let layer_colors = [theme.red, theme.yellow, theme.blue, theme.muted_foreground];
            let layer_labels = ["L0: blocked/error", "L1: review", "L2: send", "L3: idle"];
            let mut col = div().font_family("Lilex").flex().flex_col().gap_1().text_xs();
            for (i, layer) in sessions.iter().enumerate() {
              if layer.is_empty() {
                continue;
              }
              col = col.child(
                div()
                  .text_color(layer_colors[i])
                  .font_weight(FontWeight::SEMIBOLD)
                  .child(format!("{}:", layer_labels[i])),
              );
              for name in layer {
                col = col.child(
                  div()
                    .text_color(layer_colors[i])
                    .pl_2()
                    .child(name.clone()),
                );
              }
            }
            col
          })
          .build(window, cx)
        });

        el = el.child(segments);
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

    let fg = theme.foreground;
    let muted = theme.muted_foreground;

    let pane_header = |info: PaneHeader, active: bool| {
      // Budget for breadcrumb elision: leave room for the title.
      let crumb_budget = 60usize.saturating_sub(info.title.len().min(30));
      let crumbs = elide_breadcrumbs(&info.breadcrumbs, crumb_budget);

      let title_color = if active { fg } else { muted };

      let mut row = div()
        .flex()
        .items_center()
        .overflow_hidden()
        .px_2()
        .py_1()
        .text_sm()
        .border_b_1()
        .border_color(theme.border)
        .child(
          div()
            .flex_shrink_0()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(title_color)
            .child(info.title),
        );

      if !crumbs.is_empty() {
        let crumb_color = if active { muted } else { muted };
        for c in crumbs {
          row = row.child(
            div().flex_shrink_0().text_color(crumb_color).child(" > "),
          );
          row = row.child(
            div().text_color(crumb_color).truncate().child(c),
          );
        }
      }
      row
    };

    let build_pane_wrapper = |i: usize, pane: &Entity<Pane>| {
      let active = self.active_pane_index == i;
      let info = self.pane_header_info(pane, cx);
      div()
        .size_full()
        .flex()
        .flex_col()
        .border_l_2()
        .border_color(if active { active_border } else { gpui::transparent_black() })
        .child(pane_header(info, active))
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
      .on_action(cx.listener(Self::toggle_code_diff))
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
      .py(px(80.0))
      .bg(hsla(0., 0., 0., 0.3))
      .on_mouse_down(MouseButton::Left, |_, _, _cx| {})
      .child(content.clone()),
  )
  .with_priority(1)
}
