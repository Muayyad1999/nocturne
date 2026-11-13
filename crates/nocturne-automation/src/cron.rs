//! When a schedule next fires.
//!
//! Five-field cron and `daily` are **local time**, so "02:30" stays 02:30 when
//! the clocks change. The walk is over local wall-clock minutes, not over
//! absolute ones, which settles the two days of the year that are not 24 hours
//! long:
//!
//! * the hour that does not exist in spring is skipped — a 02:30 daily backup
//!   does not run that day, rather than running at 03:30;
//! * the hour that happens twice in autumn fires **once**, at its first
//!   occurrence, rather than twice.

use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike};

use crate::model::When;

#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    minute: Vec<u32>,
    hour: Vec<u32>,
    day_of_month: Vec<u32>,
    month: Vec<u32>,
    day_of_week: Vec<u32>,
    /// Vixie cron: when both the day of month and the day of week are
    /// restricted, a day matching **either** is a match.
    dom_restricted: bool,
    dow_restricted: bool,
}

const DAY_NAMES: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
const MONTH_NAMES: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

fn name_to_number(token: &str, names: &[&str], offset: u32) -> Option<u32> {
    names
        .iter()
        .position(|name| token.len() >= 3 && name.eq_ignore_ascii_case(&token[..3]))
        .map(|index| index as u32 + offset)
}

/// One cron field: `*`, `*/n`, `a`, `a-b`, `a-b/n`, or a comma-separated list.
fn field(
    text: &str,
    min: u32,
    max: u32,
    names: &[&str],
    name_offset: u32,
) -> Result<Vec<u32>, String> {
    let mut values = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(format!("`{text}` has an empty entry"));
        }
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (
                range,
                step.parse::<u32>()
                    .map_err(|_| format!("`{step}` is not a step"))?,
            ),
            None => (part, 1),
        };
        if step == 0 {
            return Err("a step of 0 goes nowhere".into());
        }
        let number = |token: &str| -> Result<u32, String> {
            let token = token.trim();
            token
                .parse::<u32>()
                .ok()
                .or_else(|| name_to_number(token, names, name_offset))
                .ok_or_else(|| format!("`{token}` is not a value here"))
        };
        let (from, to) = if range == "*" {
            (min, max)
        } else if let Some((from, to)) = range.split_once('-') {
            (number(from)?, number(to)?)
        } else {
            let single = number(range)?;
            (single, single)
        };
        if from > to {
            return Err(format!("`{range}` counts backwards"));
        }
        if to > max || from < min {
            return Err(format!("`{range}` is outside {min}–{max}"));
        }
        let mut value = from;
        while value <= to {
            values.push(value);
            value += step;
        }
    }
    values.sort_unstable();
    values.dedup();
    if values.is_empty() {
        return Err(format!("`{text}` matches nothing"));
    }
    Ok(values)
}

impl Spec {
    /// Parse a five-field expression: minute hour day-of-month month day-of-week.
    pub fn parse(expression: &str) -> Result<Spec, String> {
        let fields: Vec<&str> = expression.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(format!(
                "a cron expression has five fields (minute hour day-of-month month \
                 day-of-week); this one has {}",
                fields.len()
            ));
        }
        let mut day_of_week = field(fields[4], 0, 7, &DAY_NAMES, 0)?;
        // Both 0 and 7 mean Sunday.
        if day_of_week.contains(&7) {
            day_of_week.retain(|day| *day != 7);
            if !day_of_week.contains(&0) {
                day_of_week.push(0);
            }
            day_of_week.sort_unstable();
        }
        Ok(Spec {
            minute: field(fields[0], 0, 59, &[], 0)?,
            hour: field(fields[1], 0, 23, &[], 0)?,
            day_of_month: field(fields[2], 1, 31, &[], 0)?,
            month: field(fields[3], 1, 12, &MONTH_NAMES, 1)?,
            day_of_week,
            dom_restricted: fields[2].trim() != "*",
            dow_restricted: fields[4].trim() != "*",
        })
    }

    fn day_matches(&self, date: NaiveDate) -> bool {
        let dom = self.day_of_month.contains(&date.day());
        let dow = self
            .day_of_week
            .contains(&date.weekday().num_days_from_sunday());
        if !self.month.contains(&date.month()) {
            return false;
        }
        match (self.dom_restricted, self.dow_restricted) {
            (true, true) => dom || dow,
            (true, false) => dom,
            (false, true) => dow,
            (false, false) => true,
        }
    }

    fn time_matches(&self, at: NaiveDateTime) -> bool {
        self.minute.contains(&at.minute()) && self.hour.contains(&at.hour())
    }
}

