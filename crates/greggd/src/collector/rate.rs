//! Monotonic cumulative-counter baselines used by live telemetry collectors.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// One cumulative two-direction counter observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CounterSample {
    pub(crate) observed_at: Instant,
    pub(crate) first: u64,
    pub(crate) second: u64,
}

/// Two rates calculated from one valid counter interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CounterRates {
    pub(crate) first_per_sec: u64,
    pub(crate) second_per_sec: u64,
}

/// Identity-keyed baselines for cumulative counters.
#[derive(Debug, Default)]
pub(crate) struct CounterBaselines {
    samples: HashMap<String, CounterSample>,
}

impl CounterBaselines {
    /// Store an observation and return rates only when the prior observation
    /// is continuous, monotonic, and has a positive elapsed interval.
    pub(crate) fn observe(
        &mut self,
        id: &str,
        observed_at: Instant,
        first: u64,
        second: u64,
    ) -> Option<CounterRates> {
        let current = CounterSample {
            observed_at,
            first,
            second,
        };
        let previous = self.samples.insert(id.to_owned(), current)?;
        let elapsed = observed_at.checked_duration_since(previous.observed_at)?;
        if elapsed.is_zero() || first < previous.first || second < previous.second {
            return None;
        }
        Some(CounterRates {
            first_per_sec: rate_per_second(first - previous.first, elapsed)?,
            second_per_sec: rate_per_second(second - previous.second, elapsed)?,
        })
    }

    /// Forget identities absent from the current native enumeration.
    pub(crate) fn retain_ids<'a>(&mut self, ids: impl IntoIterator<Item = &'a str>) {
        let ids: HashSet<&str> = ids.into_iter().collect();
        self.samples.retain(|id, _| ids.contains(id.as_str()));
    }

    /// Discard all observations after a source failure so the next successful
    /// query establishes a fresh, trustworthy interval.
    pub(crate) fn clear(&mut self) {
        self.samples.clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.samples.len()
    }
}

/// Convert a counter delta to a per-second integer without using a nominal
/// sampler interval. `Duration::as_nanos` and `u128` keep the multiplication
/// checked even for large byte counters and sub-second samples.
fn rate_per_second(delta: u64, elapsed: Duration) -> Option<u64> {
    let nanos = elapsed.as_nanos();
    let scaled = u128::from(delta).checked_mul(1_000_000_000)?;
    u64::try_from(scaled.checked_div(nanos)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instant(seconds: u64, nanos: u32) -> Instant {
        Instant::now() + Duration::from_secs(seconds) + Duration::from_nanos(u64::from(nanos))
    }

    #[test]
    fn first_observation_warms_and_exact_second_is_rate() {
        let mut baselines = CounterBaselines::default();
        let start = instant(0, 0);
        assert_eq!(baselines.observe("eth0", start, 100, 200), None);
        assert_eq!(
            baselines.observe("eth0", start + Duration::from_secs(1), 200, 500),
            Some(CounterRates {
                first_per_sec: 100,
                second_per_sec: 300,
            })
        );
    }

    #[test]
    fn fractional_and_long_elapsed_intervals_use_actual_time() {
        let mut baselines = CounterBaselines::default();
        let start = instant(0, 0);
        baselines.observe("x", start, 0, 0);
        assert_eq!(
            baselines
                .observe("x", start + Duration::from_millis(250), 25, 50)
                .expect("rate"),
            CounterRates {
                first_per_sec: 100,
                second_per_sec: 200,
            }
        );
        assert_eq!(
            baselines
                .observe("x", start + Duration::from_millis(1_333), 158, 316)
                .expect("rate"),
            CounterRates {
                first_per_sec: 122,
                second_per_sec: 245,
            }
        );
    }

    #[test]
    fn reset_zero_elapsed_and_hotplug_rebaseline() {
        let mut baselines = CounterBaselines::default();
        let start = instant(0, 0);
        baselines.observe("x", start, 100, 100);
        assert_eq!(baselines.observe("x", start, 101, 101), None);
        assert_eq!(
            baselines.observe("x", start + Duration::from_secs(1), 99, 102),
            None
        );
        assert_eq!(
            baselines.observe("x", start + Duration::from_secs(2), 199, 202),
            Some(CounterRates {
                first_per_sec: 100,
                second_per_sec: 100
            })
        );
        baselines.retain_ids(["y"]);
        assert_eq!(baselines.len(), 0);
        assert_eq!(
            baselines.observe("x", start + Duration::from_secs(3), 300, 300),
            None
        );
    }

    #[test]
    fn overflow_safe_rate_conversion_returns_none() {
        let mut baselines = CounterBaselines::default();
        let start = instant(0, 0);
        baselines.observe("x", start, 0, 0);
        assert_eq!(
            baselines.observe("x", start + Duration::from_nanos(1), u64::MAX, 0),
            None
        );
    }
}
