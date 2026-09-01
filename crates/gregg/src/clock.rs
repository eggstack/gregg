#![allow(dead_code)]

//! Time abstraction for deterministic testing.
//!
//! The [`Clock`] trait allows tests to inject a fake clock instead of
//! relying on wall-clock time.

use std::time::Instant;

/// An abstraction over time sources.
///
/// Production code uses [`RealClock`]; tests can inject a [`FakeClock`]
/// to control timing without sleeping.
pub trait Clock {
    /// Return the current instant.
    fn now(&self) -> Instant;

    /// Return the current Tokio timer instant.
    ///
    /// Production code overrides this so Tokio timers remain aligned with the
    /// timer runtime. Injected clocks use that same runtime clock for timer
    /// deadlines; their wall-clock value remains independently controllable.
    fn tokio_now(&self) -> tokio::time::Instant {
        tokio::time::Instant::from_std(self.now())
    }
}

/// The real system clock backed by [`Instant::now`].
#[derive(Clone, Copy)]
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn tokio_now(&self) -> tokio::time::Instant {
        tokio::time::Instant::now()
    }
}

/// A fake clock for deterministic tests.
///
/// Starts at a fixed point and advances only when [`FakeClock::advance`]
/// is called.
#[derive(Clone)]
pub struct FakeClock {
    current: Instant,
}

impl FakeClock {
    /// Create a new fake clock anchored at the given instant.
    #[must_use]
    pub fn new(anchor: Instant) -> Self {
        Self { current: anchor }
    }

    /// Advance the clock by the given duration.
    pub fn advance(&mut self, delta: std::time::Duration) {
        self.current += delta;
    }

    /// Set the clock to an exact instant.
    pub fn set(&mut self, instant: Instant) {
        self.current = instant;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.current
    }

    fn tokio_now(&self) -> tokio::time::Instant {
        // Tokio's paused clock is advanced by `tokio::time::advance`, not by
        // the fake wall clock. Keep sleep deadlines in the runtime clock.
        tokio::time::Instant::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn real_clock_returns_instant() {
        let clock = RealClock;
        let before = Instant::now();
        let t = clock.now();
        let after = Instant::now();
        assert!(t >= before);
        assert!(t <= after);
    }

    #[test]
    fn fake_clock_starts_at_anchor() {
        let anchor = Instant::now();
        let clock = FakeClock::new(anchor);
        assert_eq!(clock.now(), anchor);
    }

    #[test]
    fn fake_clock_advances() {
        let anchor = Instant::now();
        let mut clock = FakeClock::new(anchor);
        clock.advance(Duration::from_secs(5));
        assert_eq!(clock.now(), anchor + Duration::from_secs(5));
    }

    #[test]
    fn fake_clock_set() {
        let anchor = Instant::now();
        let mut clock = FakeClock::new(anchor);
        let target = anchor + Duration::from_secs(100);
        clock.set(target);
        assert_eq!(clock.now(), target);
    }
}