/// Turn a `When` into a cron specification, or say why it is not one.
pub fn spec_of(when: &When) -> Result<Option<Spec>, String> {
    match when {
        When::Interval { every_s } => {
            if *every_s < 10 {
                return Err("an interval of less than 10 seconds is not a schedule".into());
            }
            Ok(None)
        }
        When::Cron { expr } => Spec::parse(expr).map(Some),
        When::Daily { at, days } => {
            let (hour, minute) = at
                .split_once(':')
                .ok_or_else(|| format!("`{at}` is not a time of day (use HH:MM)"))?;
            let hour: u32 = hour
                .trim()
                .parse()
                .map_err(|_| format!("`{at}` is not a time of day (use HH:MM)"))?;
            let minute: u32 = minute
                .trim()
                .parse()
                .map_err(|_| format!("`{at}` is not a time of day (use HH:MM)"))?;
            if hour > 23 || minute > 59 {
                return Err(format!("`{at}` is not a time of day"));
            }
            let day_of_week = if days.is_empty() {
                (0..7).collect()
            } else {
                let mut numbers = Vec::new();
                for day in days {
                    let number = name_to_number(day.trim(), &DAY_NAMES, 0)
                        .ok_or_else(|| format!("`{day}` is not a day of the week"))?;
                    numbers.push(number);
                }
                numbers.sort_unstable();
                numbers.dedup();
                numbers
            };
            Ok(Some(Spec {
                minute: vec![minute],
                hour: vec![hour],
                day_of_month: (1..=31).collect(),
                month: (1..=12).collect(),
                dom_restricted: false,
                dow_restricted: !days.is_empty(),
                day_of_week,
            }))
        }
    }
}

/// Validate a `When` without computing anything.
pub fn check(when: &When) -> Result<(), String> {
    spec_of(when).map(|_| ())
}

/// The first time at or after `after_ms` (exclusive) that `when` fires.
///
/// `last_run_ms` only matters for an interval, which counts from the last run
/// rather than from the clock.
pub fn next_after(when: &When, after_ms: i64, last_run_ms: Option<i64>) -> Option<i64> {
    match when {
        When::Interval { every_s } => {
            let every = (*every_s).max(10) as i64 * 1000;
            Some(last_run_ms.unwrap_or(after_ms).saturating_add(every))
        }
        _ => {
            let spec = spec_of(when).ok().flatten()?;
            next_cron(&spec, after_ms)
        }
    }
}

/// Walk local wall-clock minutes until one matches and exists.
fn next_cron(spec: &Spec, after_ms: i64) -> Option<i64> {
    let after = Local.timestamp_millis_opt(after_ms).single()?;
    let mut candidate = after
        .naive_local()
        .with_second(0)?
        .with_nanosecond(0)?
        .checked_add_signed(Duration::minutes(1))?;

    // A year of minutes is the bound; the day-level skip keeps all but the
    // last day out of the inner loop.
    for _ in 0..(366 * 24 * 60) {
        if !spec.day_matches(candidate.date()) {
            candidate = candidate
                .date()
                .checked_add_signed(Duration::days(1))?
                .and_hms_opt(0, 0, 0)?;
            continue;
        }
        if spec.time_matches(candidate) {
            match Local.from_local_datetime(&candidate) {
                // Unambiguous, or the first of two: fire once.
                chrono::LocalResult::Single(at) | chrono::LocalResult::Ambiguous(at, _) => {
                    let at = at.timestamp_millis();
                    if at > after_ms {
                        return Some(at);
                    }
                }
                // The clocks went forward over this minute: it never happens.
                chrono::LocalResult::None => {}
            }
        }
        candidate = candidate.checked_add_signed(Duration::minutes(1))?;
    }
    None
}

