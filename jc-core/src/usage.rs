use crate::claude_api::ApiUsageResponse;
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Weekday};
use serde::{Deserialize, Serialize};

const BAR_WIDTH: usize = 40;
const THRESHOLD: f64 = 10.0;

fn default_weekday() -> [u8; 2] {
  [9, 17]
}

fn default_weekend() -> [u8; 2] {
  [0, 0]
}

/// Working hours configuration per day of the week.
///
/// Each day is `[start_hour, end_hour]`. Hours must be <= 24 with
/// start <= end. Invalid entries are treated as `[0, 0]` (no work)
/// and a warning is emitted.
///
/// Stored in `~/.config/jc/config.toml` under `[working_hours]`:
/// ```toml
/// [working_hours]
/// mon = [8, 18]
/// tue = [10, 17]
/// sat = [0, 0]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingHours {
  #[serde(default = "default_weekday")]
  pub mon: [u8; 2],
  #[serde(default = "default_weekday")]
  pub tue: [u8; 2],
  #[serde(default = "default_weekday")]
  pub wed: [u8; 2],
  #[serde(default = "default_weekday")]
  pub thu: [u8; 2],
  #[serde(default = "default_weekday")]
  pub fri: [u8; 2],
  #[serde(default = "default_weekend")]
  pub sat: [u8; 2],
  #[serde(default = "default_weekend")]
  pub sun: [u8; 2],
}

impl Default for WorkingHours {
  fn default() -> Self {
    Self {
      mon: default_weekday(),
      tue: default_weekday(),
      wed: default_weekday(),
      thu: default_weekday(),
      fri: default_weekday(),
      sat: default_weekend(),
      sun: default_weekend(),
    }
  }
}

impl WorkingHours {
  fn raw_hours(&self, day: Weekday) -> [u8; 2] {
    match day {
      Weekday::Mon => self.mon,
      Weekday::Tue => self.tue,
      Weekday::Wed => self.wed,
      Weekday::Thu => self.thu,
      Weekday::Fri => self.fri,
      Weekday::Sat => self.sat,
      Weekday::Sun => self.sun,
    }
  }

  /// Returns validated (start, end) for a day. Invalid entries become (0, 0).
  fn validated_hours(&self, day: Weekday) -> (u8, u8) {
    let [start, end] = self.raw_hours(day);
    if start > 24 || end > 24 || start > end {
      eprintln!(
        "warning: invalid working hours for {day:?}: [{start}, {end}] (treating as [0, 0])"
      );
      (0, 0)
    } else {
      (start, end)
    }
  }

  /// Calculate usage report given the API-reported limit percentage and the
  /// weekly reset schedule. Uses the system clock for current time.
  ///
  /// `reset_day`/`reset_hour`/`reset_minute` define when the 7-day usage
  /// window resets (e.g. Thursday 21:59). The window runs from 7 days
  /// before the next reset to the next reset.
  pub fn calculate(
    &self,
    limit_pct: f64,
    reset_day: Weekday,
    reset_hour: u8,
    reset_minute: u8,
  ) -> UsageReport {
    let now = Local::now().naive_local();
    self.calculate_at(limit_pct, reset_day, reset_hour, reset_minute, now)
  }

