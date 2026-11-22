//! The business's calendar: where midnight is.
//!
//! A billing period is a calendar month, an invoice falls due on a day, and
//! it is overdue when that day is over. Each of those needs to know where a
//! day begins, and there are three candidates: UTC, the zone of whichever
//! browser happens to be open, and the zone the ISP does business in. Only
//! the last one is the same for every operator and every script, and only
//! the last one is the day the customer was told.
//!
//! Getting it wrong is not cosmetic. With month boundaries in UTC and a plan
//! that "starts on the 1st" at local midnight, an ISP three hours east of
//! UTC bills three hours of the month *before* the service began, and one
//! four hours west under-charges the first month — both on an invoice that
//! is then frozen and numbered.
//!
//! So there is one calendar, it is a setting (`utc_offset_minutes` in the
//! `isp` section), and everything in this crate that asks "which day?" asks
//! here. It is a **fixed offset** from UTC, not a named zone: daylight-saving
//! rules are not modelled. A business that moves its clocks either keeps its
//! books on standard time all year or changes the setting twice a year — and
//! an invoice remembers the offset it was written under
//! ([`crate::model::Invoice::utc_offset_minutes`]), so changing the setting
//! moves no date on a document that already exists.

use chrono::{DateTime, Datelike, NaiveDate};
use nocturne_app::{AppError, AppResult};

use crate::model::Settings;

pub const DAY_MS: i64 = 86_400_000;
const MINUTE_MS: i64 = 60_000;

/// UTC−12:00, the westernmost offset anybody keeps.
pub const MIN_OFFSET_MINUTES: i32 = -720;
/// UTC+14:00, the easternmost.
pub const MAX_OFFSET_MINUTES: i32 = 840;

/// The furthest two valid calendars can disagree about where a day begins.
/// A month named under one offset and the same month named under another are
/// never further apart than this.
pub const WIDEST_SHIFT_MS: i64 = (MAX_OFFSET_MINUTES - MIN_OFFSET_MINUTES) as i64 * MINUTE_MS;

/// A fixed offset from UTC, east positive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Calendar {
    offset_minutes: i32,
}

impl Calendar {
    /// The business's calendar as the settings have it.
    ///
    /// `settings.set` is the core's and takes any value, so the range is
    /// checked here, where the value is used. An offset outside it is
    /// refused rather than clamped: a calendar that was guessed would move
    /// every month boundary and every due day without anybody being told.
    pub fn of(settings: &Settings) -> AppResult<Calendar> {
        let minutes = settings.utc_offset_minutes;
        if !(MIN_OFFSET_MINUTES..=MAX_OFFSET_MINUTES).contains(&minutes) {
            return Err(AppError::invalid(format!(
                "the `isp` setting `utc_offset_minutes` is {minutes}, and it has to be between \
                 {MIN_OFFSET_MINUTES} and {MAX_OFFSET_MINUTES} (UTC-12:00 to UTC+14:00): it is \
                 how far the business's clock is ahead of UTC, in minutes. Nothing is billed, \
                 aged or called overdue until it is put right, because every one of those depends \
                 on where a day begins."
            )));
        }
        Ok(Calendar {
            offset_minutes: minutes,
        })
    }

    /// The calendar an existing document was written in, from the offset it
    /// recorded. That offset was checked when it was written; this does not
    /// refuse it again, because a document that cannot be read is worse than
    /// one read exactly as it was stored.
    pub fn stored(offset_minutes: i32) -> Calendar {
        Calendar { offset_minutes }
    }

    pub fn offset_minutes(&self) -> i32 {
        self.offset_minutes
    }

    fn offset_ms(&self) -> i64 {
        i64::from(self.offset_minutes) * MINUTE_MS
    }

    /// The moment as this calendar's wall clock reads it, in milliseconds
    /// since its own epoch. Two documents written under different offsets
    /// are compared on this scale, where "October" is the same span for
    /// both.
    pub fn local(&self, at: i64) -> i64 {
        at.saturating_add(self.offset_ms())
    }

    /// Which day a moment falls on, counted in whole days from 1 January
    /// 1970 in this calendar. Two moments are on the same day exactly when
    /// this is equal.
    pub fn day(&self, at: i64) -> i64 {
        self.local(at).div_euclid(DAY_MS)
    }

    /// The date a moment falls on, or `None` for a moment no calendar has a
    /// year for.
    pub fn date(&self, at: i64) -> Option<NaiveDate> {
        DateTime::from_timestamp_millis(self.local(at)).map(|moment| moment.date_naive())
    }

