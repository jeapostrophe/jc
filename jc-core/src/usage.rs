use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, Weekday};
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

pub struct UsageReport {
  pub limit_pct: f64,
  pub week_pct: f64,
  pub working_pct: f64,
}

impl UsageReport {
  pub fn print(&self) {
    let red = "\x1b[31m";
    let green = "\x1b[32m";
    let dim = "\x1b[2m";
    let reset = "\x1b[0m";

    let week_color = comparison_color(self.week_pct, self.limit_pct);
    let working_color = comparison_color(self.working_pct, self.limit_pct);

    println!("   Limit Usage: {:>3.0}% {}", self.limit_pct, bar(self.limit_pct, dim, dim, reset),);
    println!(
      "    Week Usage: {}{:>3.0}%{} {}",
      week_color,
      self.week_pct,
      reset,
      bar(self.week_pct, week_color, dim, reset),
    );
    println!(
      " Working Usage: {}{:>3.0}%{} {}",
      working_color,
      self.working_pct,
      reset,
      bar(self.working_pct, working_color, dim, reset),
    );

    // Summary line
    let diff = self.working_pct - self.limit_pct;
    if diff > THRESHOLD {
      println!(
        "\n  {green}Under par{reset} — {:.0}% of working time elapsed, only {:.0}% of budget used",
        self.working_pct, self.limit_pct,
      );
    } else if diff < -THRESHOLD {
      println!(
        "\n  {red}Over par{reset} — {:.0}% of budget used, but only {:.0}% of working time elapsed",
        self.limit_pct, self.working_pct,
      );
    } else {
      println!(
        "\n  On par — {:.0}% budget vs {:.0}% working time",
        self.limit_pct, self.working_pct,
      );
    }
  }
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
