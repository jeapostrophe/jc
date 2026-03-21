use gpui::{AsyncWindowContext, Context, WeakEntity, Window};
use notify::{EventKind, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set up a file-system watcher on `dir` (non-recursive).
///
/// *  `path_matches` — called inside the notify callback for each event path;
///    return `true` to accept the event.
/// *  `suppress` — when provided, events are silently dropped while the flag
///    is `true` (used to ignore self-writes).
/// *  `on_change` — called on the main thread after one or more matching events
///    have been coalesced.
///
/// Returns the `RecommendedWatcher` so the caller can keep it alive.
pub fn watch_dir<E, P, F>(
  dir: &Path,
  path_matches: P,
  suppress: Option<Arc<AtomicBool>>,
  on_change: F,
  window: &Window,
  cx: &mut Context<E>,
) -> Option<notify::RecommendedWatcher>
where
  E: 'static,
  P: Fn(&Path) -> bool + Send + 'static,
  F: Fn(&mut E, &mut gpui::Window, &mut Context<E>) + Send + 'static,
{
  let (tx, rx) = flume::unbounded::<()>();

  let mut watcher =
    notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
      if let Ok(event) = res {
        match event.kind {
          EventKind::Modify(_) | EventKind::Create(_) => {
            let dominated = suppress.as_ref().is_some_and(|s| s.load(Ordering::Relaxed));
            if !dominated && event.paths.iter().any(|p| path_matches(p)) {
              let _ = tx.send(());
            }
          }
          _ => {}
        }
      }
    })
    .ok()?;

  let _ = watcher.watch(dir, RecursiveMode::NonRecursive);

  cx.spawn_in(window, async move |this: WeakEntity<E>, cx: &mut AsyncWindowContext| {
    while rx.recv_async().await.is_ok() {
      for _ in 0..100 {
        if rx.try_recv().is_err() {
          break;
        }
      }
      let _ = this.update_in(cx, &on_change);
    }
  })
  .detach();

  Some(watcher)
}
