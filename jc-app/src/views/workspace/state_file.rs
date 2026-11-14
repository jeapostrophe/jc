//! The app follows its own `state.toml` (Cira PLAN TMUX-10; Jay: "live-react
//! to the state.toml file changing"): a project the file lists that is not
//! open opens, without switching to it; one open that it no longer lists
//! closes once its buffers are saved ([`jc_core::config::leaving`] says when
//! Jay is asked first). An app's own write lists what it has open, so it
//! changes nothing ([`jc_core::config::Resolved::change_from`]).

use super::Workspace;
use crate::views::close_confirm::Closing;
use gpui::*;
use jc_core::config::{Leaving, Resolved};
use jc_core::flavor::Flavor;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long `state.toml` must hold still before it is read: an editor's save
/// can be several writes.
const STATE_SETTLE: Duration = Duration::from_millis(150);

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
      |ws: &mut Self, window, cx| ws.state_file_changed(window, cx),
      window,
      cx,
    )
  }

  /// Read the file once it has held still for [`STATE_SETTLE`].
  fn state_file_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.state_changes += 1;
    let change = self.state_changes;
    cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| {
      Timer::after(STATE_SETTLE).await;
      let _ = this.update_in(cx, |ws, window, cx| {
        if ws.state_changes == change {
          ws.read_state_file(window, cx);
        }
      });
    })
    .detach();
  }

  /// Read and resolve `state.toml` off the main thread, then follow it,
  /// unless a newer read has been asked for meanwhile.
  pub(super) fn read_state_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let read = self.state_reads.issue();
    let work = || jc_core::config::load_state().map(|state| state.resolve());
    self.run_blocking(window, cx, work, move |ws, resolved, window, cx| {
      if !ws.state_reads.is_latest(read) {
        return;
      }
      match resolved {
        Ok(resolved) => ws.follow_state(&resolved, window, cx),
        Err(e) => eprintln!("state.toml: {e:#}; projects left as they are"),
      }
    });
  }

  /// Open and close projects to match `resolved`. A file listing no project
  /// (one being rewritten by hand, say) changes nothing.
  fn follow_state(&mut self, resolved: &Resolved, window: &mut Window, cx: &mut Context<Self>) {
    if resolved.listed.is_empty() && resolved.missing.is_empty() {
      return;
    }
    // A project the dialog asks about that the file lists again stays.
    if let Closing::Project(path) = &self.closing
      && self.close_confirm.is_some()
      && resolved.listed.contains(path)
    {
      self.close_confirm = None;
      self.restore_close_confirm_focus(window);
    }
    let open: Vec<PathBuf> = self.projects.iter().map(|p| p.path.clone()).collect();
    for missing in resolved.missing.iter().filter(|p| !open.contains(p)) {
      self.notice(format!("state.toml lists {}, which does not exist", missing.display()), cx);
    }
    let change = resolved.change_from(&open);
    for path in change.added {
      eprintln!("state.toml: opening {}", path.display());
      self.add_project(path, window, cx);
    }
    for path in change.removed {
      self.project_left_state(path, window, cx);
    }
    cx.notify();
  }

  /// `state.toml` no longer lists the open project at `path`: save its
  /// buffers, then close it, ask first, or keep it ([`jc_core::config::leaving`]).
  fn project_left_state(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
    let Some(pi) = self.projects.iter().position(|p| p.path == path) else { return };
    let name = self.projects[pi].name.clone();
    let unsaved = self.save_dirty_of(pi, window, cx);
    let active = match self.flavor {
      Flavor::Jc => self.projects[pi].sessions.values().filter(|s| s.busy).count(),
      Flavor::Jcc => 0,
    };
    let (open, sessions) = (self.projects.len(), self.projects[pi].sessions.len());
    let dialog = self.close_confirm.is_some();
    match jc_core::config::leaving(self.flavor, open, sessions, !unsaved.is_empty(), dialog) {
      Leaving::Stay => {
        let why = format!("state.toml no longer lists {name}, the only project open; it stays");
        self.notice(why, cx);
      }
      Leaving::Wait => {}
      // The dialog counts the sessions that are working.
      Leaving::Ask => self.show_close_confirm(active, unsaved, Closing::Project(path), window, cx),
      Leaving::Close => {
        self.close_project(&path, window, cx);
        let kept =
          if self.flavor == Flavor::Jcc { " (its sessions keep running in Cira)" } else { "" };
        self.notice(format!("Closed {name}: state.toml no longer lists it{kept}"), cx);
      }
    }
  }

  /// Close the project at `path`: its panes and sessions go (jcc's detach;
  /// jc's `claude` processes end), and its hooks are removed. The caller has
  /// saved its buffers.
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
    self.remove_project_hooks(project.path.clone());
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
