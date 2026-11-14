//! The app follows its own `state.toml` (Cira PLAN TMUX-10; Jay: "live-react
//! to the state.toml file changing"): a project the file lists that is not
//! open opens, without switching to it; one open that it no longer lists
//! closes. jcc closes it at once (its sessions keep running in Cira); jc,
//! whose sessions are its own `claude` processes, asks first when the project
//! has any. The app's own writes list exactly what it has open, so they ask
//! for nothing ([`jc_core::config::AppState::change_from`]).

use super::Workspace;
use crate::views::close_confirm::Closing;
use gpui::*;
use jc_core::config::AppState;
use jc_core::flavor::Flavor;
use std::path::{Path, PathBuf};

impl Workspace {
  /// Watch `state.toml` in the app's config directory.
  pub(super) fn watch_state_file(
    &self,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Option<notify::RecommendedWatcher> {
    let is_state = |p: &Path| p.file_name().is_some_and(|n| n == "state.toml");
    crate::file_watcher::watch_dir(
      &jc_core::config::config_dir(),
      is_state,
      |ws: &mut Self, window, cx| ws.read_state_file(window, cx),
      window,
      cx,
    )
  }

  /// Read `state.toml` off the main thread, then follow it.
  fn read_state_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.run_blocking(
      window,
      cx,
      jc_core::config::load_state,
      |ws, state, window, cx| match state {
        Ok(state) => ws.follow_state(&state, window, cx),
        Err(e) => eprintln!("state.toml: {e:#}; projects left as they are"),
      },
    );
  }

  /// Open and close projects to match `state`. A file listing no project
  /// (one being rewritten by hand, say) changes nothing.
  fn follow_state(&mut self, state: &AppState, window: &mut Window, cx: &mut Context<Self>) {
    if state.projects.is_empty() {
      return;
    }
    let open: Vec<PathBuf> = self.projects.iter().map(|p| p.path.clone()).collect();
    let change = state.change_from(&open);
    for path in change.added {
      eprintln!("state.toml: opening {}", path.display());
      self.add_project(path, window, cx);
    }
    for path in change.removed {
      self.project_left_state(path, window, cx);
    }
    cx.notify();
  }

  /// `state.toml` no longer lists the open project at `path`.
  fn project_left_state(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
    // One question at a time: the next change of the file asks again.
    if self.close_confirm.is_some() {
      return;
    }
    let Some(pi) = self.projects.iter().position(|p| p.path == path) else { return };
    let name = self.projects[pi].name.clone();
    if self.projects.len() == 1 {
      self
        .notice(format!("state.toml no longer lists {name}, the only project open; it stays"), cx);
      return;
    }
    let sessions = self.projects[pi].sessions.len();
    if self.flavor == Flavor::Jc && sessions > 0 {
      self.show_close_confirm(sessions, Vec::new(), Closing::Project(path), window, cx);
      return;
    }
    self.close_project(&path, window, cx);
    let kept = if self.flavor == Flavor::Jcc { " (its sessions keep running in Cira)" } else { "" };
    self.notice(format!("Closed {name}: state.toml no longer lists it{kept}"), cx);
  }

  /// Close the project at `path`: its panes and sessions go (jcc's detach;
  /// jc's `claude` processes end), and its hooks are removed.
  pub(super) fn close_project(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
    let Some(pi) = self.projects.iter().position(|p| p.path == path) else { return };
    if self.projects.len() == 1 {
      return;
    }
    if pi == self.active_project_index {
      let other = if pi == 0 { 1 } else { 0 };
      let active = self.projects[other].active_session;
      self.switch_to_session(other, active, window, cx);
    }
    let project = self.projects.remove(pi);
    if self.active_project_index > pi {
      self.active_project_index -= 1;
    }
    eprintln!("state.toml: closed {}", project.path.display());
    let (flavor, path) = (self.flavor, project.path.clone());
    std::thread::spawn(move || {
      if let Err(e) = jc_core::hooks_settings::uninstall_hooks(flavor, &path) {
        eprintln!("failed to remove hooks for {}: {e:#}", path.display());
      }
    });
    drop(project);
    self.refresh_breadcrumb_observers(cx);
    cx.notify();
  }

  /// Jay kept a project `state.toml` dropped: list it there again.
  pub(super) fn keep_project(
    &mut self,
    path: PathBuf,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let work = move || -> anyhow::Result<()> {
      let mut state = jc_core::config::load_state()?;
      state.register_project(&path);
      jc_core::config::save_state(&state)
    };
    self.run_blocking(window, cx, work, |_, kept, _, _| {
      if let Err(e) = kept {
        eprintln!("state.toml: could not list the kept project again: {e:#}");
      }
    });
  }
}
