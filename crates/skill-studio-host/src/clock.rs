//! [`Clock`] over the wall clock and a monotonic `Instant`.

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use skill_studio_core::ports::Clock;

/// `Clock` backed by `chrono::Utc::now` and a process-local `Instant`.
#[derive(Debug, Clone)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    /// Builds a clock whose monotonic zero point is now.
    pub fn new() -> Self {
        SystemClock {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn monotonic(&self) -> Duration {
        self.start.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_time_advances() {
        let clock = SystemClock::new();
        let first = clock.monotonic();
        std::thread::sleep(Duration::from_millis(5));
        assert!(clock.monotonic() > first);
    }
}