/// The next `count` times, for a preview.
pub fn preview(when: &When, from_ms: i64, count: usize) -> Vec<i64> {
    let mut times = Vec::new();
    let mut cursor = from_ms;
    for _ in 0..count.min(50) {
        // An interval previewed from "now" repeats from the previous entry.
        let next = match when {
            When::Interval { every_s } => cursor.saturating_add((*every_s).max(10) as i64 * 1000),
            _ => match next_after(when, cursor, None) {
                Some(next) => next,
                None => break,
            },
        };
        times.push(next);
        cursor = next;
    }
    times
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(text)
            .expect("a timestamp")
            .timestamp_millis()
    }

    fn local(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("an unambiguous local time")
            .timestamp_millis()
    }

    #[test]
    fn a_five_field_expression_is_read_the_way_cron_reads_it() {
        let spec = Spec::parse("*/15 2-4 * * mon-fri").unwrap();
        assert_eq!(spec.minute, vec![0, 15, 30, 45]);
        assert_eq!(spec.hour, vec![2, 3, 4]);
        assert_eq!(spec.day_of_week, vec![1, 2, 3, 4, 5]);
        assert!(spec.dow_restricted && !spec.dom_restricted);
        assert_eq!(Spec::parse("0 0 * * 7").unwrap().day_of_week, vec![0]);
        assert_eq!(Spec::parse("0 0 1 jan *").unwrap().month, vec![1]);
    }

    #[test]
    fn a_broken_expression_says_what_is_wrong_with_it() {
        for (expression, wanted) in [
            ("* * * *", "five fields"),
            ("60 * * * *", "outside"),
            ("5-1 * * * *", "backwards"),
            ("*/0 * * * *", "step of 0"),
            ("x * * * *", "not a value"),
        ] {
            let err = Spec::parse(expression).unwrap_err();
            assert!(err.contains(wanted), "{expression}: {err}");
        }
    }

    #[test]
    fn the_next_run_is_the_next_matching_local_minute() {
        // 2024-06-10 is a Monday.
        let from = local(2024, 6, 10, 1, 59);
        let next = next_after(
            &When::Cron {
                expr: "30 2 * * *".into(),
            },
            from,
            None,
        )
        .unwrap();
        assert_eq!(next, local(2024, 6, 10, 2, 30));
        // Already past today: tomorrow.
        let next = next_after(
            &When::Cron {
                expr: "30 2 * * *".into(),
            },
            local(2024, 6, 10, 3, 0),
            None,
        )
        .unwrap();
        assert_eq!(next, local(2024, 6, 11, 2, 30));
    }

    #[test]
    fn daily_with_days_only_fires_on_those_days() {
        // Saturday 2024-06-08 → the next weekday run is Monday.
        let when = When::Daily {
            at: "07:05".into(),
            days: vec!["mon".into(), "wed".into()],
        };
        let next = next_after(&when, local(2024, 6, 8, 12, 0), None).unwrap();
        assert_eq!(next, local(2024, 6, 10, 7, 5));
        let after_monday = next_after(&when, next, None).unwrap();
        assert_eq!(after_monday, local(2024, 6, 12, 7, 5));
    }

    #[test]
    fn a_month_end_schedule_skips_the_months_that_have_no_such_day() {
        let when = When::Cron {
            expr: "0 3 31 * *".into(),
        };
        let next = next_after(&when, local(2024, 1, 31, 4, 0), None).unwrap();
        // February and April have no 31st; March does.
        assert_eq!(next, local(2024, 3, 31, 3, 0));
    }

    #[test]
    fn the_29th_of_february_waits_for_a_leap_year() {
        let when = When::Cron {
            expr: "0 1 29 2 *".into(),
        };
        let next = next_after(&when, local(2025, 3, 1, 0, 0), None).unwrap();
        assert_eq!(next, local(2028, 2, 29, 1, 0));
    }

    #[test]
    fn an_interval_counts_from_the_last_run_not_from_the_clock() {
        let when = When::Interval { every_s: 900 };
        let now = at("2024-06-10T10:00:00Z");
        let last = at("2024-06-10T09:50:00Z");
        assert_eq!(
            next_after(&when, now, Some(last)),
            Some(at("2024-06-10T10:05:00Z"))
        );
        assert_eq!(
            next_after(&when, now, None),
            Some(at("2024-06-10T10:15:00Z"))
        );
        assert!(check(&When::Interval { every_s: 1 }).is_err());
    }

    /// Daylight saving, in a zone that has it. On a machine whose zone does
    /// not (UTC on CI), the walk is still the walk, so the test asserts the
    /// property that holds either way: consecutive runs of a daily schedule
    /// are one calendar day apart in *local* time, never two, and never the
    /// same local time twice.
    #[test]
    fn a_daily_schedule_keeps_its_local_time_across_a_clock_change() {
        let when = When::Daily {
            at: "02:30".into(),
            days: Vec::new(),
        };
        // March and October, where Europe and North America change.
        for (year, month, day) in [(2024, 3, 8), (2024, 10, 25)] {
            let mut cursor = Local
                .with_ymd_and_hms(year, month, day, 0, 0, 0)
                .single()
                .expect("midnight exists")
                .timestamp_millis();
            let mut seen_local = Vec::new();
            for _ in 0..8 {
                let next = next_after(&when, cursor, None).expect("a next run");
                let local_time = Local.timestamp_millis_opt(next).single().expect("local");
                // Either it is 02:30, or that minute did not exist that day
                // and the run was skipped — never 01:30 or 03:30 on a normal day.
                assert_eq!(
                    (local_time.hour(), local_time.minute()),
                    (2, 30),
                    "{local_time} is not the scheduled time of day"
                );
                assert!(
                    !seen_local.contains(&local_time.date_naive()),
                    "{local_time} fires twice on the same local day"
                );
                seen_local.push(local_time.date_naive());
                cursor = next;
            }
        }
    }

    #[test]
    fn a_preview_lists_the_next_few_runs_in_order() {
        let from = local(2024, 6, 10, 0, 0);
        let times = preview(
            &When::Cron {
                expr: "0 */6 * * *".into(),
            },
            from,
            5,
        );
        assert_eq!(times.len(), 5);
        assert!(times.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(times[0], local(2024, 6, 10, 6, 0));
        let interval = preview(&When::Interval { every_s: 60 }, from, 3);
        assert_eq!(
            interval,
            vec![from + 60_000, from + 120_000, from + 180_000]
        );
    }
}
