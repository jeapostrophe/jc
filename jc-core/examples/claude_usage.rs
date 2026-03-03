//! Claude usage par calculator.
//!
//! ```sh
//! cargo run -p jc-core --example claude_usage -- 38 thu 2159
//! ```
//!
//! Arguments:
//!   <limit_pct>   API-reported weekly usage percentage (0-100)
//!   <reset_day>   Day the 7-day window resets (mon/tue/wed/thu/fri/sat/sun)
//!   <reset_HHMM>  Time the window resets in 24h local time (e.g. 2159)
//!
//! The current time is read from the system clock.
//!
//! Reads working hours from ~/.config/jc/config.toml `[working_hours]` section.
//! Defaults to 9-17 weekdays if unconfigured.

use jc_core::config::load_config;
use jc_core::usage::{parse_day, parse_time};

fn main() {
  let args: Vec<String> = std::env::args().collect();
  if args.len() != 4 {
    eprintln!("Usage: claude_usage <limit_pct> <reset_day> <reset_HHMM>");
    eprintln!("Example: claude_usage 38 thu 2159");
    eprintln!();
    eprintln!("  limit_pct   API-reported weekly usage percentage (0-100)");
    eprintln!("  reset_day   Day the 7-day window resets (mon/tue/wed/thu/fri/sat/sun)");
    eprintln!("  reset_HHMM  Time the window resets in 24h local time (e.g. 2159)");
    std::process::exit(1);
  }

  let limit_pct: f64 = args[1].parse().unwrap_or_else(|_| {
    eprintln!("error: limit_pct must be a number (got {:?})", args[1]);
    std::process::exit(1);
  });

  let reset_day = parse_day(&args[2]).unwrap_or_else(|| {
    eprintln!("error: day must be mon/tue/wed/thu/fri/sat/sun (got {:?})", args[2]);
    std::process::exit(1);
  });

  let (reset_hour, reset_minute) = parse_time(&args[3]).unwrap_or_else(|| {
    eprintln!("error: time must be HHMM format, e.g. 2159 (got {:?})", args[3]);
    std::process::exit(1);
  });

  let working_hours = match load_config() {
    Ok(config) => config.working_hours,
    Err(e) => {
      eprintln!("warning: failed to load config: {e:#}");
      eprintln!("using default working hours (Mon-Fri 9-17)");
      Default::default()
    }
  };

  let report = working_hours.calculate(limit_pct, reset_day, reset_hour, reset_minute);
  report.print();
}
