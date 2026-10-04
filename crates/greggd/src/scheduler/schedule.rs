//! Strict five-field cron schedules in local civil time.

use chrono::{DateTime, Datelike, NaiveDate, TimeZone};

/// Maximum UTF-8 byte length accepted for a cron expression.
pub(crate) const MAX_SCHEDULE_BYTES: usize = 128;
const MAX_SEARCH_DAYS: usize = 146_097; // One Gregorian 400-year cycle.

/// Parsed schedule fields. Each mask uses the field's natural numeric value.
#[derive(Debug, Clone)]
pub(crate) struct LocalSchedule {
    minutes: u64,
    hours: u64,
    month_days: u64,
    months: u64,
    week_days: u64,
    month_day_wildcard: bool,
    week_day_wildcard: bool,
}

impl LocalSchedule {
    /// Parse ordinary five-field numeric cron syntax and supported aliases.
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

        let (minutes, _) = parse_field(fields[0], 0, 59)?;
        let (hours, _) = parse_field(fields[1], 0, 23)?;
        let (month_days, month_day_wildcard) = parse_field(fields[2], 1, 31)?;
        let (months, _) = parse_field(fields[3], 1, 12)?;
        let (week_days, week_day_wildcard) = parse_field(fields[4], 0, 6)?;

        Ok(Self {
            minutes,
            hours,
            month_days,
            months,
            week_days,
            month_day_wildcard,
            week_day_wildcard,
        })
    }

    /// Return the next matching instant strictly after `after`.
    pub(crate) fn next_after<Tz: TimeZone>(
        &self,
        after: &DateTime<Tz>,
    ) -> Result<DateTime<Tz>, String> {
        let mut date = after.date_naive();
        for _ in 0..MAX_SEARCH_DAYS {
            if self.date_matches(date) {
                for hour in 0..24 {
                    if self.hours & (1 << hour) == 0 {
                        continue;
                    }
                    for minute in 0..60 {
                        if self.minutes & (1 << minute) == 0 {
                            continue;
                        }
                        let Some(civil) = date.and_hms_opt(hour, minute, 0) else {
                            continue;
                        };
                        match after.timezone().from_local_datetime(&civil) {
                            chrono::LocalResult::Single(candidate) if candidate > *after => {
                                return Ok(candidate);
                            }
                            chrono::LocalResult::Ambiguous(first, second) => {
                                if first > *after {
                                    return Ok(first);
                                }
                                if second > *after {
                                    return Ok(second);
                                }
                            }
                            chrono::LocalResult::Single(_) | chrono::LocalResult::None => {}
                        }
                    }
                }
            }
            date = date
                .succ_opt()
                .ok_or_else(|| "cron expression has no future occurrence".to_owned())?;
        }
        Err("cron expression has no future occurrence within 400 years".to_owned())
    }

    fn date_matches(&self, date: NaiveDate) -> bool {
        let month = date.month() as usize;
        let month_day = date.day() as usize;
        let week_day = date.weekday().num_days_from_sunday() as usize;
        let month_matches = self.months & (1 << month) != 0;
        let month_day_matches = self.month_days & (1 << month_day) != 0;
        let week_day_matches = self.week_days & (1 << week_day) != 0;
        let day_matches = match (self.month_day_wildcard, self.week_day_wildcard) {
            (true, true) => true,
            (true, false) => week_day_matches,
            (false, true) => month_day_matches,
            (false, false) => month_day_matches || week_day_matches,
        };
        month_matches && day_matches
    }
}

fn parse_field(value: &str, minimum: usize, maximum: usize) -> Result<(u64, bool), String> {
    let mut mask = 0_u64;
    let mut wildcard = false;
    for item in value.split(',') {
        if item.is_empty() {
            return Err("empty cron list item".to_owned());
        }
        let mut step_parts = item.split('/');
        let base = step_parts.next().unwrap_or_default();
        let step = match step_parts.next() {
            Some(value) => {
                let step = value
                    .parse::<usize>()
                    .map_err(|_| "cron step must be a positive integer".to_owned())?;
                if step == 0 {
                    return Err("cron step must be a positive integer".to_owned());
                }
                step
            }
            None => 1,
        };
        if step_parts.next().is_some() {
            return Err("cron field has more than one step separator".to_owned());
        }

        let (start, end, is_wildcard) = if base == "*" {
            (minimum, maximum, true)
        } else if let Some((start, end)) = base.split_once('-') {
            let start = parse_number(start, minimum, maximum)?;
            let end = parse_number(end, minimum, maximum)?;
            if start > end {
                return Err("cron ranges must be ascending".to_owned());
            }
            (start, end, false)
        } else {
            let start = parse_number(base, minimum, maximum)?;
            (
                start,
                if item.contains('/') { maximum } else { start },
                false,
            )
        };
        wildcard |= is_wildcard;
        for selected in (start..=end).step_by(step) {
            mask |= 1_u64 << selected;
        }
    }
    if mask == 0 {
        return Err("cron field selects no values".to_owned());
    }
    Ok((mask, wildcard))
}

fn parse_number(value: &str, minimum: usize, maximum: usize) -> Result<usize, String> {
    let number = value
        .parse::<usize>()
        .map_err(|_| "cron fields accept numeric values only".to_owned())?;
    if !(minimum..=maximum).contains(&number) {
        return Err(format!(
            "cron value {number} is outside {minimum}..={maximum}"
        ));
    }
    Ok(number)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, NaiveDate, TimeZone, Timelike, Utc};
    use chrono_tz::America::New_York;

    fn local(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
            .with_timezone(&Local)
    }

    #[test]
    fn five_field_contract_and_aliases() {
        for expression in [
            "* * * * *",
            "0 3 * * 0",
            "0 0 1 * *",
            "0,30 8-18/2 * * 1-5",
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
            "*/0 * * * *",
            "60 * * * *",
            "5-1 * * * *",
        ] {
            assert!(LocalSchedule::parse(expression).is_err(), "{expression}");
        }
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
        let stepped_wildcard = LocalSchedule::parse("0 0 */1 * 1").unwrap();
        assert_eq!(
            stepped_wildcard.next_after(&after).unwrap().date_naive(),
            NaiveDate::from_ymd_opt(2026, 7, 6).unwrap()
        );

        let instant = local(2026, 10, 3, 12, 0);
        assert!(every_minute.next_after(&instant).unwrap() > instant);
    }

    #[test]
    fn civil_time_skips_spring_gap_and_returns_both_fall_overlap_instants() {
        let spring = New_York.with_ymd_and_hms(2026, 3, 8, 1, 59, 0).unwrap();
        let two_thirty = LocalSchedule::parse("30 2 * * *").unwrap();
        let next = two_thirty.next_after(&spring).unwrap();
        assert_eq!(next.date_naive().to_string(), "2026-03-09");

        let fall = New_York.with_ymd_and_hms(2026, 11, 1, 0, 59, 0).unwrap();
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