    /// The year an invoice or credit note number is counted in.
    pub fn year(&self, at: i64) -> AppResult<i32> {
        self.date(at).map(|date| date.year()).ok_or_else(|| {
            AppError::invalid(format!(
                "{at} milliseconds since the epoch is outside any calendar year"
            ))
        })
    }

    /// A date for a sentence: `2025-10-01`.
    pub fn date_text(&self, at: i64) -> String {
        match self.date(at) {
            Some(date) => date.format("%Y-%m-%d").to_string(),
            None => format!("{at} ms"),
        }
    }

    /// `UTC+03:00`.
    pub fn name(&self) -> String {
        let minutes = self.offset_minutes.unsigned_abs();
        format!(
            "UTC{}{:02}:{:02}",
            if self.offset_minutes < 0 { '-' } else { '+' },
            minutes / 60,
            minutes % 60
        )
    }

    /// The month `2025-10` as `[start, end)`: from the first moment of its
    /// first day in this calendar to the first moment of the next month's.
    ///
    /// This is what a billing period is. A caller who names the month gets
    /// the same two instants whatever zone their own clock is in.
    pub fn month(&self, text: &str) -> AppResult<(i64, i64)> {
        let bad = || {
            AppError::invalid(format!(
                "`{}` is not a month: write the year and the month as `2025-10`",
                text.trim()
            ))
        };
        let (year_text, month_text) = text.trim().split_once('-').ok_or_else(bad)?;
        let all_digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
        if year_text.len() != 4
            || month_text.len() != 2
            || !all_digits(year_text)
            || !all_digits(month_text)
        {
            return Err(bad());
        }
        let year: i32 = year_text.parse().map_err(|_| bad())?;
        let month: u32 = month_text.parse().map_err(|_| bad())?;
        let (next_year, next_month) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
        let start = self.first_moment(year, month).ok_or_else(bad)?;
        let end = self.first_moment(next_year, next_month).ok_or_else(bad)?;
        Ok((start, end))
    }

    /// The first moment of the first day of a month.
    fn first_moment(&self, year: i32, month: u32) -> Option<i64> {
        let midnight = NaiveDate::from_ymd_opt(year, month, 1)?.and_hms_opt(0, 0, 0)?;
        Some(midnight.and_utc().timestamp_millis() - self.offset_ms())
    }

