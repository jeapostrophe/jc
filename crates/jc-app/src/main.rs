mod app;
mod language;
mod outline;
mod views;

use clap::Parser;
use jc_core::config;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "jc", about = "Claude Code session orchestrator")]
struct Cli {
  /// Path to a project directory to open or register.
  path: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
  let cli = Cli::parse();

  let config = config::load_config()?;
  let mut state = config::load_state()?;

  if let Some(path) = &cli.path {
    state.register_project(path);
    config::save_state(&state)?;
  }

  app::run(state, config);
  Ok(())
}
