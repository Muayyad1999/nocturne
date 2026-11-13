//! Device clocks and the many ways a CLI prints durations.
//!
//! A device that was never told the time starts at 00:00:00 UTC, 1 March 1993,
//! the classic value of an unset real-time clock.

use crate::api::SimTime;

/// Unix seconds of 1993-03-01 00:00:00 UTC (a Monday).
pub(crate) const EPOCH_1993: i64 = 730_944_000;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DateTime {
    pub(crate) year: i64,
    /// 1..=12
    pub(crate) month: u32,
    pub(crate) day: u32,
    pub(crate) hour: u32,
    pub(crate) min: u32,
    pub(crate) sec: u32,
    pub(crate) ms: u32,
    /// 0 = Sunday
    pub(crate) weekday: u32,
}

impl DateTime {
    pub(crate) fn from_unix_ms(ms: i64) -> DateTime {
        let secs = ms.div_euclid(1000);
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        DateTime {
            year,
            month,
            day,
            hour: (rem / 3600) as u32,
            min: (rem % 3600 / 60) as u32,
            sec: (rem % 60) as u32,
            ms: ms.rem_euclid(1000) as u32,
            weekday: (days + 4).rem_euclid(7) as u32,
        }
    }

    pub(crate) fn month_name(&self) -> &'static str {
        MONTHS[(self.month as usize).saturating_sub(1).min(11)]
    }

    pub(crate) fn weekday_name(&self) -> &'static str {
        DAYS[(self.weekday as usize).min(6)]
    }

    /// `Mar  1 00:02:11.003` / `Mar  1 00:02:11`, the syslog timestamp body.
    pub(crate) fn log_stamp(&self, msec: bool) -> String {
        let base = format!(
            "{} {:>2} {:02}:{:02}:{:02}",
            self.month_name(),
            self.day,
            self.hour,
            self.min,
            self.sec
        );
        if msec {
            format!("{base}.{:03}", self.ms)
        } else {
            base
        }
    }

    /// `00:02:11.003 UTC Mon Mar 1 1993`, the `show clock` body.
    pub(crate) fn show_clock(&self, zone: &str) -> String {
        format!(
            "{:02}:{:02}:{:02}.{:03} {} {} {} {} {}",
            self.hour,
            self.min,
            self.sec,
            self.ms,
            zone,
            self.weekday_name(),
            self.month_name(),
            self.day,
            self.year
        )
    }
}

/// Month number for an (abbreviated) English month name.
pub(crate) fn month_number(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    (lower.len() >= 3)
        .then(|| {
            MONTHS
                .iter()
                .position(|m| lower.starts_with(&m.to_ascii_lowercase()))
        })
        .flatten()
        .map(|i| i as u32 + 1)
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// (year, month, day) → days since 1970-01-01, or `None` for an impossible date.
pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || day == 0 || !(1970..=2099).contains(&year) {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from(if month > 2 { month - 3 } else { month + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    // Reject day overflow (e.g. 31 February) by converting back.
    (civil_from_days(days) == (year, month, day)).then_some(days)
}

/// `2 hours, 14 minutes` / `1 week, 3 days, 2 hours, 14 minutes`, as in `show version`.
pub(crate) fn uptime_long(micros: SimTime) -> String {
    let mins = micros / 60_000_000;
    let (weeks, days, hours, minutes) = (mins / 10_080, mins / 1440 % 7, mins / 60 % 24, mins % 60);
    let mut parts = Vec::new();
    let mut push = |n: u64, unit: &str| {
        if n > 0 {
            parts.push(format!("{n} {unit}{}", if n == 1 { "" } else { "s" }));
        }
    };
    push(weeks, "week");
    push(days, "day");
    push(hours, "hour");
    if minutes > 0 || parts.is_empty() {
        parts.push(format!(
            "{minutes} minute{}",
            if minutes == 1 { "" } else { "s" }
        ));
    }
    parts.join(", ")
}

/// `00:01:23`, then `1d02h`, then `2w3d`: the compact age used in route and
/// neighbour tables.
pub(crate) fn age_compact(micros: SimTime) -> String {
    let secs = micros / 1_000_000;
    if secs < 86_400 {
        hms(secs)
    } else if secs < 604_800 {
        format!("{}d{:02}h", secs / 86_400, secs % 86_400 / 3600)
    } else {
        format!("{}w{}d", secs / 604_800, secs % 604_800 / 86_400)
    }
}

/// `hh:mm:ss`.
pub(crate) fn hms(secs: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_monday_first_of_march() {
        let d = DateTime::from_unix_ms(EPOCH_1993 * 1000 + 131_003);
        assert_eq!(
            (d.year, d.month, d.day, d.weekday_name()),
            (1993, 3, 1, "Mon")
        );
        assert_eq!(d.log_stamp(true), "Mar  1 00:02:11.003");
        assert_eq!(d.log_stamp(false), "Mar  1 00:02:11");
        assert_eq!(d.show_clock("UTC"), "00:02:11.003 UTC Mon Mar 1 1993");
    }

    #[test]
    fn civil_round_trip() {
        for (y, m, d) in [(1970, 1, 1), (2000, 2, 29), (2024, 12, 31), (2026, 11, 13)] {
            let days = days_from_civil(y, m, d).unwrap();
            assert_eq!(civil_from_days(days), (y, m, d));
        }
        assert_eq!(days_from_civil(2023, 2, 29), None);
        assert_eq!(days_from_civil(2023, 13, 1), None);
        assert_eq!(month_number("march"), Some(3));
        assert_eq!(month_number("Ma"), None);
    }

    #[test]
    fn durations() {
        assert_eq!(uptime_long(0), "0 minutes");
        assert_eq!(uptime_long(134 * 60_000_000), "2 hours, 14 minutes");
        assert_eq!(uptime_long(60 * 60_000_000), "1 hour");
        assert_eq!(
            uptime_long((10_080 + 1441) * 60_000_000),
            "1 week, 1 day, 1 minute"
        );
        assert_eq!(age_compact(83_000_000), "00:01:23");
        assert_eq!(age_compact(93_600_000_000), "1d02h");
        assert_eq!(age_compact(1_468_800_000_000), "2w3d");
    }
}