    /// How many days past its due day something is — the one answer to "is
    /// this overdue?", for the invoice, the arrears report and the summary
    /// alike. `None` until the due day is over.
    ///
    /// A due date is a day, not an instant: an invoice due on the 15th is
    /// not late at any time on the 15th, and is one day late throughout the
    /// 16th. Which day `due_at` is was settled by the calendar the invoice
    /// was `written_in`, and stays settled; what today is, is this
    /// calendar's — the business's, as it is set now.
    pub fn days_overdue(&self, due_at: i64, written_in: Calendar, now: i64) -> Option<i64> {
        let days = self.day(now) - written_in.day(due_at);
        (days > 0).then_some(days)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> i64 {
        DateTime::parse_from_rfc3339(text)
            .expect("a moment")
            .timestamp_millis()
    }

    fn offset(minutes: i32) -> Calendar {
        Calendar::of(&Settings {
            utc_offset_minutes: minutes,
            ..Settings::default()
        })
        .expect("an offset in range")
    }

    #[test]
    fn a_month_begins_at_the_business_s_midnight_wherever_that_is() {
        assert_eq!(
            offset(0).month("2025-10").expect("a month"),
            (at("2025-10-01T00:00:00Z"), at("2025-11-01T00:00:00Z"))
        );
        // Baghdad's October begins while UTC is still in September.
        assert_eq!(
            offset(180).month("2025-10").expect("a month"),
            (at("2025-09-30T21:00:00Z"), at("2025-10-31T21:00:00Z"))
        );
        // New York's begins four hours into UTC's.
        assert_eq!(
            offset(-240).month("2025-10").expect("a month"),
            (at("2025-10-01T04:00:00Z"), at("2025-11-01T04:00:00Z"))
        );
        // Offsets are not all whole hours, and December runs into January.
        assert_eq!(
            offset(345).month("2025-12").expect("a month"),
            (
                at("2025-12-01T00:00:00+05:45"),
                at("2026-01-01T00:00:00+05:45")
            )
        );
        // February is as long as that year's February.
        let (start, end) = offset(0).month("2024-02").expect("a month");
        assert_eq!((end - start) / DAY_MS, 29);
        for nonsense in [
            "2025-13",
            "2025-00",
            "2025-1",
            "25-10",
            "October",
            "",
            "2025-10-01",
        ] {
            assert_eq!(
                offset(0).month(nonsense).expect_err(nonsense).code,
                "invalid"
            );
        }
    }

    #[test]
    fn an_offset_outside_the_range_is_refused_and_the_edges_are_not() {
        for minutes in [MIN_OFFSET_MINUTES, 0, 345, MAX_OFFSET_MINUTES] {
            assert_eq!(offset(minutes).offset_minutes(), minutes);
        }
        for minutes in [MIN_OFFSET_MINUTES - 1, MAX_OFFSET_MINUTES + 1, i32::MAX] {
            let err = Calendar::of(&Settings {
                utc_offset_minutes: minutes,
                ..Settings::default()
            })
            .expect_err("out of range");
            assert_eq!(err.code, "invalid");
            assert!(
                err.message.contains("utc_offset_minutes"),
                "{}",
                err.message
            );
        }
        assert_eq!(offset(180).name(), "UTC+03:00");
        assert_eq!(offset(-570).name(), "UTC-09:30");
    }

    #[test]
    fn a_due_day_is_over_at_the_business_s_midnight_and_not_before() {
        let baghdad = offset(180);
        let due = at("2025-10-15T09:00:00+03:00");
        // Any time on the 15th, it is not late — including the hours of the
        // 15th that UTC already calls the 15th and those it still calls the 14th.
        for now in [
            "2025-10-15T00:00:00+03:00",
            "2025-10-15T09:00:01+03:00",
            "2025-10-15T23:59:59+03:00",
        ] {
            assert_eq!(baghdad.days_overdue(due, baghdad, at(now)), None, "{now}");
        }
        assert_eq!(
            baghdad.days_overdue(due, baghdad, at("2025-10-16T00:00:00+03:00")),
            Some(1)
        );
        assert_eq!(
            baghdad.days_overdue(due, baghdad, at("2025-10-16T23:59:59+03:00")),
            Some(1)
        );
        assert_eq!(
            baghdad.days_overdue(due, baghdad, at("2025-11-14T12:00:00+03:00")),
            Some(30)
        );
        // In UTC the same instant of the 16th in Baghdad is still the 15th.
        assert_eq!(
            offset(0).days_overdue(due, offset(0), at("2025-10-16T00:00:00+03:00")),
            None
        );
    }

    #[test]
    fn a_due_day_stays_the_day_it_was_written_as_when_the_calendar_is_changed() {
        // Due on the 15th, typed as the first moment of the 15th in Baghdad —
        // which UTC calls nine in the evening of the 14th.
        let written = offset(180);
        let due = at("2025-10-15T00:00:00+03:00");
        let today = offset(0);
        // The business moves its calendar to UTC. Read in the new calendar
        // alone the invoice would have fallen due on the 14th and be a day
        // late throughout the 15th. It was due on the 15th and still is.
        assert_eq!(today.day(due) + 1, written.day(due));
        assert_eq!(
            today.days_overdue(due, written, at("2025-10-15T12:00:00Z")),
            None
        );
        assert_eq!(
            today.days_overdue(due, written, at("2025-10-16T00:00:00Z")),
            Some(1)
        );
    }

    #[test]
    fn a_moment_no_calendar_reaches_is_an_answer_and_not_a_panic() {
        let calendar = offset(840);
        assert_eq!(
            calendar.year(at("2025-12-31T10:00:00Z")).expect("a year"),
            2026
        );
        assert_eq!(
            offset(-720)
                .year(at("2026-01-01T11:59:59Z"))
                .expect("a year"),
            2025
        );
        assert_eq!(
            calendar.year(i64::MAX).expect_err("no such year").code,
            "invalid"
        );
        assert_eq!(
            calendar.year(i64::MIN).expect_err("no such year").code,
            "invalid"
        );
        // Day arithmetic saturates rather than wrapping round.
        assert!(calendar.day(i64::MAX) > 0);
        assert!(offset(-720).day(i64::MIN) < 0);
        assert_eq!(
            calendar.date_text(at("2025-10-01T00:00:00+14:00")),
            "2025-10-01"
        );
    }
}