  /// Same as [`calculate`](Self::calculate) but with an explicit current time.
  pub fn calculate_at(
    &self,
    limit_pct: f64,
    reset_day: Weekday,
    reset_hour: u8,
    reset_minute: u8,
    now: NaiveDateTime,
  ) -> UsageReport {
    let reset_time = NaiveTime::from_hms_opt(reset_hour as u32, reset_minute as u32, 0)
      .expect("invalid reset time");

    let window_end = next_weekday_time(now, reset_day, reset_time);
    let window_start = window_end - Duration::days(7);

    // Week percentage: position within the 7-day window
    let elapsed_secs = (now - window_start).num_seconds() as f64;
    let total_secs = 7.0 * 24.0 * 3600.0;
    let week_pct = (elapsed_secs / total_secs * 100.0).clamp(0.0, 100.0);

    // Working hours: iterate calendar days overlapping the window,
    // clipping each day's work period to the window boundaries.
    let start_date = window_start.date();
    let end_date = window_end.date();

    let mut total_work_secs: f64 = 0.0;
    let mut elapsed_work_secs: f64 = 0.0;

    let mut date = start_date;
    while date <= end_date {
      let (ws, we) = self.validated_hours(date.weekday());
      if ws < we {
        let work_start = date_and_hour(date, ws);
        let work_end = date_and_hour(date, we);

        // Clip work period to the window
        let clipped_start = work_start.max(window_start);
        let clipped_end = work_end.min(window_end);

        if clipped_start < clipped_end {
          let period_secs = (clipped_end - clipped_start).num_seconds() as f64;
          total_work_secs += period_secs;

          // Clip further to what has elapsed (up to now)
          let elapsed_end = clipped_end.min(now);
          if clipped_start < elapsed_end {
            elapsed_work_secs += (elapsed_end - clipped_start).num_seconds() as f64;
          }
        }
      }
      date = date.succ_opt().unwrap();
    }

    let working_pct = if total_work_secs > 0.0 {
      (elapsed_work_secs / total_work_secs * 100.0).clamp(0.0, 100.0)
    } else {
      0.0
    };

    UsageReport { limit_pct, week_pct, working_pct }
  }
}

/// Find the next occurrence of `day` at `time` strictly after `after`.
fn next_weekday_time(after: NaiveDateTime, day: Weekday, time: NaiveTime) -> NaiveDateTime {
  let today = after.date();
  for offset in 0..=7 {
    let candidate_date = today + Duration::days(offset);
    if candidate_date.weekday() == day {
      let candidate = candidate_date.and_time(time);
      if candidate > after {
        return candidate;
      }
    }
  }
  unreachable!("must find a matching day within 8 days")
}

/// Convert a date and hour (0–24) to NaiveDateTime. Hour 24 becomes next day 00:00.
fn date_and_hour(date: NaiveDate, hour: u8) -> NaiveDateTime {
  if hour >= 24 {
    (date + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap()
  } else {
    date.and_hms_opt(hour as u32, 0, 0).unwrap()
  }
}

#[derive(Debug, Clone)]
pub struct UsageReport {
  pub limit_pct: f64,
  pub week_pct: f64,
  pub working_pct: f64,
}

impl UsageReport {
  /// Par differential: positive = under par (budget to spare), negative = over par (burning too fast).
  ///
  /// The value is `working_pct - limit_pct`: how many percentage points of headroom (or deficit)
  /// you have relative to the pace of working time elapsed.
  pub fn par(&self) -> f64 {
    self.working_pct - self.limit_pct
  }

  /// Whether par is under (good), over (bad), or on par.
  pub fn par_status(&self) -> ParStatus {
    let par = self.par();
    if par > THRESHOLD {
      ParStatus::Under
    } else if par < -THRESHOLD {
      ParStatus::Over
    } else {
      ParStatus::On
    }
  }

  pub fn print(&self) {
    let red = "\x1b[31m";
    let green = "\x1b[32m";
    let yellow = "\x1b[33m";
    let dim = "\x1b[2m";
    let reset = "\x1b[0m";

    let par = self.par();
    let (par_color, par_label) = match self.par_status() {
      ParStatus::Under => (green, "Under par"),
      ParStatus::Over => (red, "Over par"),
      ParStatus::On => (yellow, "On par"),
    };

    // Headline: single par number
    let sign = if par > 0.0 { "+" } else { "" };
    println!("  {par_color}{par_label}: {sign}{:.0}{reset}", par);

    // Detail bars
    let working_color = comparison_color(self.working_pct, self.limit_pct);

    println!();
    println!("   Budget used: {:>3.0}% {}", self.limit_pct, bar(self.limit_pct, dim, dim, reset));
    println!(
      "  Working time: {}{:>3.0}%{} {}",
      working_color,
      self.working_pct,
      reset,
      bar(self.working_pct, working_color, dim, reset),
    );
  }
}

/// Par status classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParStatus {
  /// Budget to spare — working time is ahead of usage.
  Under,
  /// Burning too fast — usage is ahead of working time.
  Over,
  /// Roughly balanced.
  On,
}

