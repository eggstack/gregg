//! Strict five-field cron schedules in the daemon's local civil timezone.

use chrono::{DateTime, TimeZone};
use cron_parser::Schedule;

/// Maximum UTF-8 byte length accepted for a cron expression.
pub(crate) const MAX_SCHEDULE_BYTES: usize = 128;

/// A parsed five-field cron schedule.
#[derive(Debug, Clone)]
pub(crate) struct LocalSchedule {
    schedules: Vec<Schedule>,
}

impl LocalSchedule {
    /// Parse ordinary five-field numeric cron syntax, plus four roadmap aliases.
    pub(crate) fn parse(expression: &str) -> Result<Self, String> {
        if expression.is_empty() || expression.len() > MAX_SCHEDULE_BYTES {
            return Err(format!(
                "schedule must contain 1..={MAX_SCHEDULE_BYTES} UTF-8 bytes"
            ));
        }
        let normalized = match expression.trim() {
            "@hourly" => "0 * * * *",
            "@daily" => "0 0 * * *",
            "@weekly" => "0 0 * * 0",
            "@monthly" => "0 0 1 * *",
            value => value,
        };
        let fields: Vec<_> = normalized.split_whitespace().collect();
        if fields.len() != 5 {
            return Err("schedule must use exactly five cron fields".to_owned());
        }
        for field in &fields {
            if !field
                .chars()
                .all(|ch| ch.is_ascii_digit() || matches!(ch, '*' | '/' | ',' | '-'))
            {
                return Err("schedule fields accept only digits, '*', ',', '-', and '/'".to_owned());
            }
        }

        let parse = |parts: &[&str]| {
            parts
                .join(" ")
                .parse::<Schedule>()
                .map_err(|error| error.to_string())
        };
        let mut schedules = vec![parse(&fields)?];
        // Traditional cron ORs restricted day-of-month and day-of-week.
        // cron-parser uses AND, so compute each branch and take the earlier
        // instant when both fields are restricted.
        if fields[2] != "*" && fields[4] != "*" {
            for day_index in [2, 4] {
                let mut alternative = fields.clone();
                alternative[day_index] = "*";
                schedules.push(parse(&alternative)?);
            }
        }
        Ok(Self { schedules })
    }

    /// Return the next matching instant strictly after `after`.
    pub(crate) fn next_after<Tz: TimeZone>(
        &self,
        after: &DateTime<Tz>,
    ) -> Result<DateTime<Tz>, String> {
        self.schedules
            .iter()
            .filter_map(|schedule| schedule.next_after(after))
            .min()
            .ok_or_else(|| "cron expression has no future occurrence".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, NaiveDate, TimeZone, Timelike, Utc};
    use chrono_tz::America::New_York;

    fn local(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
    }

    #[test]
    fn five_field_contract_and_aliases() {
        for expression in [
            "* * * * *",
            "0 3 * * 0",
            "0 0 1 * *",
            "@hourly",
            "@daily",
            "@weekly",
            "@monthly",
        ] {
            assert!(LocalSchedule::parse(expression).is_ok(), "{expression}");
        }
        for expression in [
            "0 0 1 *",
            "0 0 1 * * 2026",
            "0 0 1 * * *",
            "@reboot",
            "0 3 * * SUN",
        ] {
            assert!(LocalSchedule::parse(expression).is_err(), "{expression}");
        }
        assert!(LocalSchedule::parse("0,30 8-18/2 * * 1-5").is_ok());
    }

    #[test]
    fn minute_weekday_month_leap_dom_dow_and_exclusive_semantics() {
        let after = local(2026, 10, 3, 12, 0);
        let every_minute = LocalSchedule::parse("* * * * *").unwrap();
        assert_eq!(every_minute.next_after(&after).unwrap().minute(), 1);

        let weekly = LocalSchedule::parse("0 0 * * 0").unwrap();
        assert_eq!(
            weekly.next_after(&after).unwrap().weekday(),
            chrono::Weekday::Sun
        );

        let month = LocalSchedule::parse("0 0 1 * *").unwrap();
        assert_eq!(month.next_after(&after).unwrap().day(), 1);

        let leap = LocalSchedule::parse("0 0 29 2 *").unwrap();
        assert_eq!(
            leap.next_after(&local(2025, 3, 1, 0, 0)).unwrap().year(),
            2028
        );

        let dom_dow = LocalSchedule::parse("0 0 1 * 1").unwrap();
        let after = local(2026, 6, 30, 23, 59);
        assert_eq!(
            dom_dow.next_after(&after).unwrap().date_naive(),
            NaiveDate::from_ymd_opt(2026, 7, 1).unwrap()
        );

        let instant = Utc.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap();
        assert!(every_minute.next_after(&instant).unwrap() > instant);
    }

    #[test]
    fn civil_time_skips_spring_gap_and_returns_both_fall_overlap_instants() {
        let spring: DateTime<_> = New_York.with_ymd_and_hms(2026, 3, 8, 1, 59, 0).unwrap();
        let two_thirty = LocalSchedule::parse("30 2 * * *").unwrap();
        let next = two_thirty.next_after(&spring).unwrap();
        assert_eq!(next.date_naive().to_string(), "2026-03-09");

        let fall: DateTime<_> = New_York.with_ymd_and_hms(2026, 11, 1, 0, 59, 0).unwrap();
        let one_thirty = LocalSchedule::parse("30 1 * * *").unwrap();
        let first = one_thirty.next_after(&fall).unwrap();
        let second = one_thirty.next_after(&first).unwrap();
        assert_eq!(first.hour(), 1);
        assert_eq!(second.hour(), 1);
        assert_eq!(first.date_naive(), second.date_naive());
        assert_ne!(first.offset(), second.offset());
        assert!(second > first);
    }
}
