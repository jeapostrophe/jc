use gpui::*;
use gpui_component::ActiveTheme;
use jc_core::config::AppState;

pub struct ProjectView {
  state: AppState,
}

impl ProjectView {
  pub fn with_state(state: AppState) -> Self {
    Self { state }
  }
}

impl Render for ProjectView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();

    let content = if self.state.projects.is_empty() {
      div()
        .flex()
        .flex_col()
        .gap_2()
        .items_center()
        .child(div().text_xl().text_color(theme.foreground).child("jc"))
        .child(
          div()
            .text_color(theme.muted_foreground)
            .child("No projects yet. Run jc <path> to add one."),
        )
    } else {
      let mut col = div().flex().flex_col().gap_3().w_full();

      col = col.child(div().text_xl().text_color(theme.foreground).child("Projects"));

      for project in &self.state.projects {
        let mut project_div = div()
          .flex()
          .flex_col()
          .gap_1()
          .p_3()
          .rounded_md()
          .bg(theme.secondary)
          .child(
            div()
              .text_base()
              .font_weight(FontWeight::SEMIBOLD)
              .text_color(theme.foreground)
              .child(project.name.clone()),
          )
          .child(
            div()
              .text_sm()
              .text_color(theme.muted_foreground)
              .child(project.path.display().to_string()),
          );

        if !project.tasks.is_empty() {
          let mut task_list = div().flex().flex_col().gap_1().mt_2();
          for task in &project.tasks {
            task_list = task_list.child(
              div()
                .text_sm()
                .text_color(theme.foreground)
                .child(format!("  {} ({:?})", task.name, task.status)),
            );
          }
          project_div = project_div.child(task_list);
        }

        col = col.child(project_div);
      }

      col
    };

    div()
      .flex()
      .size_full()
      .justify_center()
      .items_center()
      .bg(theme.background)
      .child(div().flex().flex_col().items_center().max_w(px(600.0)).p_8().child(content))
  }
}