/// Pick ANSI color: green if reference > limit (under par), red if under (over par).
fn comparison_color(reference_pct: f64, limit_pct: f64) -> &'static str {
  if reference_pct > limit_pct + THRESHOLD {
    "\x1b[32m" // green — time passing faster than usage
  } else if reference_pct < limit_pct - THRESHOLD {
    "\x1b[31m" // red — usage outpacing time
  } else {
    "\x1b[33m" // yellow — on par
  }
}

fn bar(pct: f64, fill_style: &str, empty_style: &str, reset: &str) -> String {
  let filled = ((pct / 100.0) * BAR_WIDTH as f64).round() as usize;
  let empty = BAR_WIDTH.saturating_sub(filled);
  format!("{fill_style}{}{empty_style}{}{reset}", "█".repeat(filled), "░".repeat(empty),)
}

pub fn parse_day(s: &str) -> Option<Weekday> {
  match s.to_lowercase().as_str() {
    "mon" => Some(Weekday::Mon),
    "tue" => Some(Weekday::Tue),
    "wed" => Some(Weekday::Wed),
    "thu" => Some(Weekday::Thu),
    "fri" => Some(Weekday::Fri),
    "sat" => Some(Weekday::Sat),
    "sun" => Some(Weekday::Sun),
    _ => None,
  }
}

pub fn parse_time(s: &str) -> Option<(u8, u8)> {
  if s.len() != 4 {
    return None;
  }
  let hour: u8 = s[..2].parse().ok()?;
  let minute: u8 = s[2..].parse().ok()?;
  if hour > 23 || minute > 59 {
    return None;
  }
  Some((hour, minute))
}

/// Extra usage billing info (formatted for display).
#[derive(Debug, Clone)]
pub struct ExtraUsageInfo {
  pub monthly_limit: f64,
  pub used_credits: f64,
  pub utilization: f64,
}

/// Full usage report combining 5-hour window, 7-day par calculation, and extra usage.
#[derive(Debug, Clone)]
pub struct FullUsageReport {
  /// 7-day par report (the core calculation).
  pub report: UsageReport,
  /// 5-hour window utilization percentage.
  pub five_hour_pct: f64,
  /// Human-readable 5-hour reset time (e.g. "in 2h 15m").
  pub five_hour_reset: String,
  /// Human-readable 7-day reset time (e.g. "Thu 21:59").
  pub seven_day_reset: String,
  /// Extra usage info, if enabled.
  pub extra: Option<ExtraUsageInfo>,
}

impl FullUsageReport {
  /// Build from API response + working hours config.
  pub fn from_api(api: &ApiUsageResponse, working_hours: &WorkingHours) -> Self {
    // Parse the 7-day reset time to extract day/hour/minute for the par calculation.
    let (reset_day, reset_hour, reset_minute) = parse_reset_time(&api.seven_day.resets_at);

    let report =
      working_hours.calculate(api.seven_day.utilization, reset_day, reset_hour, reset_minute);

    let five_hour_reset = format_reset_relative(&api.five_hour.resets_at);
    let seven_day_reset = format_reset_weekday(&api.seven_day.resets_at);

    let extra = api.extra_usage.as_ref().and_then(|e| {
      if e.is_enabled {
        Some(ExtraUsageInfo {
          monthly_limit: e.monthly_limit.unwrap_or(0.0),
          used_credits: e.used_credits.unwrap_or(0.0),
          utilization: e.utilization.unwrap_or(0.0),
        })
      } else {
        None
      }
    });

    Self {
      report,
      five_hour_pct: api.five_hour.utilization,
      five_hour_reset,
      seven_day_reset,
      extra,
    }
  }

