use crate::views::reply_view::ReplyView;
use gpui::*;
use jc_core::problem::Problem;
use jc_core::session::discover_session_group;
use jc_terminal::{Palette, TerminalConfig, TerminalView};
use std::path::Path;

pub struct SessionState {
  pub slug: String,
  pub label: String,
  pub claude_terminal: Entity<TerminalView>,
  pub general_terminal: Entity<TerminalView>,
  pub reply_view: Entity<ReplyView>,
  pub problems: Vec<Problem>,
}

impl SessionState {
  pub fn create(
    slug: String,
    label: String,
    project_path: &Path,
    palette: &Palette,
    window: &mut Window,
    cx: &mut App,
  ) -> Self {
    // Find the most recent JSONL session UUID for this slug so we can
    // resume the Claude session. Falls back to plain `claude` if none found.
    let command = discover_session_group(project_path, &slug)
      .and_then(|g| g.latest_session_id())
      .map(|uuid| format!("claude --resume {uuid}"))
      .unwrap_or_else(|| "claude".to_string());

    let claude_config = TerminalConfig {
      command: Some(command),
      palette: Some(palette.clone()),
      ..Default::default()
    };
    let general_config = TerminalConfig { palette: Some(palette.clone()), ..Default::default() };

    let project = project_path.to_path_buf();
    let claude_terminal = cx.new(|cx| TerminalView::new(claude_config, Some(&project), window, cx));
    let general_terminal =
      cx.new(|cx| TerminalView::new(general_config, Some(&project), window, cx));

    let slug_for_reply = slug.clone();
    let reply_project = project_path.to_path_buf();
    let reply_view = cx.new(|cx| {
      let mut rv = ReplyView::new(reply_project, window, cx);
      rv.set_session_slug(Some(slug_for_reply), window, cx);
      rv
    });

    Self { slug, label, claude_terminal, general_terminal, reply_view, problems: Vec::new() }
  }
}
