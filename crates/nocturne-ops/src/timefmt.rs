//! Times and durations as people read them.
//!
//! A document shows every time in one fixed UTC offset and says which in its
//! header: the offset the caller asks for (the UI passes the browser's), or
//! the offset of the machine Nocturne runs on.

use chrono::{DateTime, FixedOffset, Local, Offset, SecondsFormat, Utc};
use nocturne_app::{AppError, AppResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zone {
    offset: FixedOffset,
}

impl Zone {
    /// The zone for `utc_offset_minutes` (east of UTC), or this machine's
    /// current one when the caller did not say.
    pub fn resolve(utc_offset_minutes: Option<i32>) -> AppResult<Zone> {
        let Some(minutes) = utc_offset_minutes else {
            return Ok(Zone {
                offset: Local::now().offset().fix(),
            });
        };
        Some(minutes)
            .filter(|minutes| (-14 * 60..=14 * 60).contains(minutes))
            .and_then(|minutes| FixedOffset::east_opt(minutes * 60))
            .map(|offset| Zone { offset })
            .ok_or_else(|| {
                AppError::invalid(format!(
                    "`utc_offset_minutes` must be between -840 and 840 (UTC-14:00 to UTC+14:00), not {minutes}"
                ))
            })
    }

    fn at(&self, ms: i64) -> Option<DateTime<FixedOffset>> {
        DateTime::<Utc>::from_timestamp_millis(ms).map(|utc| utc.with_timezone(&self.offset))
    }

    /// `UTC`, `UTC+03:00`, `UTC-05:30`.
    pub fn label(&self) -> String {
        let seconds = self.offset.local_minus_utc();
        if seconds == 0 {
            return "UTC".into();
        }
        let sign = if seconds < 0 { '-' } else { '+' };
        let minutes = seconds.abs() / 60;
        format!("UTC{sign}{:02}:{:02}", minutes / 60, minutes % 60)
    }

    /// `2026-11-13 14:05`.
    pub fn datetime(&self, ms: i64) -> String {
        match self.at(ms) {
            Some(time) => time.format("%Y-%m-%d %H:%M").to_string(),
            None => format!("{ms} ms"),
        }
    }

    /// `2026-11-13T14:05:09+03:00`, for machine-read output.
    pub fn rfc3339(&self, ms: i64) -> String {
        match self.at(ms) {
            Some(time) => time.to_rfc3339_opts(SecondsFormat::Secs, false),
            None => format!("{ms} ms"),
        }
    }

    /// `20261113-1405`, for file names.
    pub fn stamp(&self, ms: i64) -> String {
        match self.at(ms) {
            Some(time) => time.format("%Y%m%d-%H%M").to_string(),
            None => ms.to_string(),
        }
    }
}

/// A duration in its two largest units: `45 s`, `1 min 30 s`, `1 h 12 min`, `2 d 3 h`.
pub fn duration(ms: i64) -> String {
    // Saturating: a gap between two times another feature sent can be as
    // long as the number line, and a document is not to fall over it.
    let seconds = ms.max(0).saturating_add(500) / 1000;
    let (days, hours, minutes, secs) = (
        seconds / 86_400,
        seconds % 86_400 / 3_600,
        seconds % 3_600 / 60,
        seconds % 60,
    );
    let parts = [(days, "d"), (hours, "h"), (minutes, "min"), (secs, "s")];
    let Some(first) = parts.iter().position(|(value, _)| *value > 0) else {
        return "0 s".into();
    };
    parts[first..]
        .iter()
        .take(2)
        .filter(|(value, _)| *value > 0)
        .map(|(value, unit)| format!("{value} {unit}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    // 2026-11-13 14:05:09 UTC, worked out by hand: 20 770 days after the epoch, plus 14 h 5 min 9 s.
    const AT: i64 = (20_770 * 86_400 + 14 * 3_600 + 5 * 60 + 9) * 1000;

    #[test]
    fn formats_in_the_requested_offset() {
        let utc = Zone::resolve(Some(0)).unwrap();
        assert_eq!(utc.label(), "UTC");
        assert_eq!(utc.datetime(AT), "2026-11-13 14:05");
        assert_eq!(utc.rfc3339(AT), "2026-11-13T14:05:09+00:00");
        assert_eq!(utc.stamp(AT), "20261113-1405");

        let riyadh = Zone::resolve(Some(180)).unwrap();
        assert_eq!(riyadh.label(), "UTC+03:00");
        assert_eq!(riyadh.datetime(AT), "2026-11-13 17:05");
        assert_eq!(riyadh.rfc3339(AT), "2026-11-13T17:05:09+03:00");

        // Half-hour offsets, and a day boundary crossed backwards.
        let newfoundland = Zone::resolve(Some(-210)).unwrap();
        assert_eq!(newfoundland.label(), "UTC-03:30");
        assert_eq!(newfoundland.datetime(AT), "2026-11-13 10:35");
        let baker = Zone::resolve(Some(-12 * 60)).unwrap();
        assert_eq!(baker.datetime(AT - 3 * 3_600_000), "2026-11-12 23:05");
    }

    #[test]
    fn refuses_impossible_offsets_and_defaults_to_the_local_zone() {
        let err = Zone::resolve(Some(900)).unwrap_err();
        assert_eq!(err.code, "invalid");
        assert!(err.message.contains("900"), "{}", err.message);
        assert!(Zone::resolve(Some(-841)).is_err());
        // Whatever this machine's zone is, it formats and labels itself.
        let local = Zone::resolve(None).unwrap();
        assert!(local.label().starts_with("UTC"));
        assert_eq!(local.datetime(AT).len(), 16);
    }

    #[test]
    fn durations_use_the_two_largest_units() {
        assert_eq!(duration(0), "0 s");
        assert_eq!(duration(499), "0 s");
        assert_eq!(duration(45_000), "45 s");
        assert_eq!(duration(60_000), "1 min");
        assert_eq!(duration(90_000), "1 min 30 s");
        assert_eq!(duration(3_600_000), "1 h");
        assert_eq!(duration(4_320_000), "1 h 12 min");
        assert_eq!(duration(3_601_000), "1 h");
        assert_eq!(duration(183_600_000), "2 d 3 h");
        assert_eq!(duration(-5), "0 s");
        // The longest there is, and the shortest: neither is a reason to stop.
        assert_eq!(duration(i64::MAX), "106751991167 d 7 h");
        assert_eq!(duration(i64::MIN), "0 s");
    }
}
