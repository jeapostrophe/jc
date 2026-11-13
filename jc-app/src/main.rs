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
use std::path::PathBuf;

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
  // `jcc` (the jcc.app bundle's executable) is Cira's client; any other name
  // is jc (Cira PLAN TMUX-07). Chosen before anything reads config or TODO.md.
  let flavor = Flavor::from_program(std::env::args_os().next().as_deref()).init();
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

/// Store project paths globally so the signal handler can access them.
static SIGNAL_CLEANUP_PATHS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

fn install_signal_handler(state: &config::AppState) {
  let paths: Vec<PathBuf> = state.projects.iter().map(|p| p.path.clone()).collect();
  *SIGNAL_CLEANUP_PATHS.lock().unwrap() = paths;

  unsafe {
    libc::signal(libc::SIGINT, sigint_handler as *const () as libc::sighandler_t);
    libc::signal(libc::SIGTERM, sigint_handler as *const () as libc::sighandler_t);
  }
}

extern "C" fn sigint_handler(sig: libc::c_int) {
  // Signal handlers must be async-signal-safe. We do minimal work:
  // uninstall_hooks only does file I/O (open, read, write), which is
  // technically not async-signal-safe but is reliable in practice for
  // single-threaded cleanup before exit.
  ipc::cleanup_socket();

  if let Ok(paths) = SIGNAL_CLEANUP_PATHS.lock() {
    for path in paths.iter() {
      let _ = jc_core::hooks_settings::uninstall_hooks(Flavor::current(), path);
    }
  }

  // Re-raise with default handler for normal exit behavior.
  unsafe {
    libc::signal(sig, libc::SIG_DFL);
    libc::raise(sig);
  }
}