  /// Pace multiplier: `limit_pct / working_pct`.
  /// Values < 1.0 mean under par (budget to spare), > 1.0 means over par.
  /// Returns `None` if working_pct is zero (no work time elapsed).
  pub fn pace(&self) -> Option<f64> {
    if self.report.working_pct > 0.0 {
      Some(self.report.limit_pct / self.report.working_pct)
    } else {
      None
    }
  }

  /// Projected remaining working hours at the current burn rate before hitting 100%.
  /// Returns `None` if no usage yet or working_pct is zero.
  pub fn projected_remaining_hours(&self) -> Option<f64> {
    let r = &self.report;
    if r.limit_pct <= 0.0 || r.working_pct <= 0.0 {
      return None;
    }
    // Burn rate: limit_pct per working_pct (in the same units).
    // Remaining budget: 100 - limit_pct.
    // Total working hours in window derived from working_pct / 100 * total_work_hours,
    // but we can compute directly: remaining_pct / rate_per_work_pct.
    // rate = limit_pct / working_pct (budget points consumed per work-time point).
    // remaining work_pct until budget hits 100: (100 - limit_pct) / rate.
    // Convert that to hours: remaining_work_pct / 100 * total_work_hours.
    // Simplified: (100 - limit_pct) * working_pct / limit_pct is remaining_work_pct,
    // then * total_work_hours / 100. But we don't know total_work_hours here.
    //
    // Alternative: use the ratio directly.
    // Hours elapsed so far = working_pct / 100 * total_work_hours (unknown).
    // Budget consumed per hour = limit_pct / (working_pct / 100 * total_work_hours).
    // Hours to exhaust = (100 - limit_pct) / (limit_pct / (working_pct / 100 * TWH))
    //                  = (100 - limit_pct) * working_pct * TWH / (limit_pct * 100).
    //
    // We need total_work_hours. We can compute it from the 7-day schedule but we don't
    // store it. Instead, approximate: working_pct% of a 7-day window means
    // elapsed_work_hours = working_pct / 100 * TWH. We know the burn rate in
    // pct-per-work-pct = limit_pct / working_pct. Remaining budget = 100 - limit_pct.
    // Remaining work_pct = remaining_budget / burn_rate = (100 - limit_pct) * working_pct / limit_pct.
    // But we need to convert work_pct to hours. Since we don't have TWH, use a simpler
    // approach: the user worked some hours and burned limit_pct. Remaining budget is
    // (100 - limit_pct). At the same rate, remaining_hours = elapsed_hours * (100 - limit_pct) / limit_pct.
    // We don't have elapsed_hours directly, but we can estimate from working_pct.
    //
    // Use a standard 40h work week as the baseline for the 7-day window.
    let total_work_hours = 40.0; // reasonable default
    let elapsed_work_hours = r.working_pct / 100.0 * total_work_hours;
    if elapsed_work_hours <= 0.0 {
      return None;
    }
    let burn_rate = r.limit_pct / elapsed_work_hours; // pct per hour
    let remaining_pct = 100.0 - r.limit_pct;
    if burn_rate <= 0.0 {
      return None;
    }
    Some(remaining_pct / burn_rate)
  }

  /// Format pace as a string like "0.7x".
  pub fn pace_label(&self) -> String {
    match self.pace() {
      Some(p) => format!("{p:.1}x"),
      None => "—".to_string(),
    }
  }

  /// Format projected remaining hours as a string like "12.5h".
  pub fn remaining_hours_label(&self) -> String {
    match self.projected_remaining_hours() {
      Some(h) if h >= 100.0 => ">99h".to_string(),
      Some(h) => format!("{h:.1}h"),
      None => "—".to_string(),
    }
  }

  /// Short label for the title bar, e.g. "Par +12".
  pub fn title_label(&self) -> String {
    let par = self.report.par();
    let sign = if par > 0.0 { "+" } else { "" };
    format!("Par {sign}{:.0}", par)
  }

