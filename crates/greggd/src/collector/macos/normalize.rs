//! Shared normalization helpers for the macOS collector.

/// Compute a utilization percentage from used and total byte counts.
///
/// Returns `0.0` when `total` is zero to avoid division by zero. The result
/// is clamped to the closed interval `0.0..=100.0`.
pub(crate) fn percent(used: u64, total: u64) -> f32 {
    crate::collector::clamped_usage_pct(used, total)
}

/// Clip an identifier string to `max_len` bytes on a valid UTF-8 boundary.
pub(crate) fn clip_identifier(input: &str, max_len: usize) -> String {
    let input = input.trim();
    if input.len() <= max_len {
        input.to_string()
    } else {
        let mut end = max_len;
        while end > 0 && !input.is_char_boundary(end) {
            end -= 1;
        }
        input[..end].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_zero_total() {
        assert!((percent(0, 0) - 0.0).abs() < f32::EPSILON);
        assert!((percent(100, 0) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn percent_normal() {
        assert!((percent(50, 100) - 50.0).abs() < 1e-6);
        assert!((percent(1, 3) - 33.333_332).abs() < 0.01);
    }

    #[test]
    fn percent_clamped() {
        assert!((percent(u64::MAX, 1) - 100.0).abs() < f32::EPSILON);
    }

    #[test]
    fn clip_short_string() {
        assert_eq!(clip_identifier("hello", 10), "hello");
    }

    #[test]
    fn clip_long_string() {
        assert_eq!(clip_identifier("hello world", 5), "hello");
    }

    #[test]
    fn clip_on_char_boundary() {
        assert_eq!(clip_identifier("hello!", 5), "hello");
    }
}
