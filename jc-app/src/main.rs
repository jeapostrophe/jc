mod app;
mod file_watcher;
mod ipc;
mod language;
mod notify;
mod outline;
mod views;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use jc_core::config;
use jc_core::flavor::Flavor;
use std::io::Read as _;
use std::os::fd::IntoRawFd as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};

#[derive(Parser)]
#[command(name = "jc", about = "Claude Code session orchestrator")]
struct Cli {
  #[command(subcommand)]
  command: Option<Command>,

  /// Path to a project directory to open or register.
  path: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
  /// Remove this app's stale hooks from all its configured projects (jc's or
  /// jcc's; the other's are left alone).
  CleanHooks,
}

fn main() -> anyhow::Result<()> {
  // `jcc` (the jcc.app bundle's executable, or JC_FLAVOR=jcc) is Cira's
  // client; anything else is jc (Cira PLAN TMUX-07). Chosen before anything
  // reads config or a TODO.
  let flavor = match Flavor::chosen(
    std::env::args_os().next().as_deref(),
    std::env::var_os("JC_FLAVOR").as_deref(),
  ) {
    Ok(flavor) => flavor,
    Err((flavor, why)) => {
      eprintln!("{why}; running as {}", flavor.name());
      flavor
    }
  }
  .init();
  let matches = Cli::command().name(flavor.name()).get_matches();
  let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());

  if let Some(Command::CleanHooks) = cli.command {
    return cmd_clean_hooks();
  }

  // Resolve the project path early.
  let project_path =
    cli.path.as_ref().map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()));

  // Try to send to an already-running instance.
  if let Some(path) = &project_path
    && ipc::try_send_to_running(path)
  {
    return Ok(());
  }

  let config = config::load_config()?;
  let mut state = config::load_state()?;

  if let Some(path) = &project_path {
    state.register_project(path);
    config::save_state(&state)?;
  } else if state.projects.is_empty() {
    let cwd = std::env::current_dir()?;
    state.register_project(&cwd);
    config::save_state(&state)?;
  }

  // Start IPC server so subsequent `jc .` invocations can reach us.
  // The channel bridges the IPC thread to the GPUI main thread.
  let (ipc_tx, ipc_rx) = flume::unbounded::<PathBuf>();
  let _server = ipc::SocketServer::bind(move |path| {
    let _ = ipc_tx.send(path);
  });

  // Install a SIGINT handler that cleans hooks before exiting.
  // The GPUI event loop swallows Ctrl-C, so Drop doesn't always run.
  install_signal_handler(&state);

  app::run(state, config, ipc_rx);
  Ok(())
}

fn cmd_clean_hooks() -> anyhow::Result<()> {
  let state = config::load_state()?;

  let mut paths: Vec<PathBuf> = state.projects.iter().map(|p| p.path.clone()).collect();

  // Also include cwd if it's not already in the list.
  if let Ok(cwd) = std::env::current_dir() {
    let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    if !paths.contains(&cwd) {
      paths.push(cwd);
    }
  }

  for path in &paths {
    match jc_core::hooks_settings::uninstall_hooks(Flavor::current(), path) {
      Ok(()) => eprintln!("cleaned hooks for {}", path.display()),
      Err(e) => eprintln!("failed to clean hooks for {}: {e}", path.display()),
    }
  }
  Ok(())
}

/// The write end of the pipe [`on_signal`] wakes the cleanup thread through;
/// -1 until [`spawn_cleanup`] makes it.
static SIGNAL_PIPE: AtomicI32 = AtomicI32::new(-1);

/// On SIGINT or SIGTERM, remove this app's hooks from every project and its
/// IPC socket, then die of the signal. The GPUI event loop swallows Ctrl-C, so
/// Drop doesn't always run.
fn install_signal_handler(state: &config::AppState) {
  let paths: Vec<PathBuf> = state.projects.iter().map(|p| p.path.clone()).collect();
  let cleanup = move || {
    ipc::cleanup_socket();
    for path in &paths {
      let _ = jc_core::hooks_settings::uninstall_hooks(Flavor::current(), path);
    }
  };
  // Re-raise with the default handler for normal exit behavior.
  let exit = |sig| unsafe {
    libc::signal(sig, libc::SIG_DFL);
    libc::raise(sig);
  };
  if let Err(e) = spawn_cleanup(cleanup, exit) {
    eprintln!("no cleanup on SIGINT/SIGTERM: {e}");
    return;
  }
  unsafe {
    libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
    libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
  }
}

/// Start the thread that runs `cleanup` and then `then(signal)` once
/// [`on_signal`] reports a signal. The cleanup takes the `.claude` lock, which
/// the interrupted thread may hold (jc's start installs hooks under it), so it
/// never runs inside the handler.
fn spawn_cleanup(
  cleanup: impl FnOnce() + Send + 'static,
  then: impl FnOnce(libc::c_int) + Send + 'static,
) -> std::io::Result<()> {
  let (mut reader, writer) = std::io::pipe()?;
  std::thread::Builder::new().name("jc-signal".into()).spawn(move || {
    let mut sig = [0u8];
    if reader.read_exact(&mut sig).is_ok() {
      cleanup();
      then(libc::c_int::from(sig[0]));
    }
  })?;
  SIGNAL_PIPE.store(writer.into_raw_fd(), Ordering::Release);
  Ok(())
}

/// The handler: one `write(2)`, which is async-signal-safe, and nothing that
/// takes a lock.
extern "C" fn on_signal(sig: libc::c_int) {
  let fd = SIGNAL_PIPE.load(Ordering::Acquire);
  if fd >= 0 {
    let byte = sig as u8;
    unsafe { libc::write(fd, (&raw const byte).cast(), 1) };
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::time::Duration;

  #[test]
  fn a_signal_while_this_thread_holds_the_claude_lock_does_not_hang() {
    let project = std::env::temp_dir().join(format!("jc-signal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    jc_core::hooks_settings::install_hooks(Flavor::Jc, &project, 1111).unwrap();
    let settings = project.join(".claude/settings.local.json");
    assert!(std::fs::read_to_string(&settings).unwrap().contains("/jc-hook/"));

    let (done_tx, done_rx) = flume::unbounded();
    let cleaned = project.clone();
    spawn_cleanup(
      move || jc_core::hooks_settings::uninstall_hooks(Flavor::Jc, &cleaned).unwrap(),
      move |sig| done_tx.send(sig).unwrap(),
    )
    .unwrap();

    let (returned_tx, returned_rx) = flume::unbounded();
    let dir = project.join(".claude");
    std::thread::spawn(move || {
      // The signal lands while this thread is inside install_hooks, holding
      // the lock on `.claude` (jc's start, on the main thread).
      let lock = std::fs::File::open(&dir).unwrap();
      lock.lock().unwrap();
      on_signal(libc::SIGTERM);
      returned_tx.send(()).unwrap();
      drop(lock);
    });
    returned_rx
      .recv_timeout(Duration::from_secs(10))
      .expect("the signal handler waited on the .claude lock its own thread holds");
    assert_eq!(done_rx.recv_timeout(Duration::from_secs(10)), Ok(libc::SIGTERM));
    assert!(!std::fs::read_to_string(&settings).unwrap().contains("/jc-hook/"), "hooks removed");
    std::fs::remove_dir_all(&project).unwrap();
  }
}