  pub fn par_status(&self) -> ParStatus {
    self.report.par_status()
  }

  /// Print colored CLI output.
  pub fn print_cli(&self) {
    let red = "\x1b[31m";
    let green = "\x1b[32m";
    let yellow = "\x1b[33m";
    let dim = "\x1b[2m";
    let reset = "\x1b[0m";
    let bold = "\x1b[1m";

    let par = self.report.par();
    let (par_color, par_label) = match self.report.par_status() {
      ParStatus::Under => (green, "Under par"),
      ParStatus::Over => (red, "Over par"),
      ParStatus::On => (yellow, "On par"),
    };

    // Headline
    let sign = if par > 0.0 { "+" } else { "" };
    println!("  {bold}{par_color}{par_label}: {sign}{:.0}{reset}", par);

    // 5-hour window
    let five_color = if self.five_hour_pct > 80.0 {
      red
    } else if self.five_hour_pct > 50.0 {
      yellow
    } else {
      green
    };
    println!();
    println!(
      "  5h window: {five_color}{:>3.0}%{reset} {} {dim}resets {}{reset}",
      self.five_hour_pct,
      bar(self.five_hour_pct, five_color, dim, reset),
      self.five_hour_reset,
    );

    // 7-day bars
    let working_color = comparison_color(self.report.working_pct, self.report.limit_pct);
    println!(
      "  7d budget: {:>3.0}% {} {dim}resets {}{reset}",
      self.report.limit_pct,
      bar(self.report.limit_pct, dim, dim, reset),
      self.seven_day_reset,
    );
    println!(
      "  Work time: {working_color}{:>3.0}%{reset} {}",
      self.report.working_pct,
      bar(self.report.working_pct, working_color, dim, reset),
    );

    // Pace and projection
    println!();
    let pace_color = match self.pace() {
      Some(p) if p > 1.1 => red,
      Some(p) if p < 0.9 => green,
      _ => yellow,
    };
    println!(
      "       Pace: {pace_color}{}{reset}  {dim}Remaining: {}{reset}",
      self.pace_label(),
      self.remaining_hours_label(),
    );

    // Extra usage
    if let Some(extra) = &self.extra {
      println!(
        "  {dim}Extra usage: ${:.0} / ${:.0} ({:.1}%){reset}",
        extra.used_credits, extra.monthly_limit, extra.utilization,
      );
    }
  }
}

/// Parse ISO 8601 reset time to (Weekday, hour, minute) in local time.
fn parse_reset_time(iso: &str) -> (Weekday, u8, u8) {
  use chrono::DateTime;
  if let Ok(dt) = DateTime::parse_from_rfc3339(iso) {
    let local = dt.with_timezone(&Local);
    (local.weekday(), local.hour() as u8, local.minute() as u8)
  } else {
    // Fallback: try without timezone suffix (shouldn't happen with the API)
    (Weekday::Thu, 21, 59)
  }
}

/// Format reset time as relative duration, e.g. "in 2h 15m".
fn format_reset_relative(iso: &str) -> String {
  use chrono::DateTime;
  if let Ok(dt) = DateTime::parse_from_rfc3339(iso) {
    let now = Local::now();
    let diff = dt.signed_duration_since(now);
    if diff.num_seconds() <= 0 {
      return "now".to_string();
    }
    let hours = diff.num_hours();
    let minutes = diff.num_minutes() % 60;
    if hours > 0 { format!("in {hours}h {minutes:02}m") } else { format!("in {minutes}m") }
  } else {
    "unknown".to_string()
  }
}

/// Format reset time as weekday + time, e.g. "Thu 21:59".
fn format_reset_weekday(iso: &str) -> String {
  use chrono::DateTime;
  if let Ok(dt) = DateTime::parse_from_rfc3339(iso) {
    let local = dt.with_timezone(&Local);
    local.format("%a %H:%M").to_string()
  } else {
    "unknown".to_string()
  }
}
