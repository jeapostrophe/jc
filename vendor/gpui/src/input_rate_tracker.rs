use std::time::{Duration, Instant};

/// Tracks input event rates to decide when VRR (Variable Refresh Rate)
/// keepalive presentation is needed.
///
/// Only activates continuous frame presentation when input arrives at
/// a sustained high rate (e.g. scrolling, dragging), not on casual
/// mouse moves or occasional keystrokes.
pub(crate) struct InputRateTracker {
    timestamps: Vec<Instant>,
    /// Sliding window over which to measure input rate.
    window: Duration,
    /// Minimum events-per-second to trigger VRR sustain.
    inputs_per_second: u32,
    /// VRR keepalive stays active until this instant.
    sustain_until: Instant,
    /// How long to sustain VRR after high-rate input stops.
    sustain_duration: Duration,
}

impl InputRateTracker {
    pub fn new() -> Self {
        Self {
            timestamps: Vec::new(),
            window: Duration::from_millis(100),
            inputs_per_second: 60,
            sustain_until: Instant::now(),
            sustain_duration: Duration::from_secs(1),
        }
    }

    /// Record an input event. Only call this when the input actually caused
    /// the window to become dirty (i.e. triggered a re-render).
    pub fn record_input(&mut self) {
        let now = Instant::now();
        self.timestamps.push(now);
        self.prune_old_timestamps(now);

        let min_events =
            self.inputs_per_second as u128 * self.window.as_millis() / 1000;
        if self.timestamps.len() as u128 >= min_events {
            self.sustain_until = now + self.sustain_duration;
        }
    }

    /// Returns true if VRR keepalive should be active (present frames
    /// even when not dirty to prevent display refresh rate downclocking).
    pub fn is_high_rate(&self) -> bool {
        Instant::now() < self.sustain_until
    }

    fn prune_old_timestamps(&mut self, now: Instant) {
        let cutoff = now.checked_sub(self.window).unwrap_or(now);
        self.timestamps.retain(|t| *t >= cutoff);
    }
}
