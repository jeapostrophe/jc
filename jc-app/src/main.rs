mod app;
mod language;
mod outline;
mod views;

use clap::{Parser, Subcommand};
use jc_core::config;
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
  /// Show Claude usage par report.
  Usage {
    /// API-reported weekly usage percentage (0-100). If omitted, fetches from API.
    limit_pct: Option<f64>,
    /// Day the 7-day window resets (mon/tue/wed/thu/fri/sat/sun).
    reset_day: Option<String>,
    /// Time the window resets in 24h local time (e.g. 2159).
    reset_hhmm: Option<String>,
  },
}

fn main() -> anyhow::Result<()> {
  let cli = Cli::parse();

  if let Some(Command::Usage { limit_pct, reset_day, reset_hhmm }) = cli.command {
    return cmd_usage(limit_pct, reset_day, reset_hhmm);
  }

  let config = config::load_config()?;
  let mut state = config::load_state()?;

  if let Some(path) = &cli.path {
    state.register_project(path);
    config::save_state(&state)?;
  } else if state.projects.is_empty() {
    let cwd = std::env::current_dir()?;
    state.register_project(&cwd);
    config::save_state(&state)?;
  }

  app::run(state, config);
  Ok(())
}

fn cmd_usage(
  limit_pct: Option<f64>,
  reset_day: Option<String>,
  reset_hhmm: Option<String>,
) -> anyhow::Result<()> {
  use jc_core::claude_api;
  use jc_core::usage::{FullUsageReport, parse_day, parse_time};

  let config = config::load_config()?;

  match (limit_pct, reset_day, reset_hhmm) {
    // Manual args mode: jc usage 38 thu 2159
    (Some(pct), Some(day_str), Some(time_str)) => {
      let day = parse_day(&day_str).ok_or_else(|| {
        anyhow::anyhow!("invalid day: {day_str} (use mon/tue/wed/thu/fri/sat/sun)")
      })?;
      let (hour, minute) = parse_time(&time_str)
        .ok_or_else(|| anyhow::anyhow!("invalid time: {time_str} (use HHMM format, e.g. 2159)"))?;
      let report = config.working_hours.calculate(pct, day, hour, minute);
      report.print();
    }
    // API mode: jc usage
    (None, None, None) => {
      let token = claude_api::load_oauth_token()?;
      let api = claude_api::fetch_usage(&token)?;
      let full = FullUsageReport::from_api(&api, &config.working_hours);
      full.print_cli();
    }
    _ => {
      anyhow::bail!("usage: jc usage [<limit_pct> <reset_day> <reset_HHMM>]");
    }
  }

  Ok(())
}
