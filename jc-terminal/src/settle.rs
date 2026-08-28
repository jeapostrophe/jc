//! Launch-settle policy: when a freshly spawned child's startup output is over.
//!
//! Pure — it observes `Instant`s and answers with a decision — so the policy is
//! testable without a live PTY. `TerminalView::discount_launch_output` drives it
//! from the relay that already sees every parsed batch.

use std::time::{Duration, Instant};

/// How long a child must be quiet before its launch output is discounted.
/// Longer than a moment because `claude --resume` pauses between printing its
/// banner and replaying a long transcript, and that pause must not end the
/// window.
pub(crate) const LAUNCH_QUIET: Duration = Duration::from_secs(4);

/// Absolute bound on the window. A child that never goes quiet is baselined
/// anyway rather than being tracked for the life of the process.
pub(crate) const LAUNCH_GIVE_UP: Duration = Duration::from_secs(30);

/// What the driver should do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettleStep {
  /// Take the baseline now.
  Clear,
  /// Sleep this long, then step again.
  Wait(Duration),
  /// The window is over — nothing more to do, and nothing to clear.
  Done,
}

/// Progress toward discounting one child's launch output.
#[derive(Debug)]
pub(crate) struct SettleWindow {
  quiet: Duration,
  /// Absolute give-up deadline.
  deadline: Instant,
  /// When the most recent batch arrived, or `None` while the child has printed
  /// nothing at all.
  last_batch: Option<Instant>,
  /// Set once the window has ended, so a session is baselined exactly once.
  finished: bool,
}

impl SettleWindow {
  pub(crate) fn new(quiet: Duration, give_up: Duration, now: Instant) -> Self {
    Self { quiet, deadline: now + give_up, last_batch: None, finished: false }
  }

  /// Note that the child printed something at `now`, restarting the quiet wait.
  pub(crate) fn note_batch(&mut self, now: Instant) {
    self.last_batch = Some(now);
  }

  /// End the window without clearing.
  pub(crate) fn cancel(&mut self) {
    self.finished = true;
  }

  /// Decide what to do at `now`.
  pub(crate) fn step(&mut self, now: Instant) -> SettleStep {
    if self.finished {
      return SettleStep::Done;
    }
    let to_deadline = self.deadline.saturating_duration_since(now);
    if to_deadline.is_zero() {
      // The give-up path CLEARS. A child that never settles is still printing,
      // so it re-marks itself on its next batch; a marker left set here would
      // never retire (only switching away clears one).
      self.finished = true;
      return SettleStep::Clear;
    }
    // A child that has printed nothing yet is not quiet, it is slow to start;
    // baselining now would let its banner land above the baseline. Sleep to the
    // give-up deadline — the next batch is what reopens the quiet wait.
    let Some(last_batch) = self.last_batch else {
      return SettleStep::Wait(to_deadline);
    };
    let quiet_for = now.saturating_duration_since(last_batch);
    match self.quiet.checked_sub(quiet_for) {
      Some(remaining) if !remaining.is_zero() => SettleStep::Wait(remaining.min(to_deadline)),
      _ => {
        self.finished = true;
        SettleStep::Clear
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const QUIET: Duration = Duration::from_secs(4);
  const GIVE_UP: Duration = Duration::from_secs(30);

  fn window(start: Instant) -> SettleWindow {
    SettleWindow::new(QUIET, GIVE_UP, start)
  }

  /// A child that has printed NOTHING is slow to start, not quiet. Baselining
  /// it now would let its banner land above the baseline and mark the session.
  #[test]
  fn silent_child_is_never_cleared_before_it_prints() {
    let start = Instant::now();
    let mut w = window(start);
    for elapsed in [Duration::ZERO, QUIET, QUIET * 2, GIVE_UP - Duration::from_millis(1)] {
      assert!(
        matches!(w.step(start + elapsed), SettleStep::Wait(_)),
        "a child that has printed nothing must not be baselined at {elapsed:?}; \
         it is slow to start, not quiet"
      );
    }
  }

  /// While no batch has arrived, the only thing left to wake for is the give-up
  /// deadline, so the wait must run exactly to it — never past it.
  #[test]
  fn wait_before_first_batch_runs_to_the_give_up_deadline() {
    let start = Instant::now();
    let mut w = window(start);
    assert_eq!(
      w.step(start + Duration::from_secs(5)),
      SettleStep::Wait(GIVE_UP - Duration::from_secs(5)),
      "with nothing printed yet the next wake must be the give-up deadline"
    );
  }

  /// `claude --resume` pauses between banner and transcript replay. The window
  /// must ride that pause out: the quiet wait restarts at each batch.
  #[test]
  fn quiet_window_restarts_on_each_batch() {
    let start = Instant::now();
    let mut w = window(start);
    w.note_batch(start + Duration::from_secs(1));
    w.note_batch(start + Duration::from_secs(3));

    assert!(
      matches!(w.step(start + Duration::from_secs(6)), SettleStep::Wait(_)),
      "only 3s of quiet since the last batch — the window must still be open"
    );
    assert_eq!(
      w.step(start + Duration::from_secs(7)),
      SettleStep::Clear,
      "4s of quiet since the last batch — the child has settled"
    );
  }

  /// A session is baselined once and then left alone; re-clearing later would
  /// swallow the real output the marker exists to report.
  #[test]
  fn clears_exactly_once() {
    let start = Instant::now();
    let mut w = window(start);
    w.note_batch(start + Duration::from_secs(1));
    assert_eq!(w.step(start + Duration::from_secs(5)), SettleStep::Clear);
    assert_eq!(
      w.step(start + Duration::from_secs(6)),
      SettleStep::Done,
      "a settled session must be baselined exactly once"
    );
  }

  /// User input ends the window immediately WITHOUT clearing: from then on
  /// everything the child prints answers something you asked for.
  #[test]
  fn cancel_ends_the_window_without_clearing() {
    let start = Instant::now();
    let mut w = window(start);
    w.note_batch(start + Duration::from_secs(1));
    w.cancel();
    assert_eq!(
      w.step(start + Duration::from_secs(2)),
      SettleStep::Done,
      "user input must end the window at once"
    );
    assert_eq!(
      w.step(start + GIVE_UP + Duration::from_secs(1)),
      SettleStep::Done,
      "a cancelled window must never clear, not even at the give-up deadline"
    );
  }

  /// The give-up path CLEARS. A child still printing re-marks itself on its
  /// next batch, whereas a marker left set is never retired.
  #[test]
  fn gives_up_and_clears_when_the_child_never_settles() {
    let start = Instant::now();
    let mut w = window(start);
    let mut elapsed = Duration::from_secs(1);
    while elapsed < GIVE_UP {
      w.note_batch(start + elapsed);
      assert!(
        matches!(w.step(start + elapsed), SettleStep::Wait(_)),
        "a child still printing at {elapsed:?} has not settled"
      );
      elapsed += Duration::from_secs(1);
    }
    w.note_batch(start + GIVE_UP);
    assert_eq!(
      w.step(start + GIVE_UP),
      SettleStep::Clear,
      "the give-up path must clear, not merely stop waiting"
    );
  }

  /// A wait must never run past the give-up deadline, or a chatty child would
  /// keep the window open beyond its bound.
  #[test]
  fn wait_never_overruns_the_give_up_deadline() {
    let start = Instant::now();
    let mut w = window(start);
    let late = GIVE_UP - Duration::from_secs(1);
    w.note_batch(start + late);
    assert_eq!(
      w.step(start + late),
      SettleStep::Wait(Duration::from_secs(1)),
      "the quiet wait must be clamped to the give-up deadline"
    );
  }
}
