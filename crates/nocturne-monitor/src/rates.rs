//! Turning counters into rates.
//!
//! A device reports interface traffic as counters that only ever go up — until
//! they run off the end of their 32 or 64 bits and start again, or until the
//! device reboots and they start from zero. Both look identical from outside:
//! a number that went down. The one thing that tells them apart is the
//! device's own uptime, which is why every rate here is a counter delta over
//! an *uptime* delta rather than over wall-clock time.
//!
//! The rules:
//!
//! * uptime went **forwards** → the interval is real. A 32-bit counter that
//!   went down wrapped, and the delta is taken modulo 2^32. A 64-bit counter
//!   that went down wrapped only if it had actually reached the top of its
//!   range; otherwise, and always for a counter read off text, it was
//!   started again on the device (`clear counters`, a re-seated line card)
//!   and there is no delta at all — see [`delta`];
//! * uptime went **backwards** → the device restarted; the interval is
//!   thrown away and the new readings become the baseline. A reboot must
//!   never show up as a spike on a graph;
//! * `sysUpTime` itself is a 32-bit tick counter and wraps after ~497 days.
//!   A backwards step that is explained by that wrap (it was near the end of
//!   the range and the wrapped difference is a plausible poll interval) is an
//!   interval, not a reboot.
//!
//! Nothing here guesses: when an interval cannot be measured the caller gets
//! `None` and writes no sample at all, leaving a gap in the series.

/// How wide a counter is, and so what it wraps at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    Bits32,
    Bits64,
    /// Read off the text of a `show` command: a number, with nothing to say
    /// how wide the counter behind it is. It has no wrap point to take a
    /// delta around, so one that went down is never a wrap.
    Text,
}

/// One reading of one counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counter {
    pub value: u64,
    pub width: Width,
}

impl Counter {
    pub fn bits32(value: u64) -> Counter {
        Counter {
            value,
            width: Width::Bits32,
        }
    }

    pub fn bits64(value: u64) -> Counter {
        Counter {
            value,
            width: Width::Bits64,
        }
    }

    pub fn text(value: u64) -> Counter {
        Counter {
            value,
            width: Width::Text,
        }
    }
}

/// What a device says about how long it has been running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeBase {
    /// `sysUpTime`, in hundredths of a second, 32 bits wide.
    Ticks(u32),
    /// Uptime in whole seconds, as a CLI `show version` reports it.
    Seconds(u64),
    /// The device says nothing about its uptime: the poller's own clock is
    /// all there is, and a reboot between polls cannot be detected.
    Wall,
}

/// What two consecutive readings amount to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Interval {
    /// Seconds of device time between the readings.
    Elapsed(f64),
    /// The device restarted: discard the interval and start again.
    Rebooted,
    /// No usable interval (no time passed, or the two readings are not
    /// comparable). Discard, without calling it a reboot.
    Unusable,
}

/// `sysUpTime` ticks in a day.
const TICKS_PER_DAY: u32 = 8_640_000;
/// A backwards step of `sysUpTime` counts as the 32-bit wrap only if the
/// previous reading was within a day of the end of the range and the wrapped
/// difference is at most a day: anything else is a restart.
const WRAP_NEAR_END: u32 = u32::MAX - TICKS_PER_DAY;

/// The device time between two readings.
///
/// `wall_elapsed_ms` is what the poller's own clock says passed; it is only
/// used for [`TimeBase::Wall`].
pub fn interval(previous: TimeBase, current: TimeBase, wall_elapsed_ms: i64) -> Interval {
    match (previous, current) {
        (TimeBase::Ticks(before), TimeBase::Ticks(now)) => {
            if now >= before {
                seconds(f64::from(now - before) / 100.0)
            } else if before > WRAP_NEAR_END && now.wrapping_sub(before) <= TICKS_PER_DAY {
                // sysUpTime ran off the end of its 32 bits; the device did not restart.
                seconds(f64::from(now.wrapping_sub(before)) / 100.0)
            } else {
                Interval::Rebooted
            }
        }
        (TimeBase::Seconds(before), TimeBase::Seconds(now)) => {
            if now >= before {
                seconds((now - before) as f64)
            } else {
                Interval::Rebooted
            }
        }
        (TimeBase::Wall, TimeBase::Wall) => seconds(wall_elapsed_ms as f64 / 1000.0),
        // The device changed what it tells us about its uptime (SNMP came
        // back, or the CLI answered where SNMP used to): not comparable.
        _ => Interval::Unusable,
    }
}

fn seconds(elapsed: f64) -> Interval {
    if elapsed > 0.0 && elapsed.is_finite() {
        Interval::Elapsed(elapsed)
    } else {
        Interval::Unusable
    }
}

/// The most a 64-bit counter can plausibly have advanced across a wrap.
///
/// 2^56 octets is more than two weeks of a 400 Gbit/s port running flat out,
/// which no gap between two polls comes near; and a counter has to have
/// counted 99.6 % of its range to be within 2^56 of the top.
const MAX_64_BIT_WRAP: u64 = 1 << 56;

/// How much a counter advanced between two readings, taking the wrap into
/// account. `None` when the two readings cannot be compared: the counter
/// changed width (the device started or stopped offering the 64-bit
/// columns, or the reading came from the other source), or it went down in a
/// way that is not a wrap — it was started again on the device while the
/// device itself stayed up.
pub fn delta(previous: Counter, current: Counter) -> Option<u64> {
    if previous.width != current.width {
        return None;
    }
    if current.value >= previous.value {
        return Some(current.value - previous.value);
    }
    match current.width {
        // A 32-bit counter really does wrap — every 5.7 minutes on a busy
        // 100 Mbit/s port — and a wrap cannot be told from a restart of the
        // counter, so it is taken for the wrap it nearly always is. Both
        // readings come from a 32-bit column, so the arithmetic is modulo
        // 2^32 whatever the values happen to be stored in.
        Width::Bits32 => Some(u64::from(
            (current.value as u32).wrapping_sub(previous.value as u32),
        )),
        // A 64-bit octet counter takes 46 years to wrap at 100 Gbit/s, so
        // one that went down has almost never wrapped: statistics were
        // cleared, a line card was re-seated, an interface was created
        // again. Calling that a wrap charts 10^18 bit/s. It is a wrap only
        // when the counter had really reached the end of its range, which
        // is to say when the wrapped difference is small enough to be
        // traffic; anything else is an interval that cannot be measured.
        Width::Bits64 => {
            let wrapped = current.value.wrapping_sub(previous.value);
            (wrapped <= MAX_64_BIT_WRAP).then_some(wrapped)
        }
        Width::Text => None,
    }
}

/// Units per second between two readings, or `None` if the interval or the
/// delta could not be worked out.
pub fn rate(previous: Counter, current: Counter, elapsed_s: f64) -> Option<f64> {
    if elapsed_s <= 0.0 || !elapsed_s.is_finite() {
        return None;
    }
    let rate = delta(previous, current)? as f64 / elapsed_s;
    rate.is_finite().then_some(rate)
}

/// Bits per second from an octet counter.
pub fn bits_rate(previous: Counter, current: Counter, elapsed_s: f64) -> Option<f64> {
    rate(previous, current, elapsed_s).map(|octets| octets * 8.0)
}

/// Utilisation as a percentage of the interface's speed. Full duplex: the
/// busier direction decides.
pub fn utilisation(
    in_bps: Option<f64>,
    out_bps: Option<f64>,
    speed_bps: Option<u64>,
) -> Option<f64> {
    let speed = speed_bps.filter(|speed| *speed > 0)? as f64;
    let busiest = in_bps.unwrap_or(0.0).max(out_bps.unwrap_or(0.0));
    // A rate above the nominal speed is possible on a mis-reported speed; it
    // is reported as it is rather than capped, so the number stays honest.
    (in_bps.is_some() || out_bps.is_some()).then(|| busiest / speed * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_counter_that_only_goes_up_is_a_plain_difference() {
        assert_eq!(
            delta(Counter::bits64(1_000), Counter::bits64(3_000)),
            Some(2_000)
        );
        assert_eq!(
            rate(Counter::bits64(1_000), Counter::bits64(3_000), 10.0),
            Some(200.0)
        );
        assert_eq!(
            bits_rate(Counter::bits32(0), Counter::bits32(1_250_000), 10.0),
            Some(1_000_000.0)
        );
    }

    #[test]
    fn a_32_bit_counter_wraps_at_two_to_the_thirty_two() {
        let before = Counter::bits32(u64::from(u32::MAX) - 99);
        let after = Counter::bits32(100);
        // 99 to get to the top, one more to wrap to zero, then 100.
        assert_eq!(delta(before, after), Some(200));
        assert_eq!(rate(before, after, 2.0), Some(100.0));
    }

    #[test]
    fn a_64_bit_counter_wraps_at_two_to_the_sixty_four() {
        let before = Counter::bits64(u64::MAX - 9);
        let after = Counter::bits64(5);
        assert_eq!(delta(before, after), Some(15));
        // And a 64-bit reading is never mistaken for a 32-bit one.
        assert_eq!(delta(Counter::bits32(10), Counter::bits64(20)), None);
    }

    #[test]
    fn a_64_bit_counter_that_went_down_far_from_the_top_was_started_again() {
        // 5 GB counted, then 1 kB, with the device up throughout.
        let before = Counter::bits64(5_000_000_000);
        let after = Counter::bits64(1_000);
        assert_eq!(delta(before, after), None);
        assert_eq!(bits_rate(before, after, 300.0), None, "a gap, not 4.9e17");
        // Right up to the edge of what a wrap could be.
        let top = Counter::bits64(u64::MAX);
        assert_eq!(
            delta(top, Counter::bits64(MAX_64_BIT_WRAP - 1)),
            Some(MAX_64_BIT_WRAP)
        );
        assert_eq!(delta(top, Counter::bits64(MAX_64_BIT_WRAP)), None);
    }

    #[test]
    fn a_counter_read_off_text_never_wraps() {
        assert_eq!(
            delta(Counter::text(1_000), Counter::text(3_000)),
            Some(2_000)
        );
        assert_eq!(
            rate(Counter::text(1_000), Counter::text(3_000), 10.0),
            Some(200.0)
        );
        // `clear counters`: down, with nothing to wrap around.
        assert_eq!(
            delta(Counter::text(5_000_000_000), Counter::text(1_000)),
            None
        );
        // Not even from the top of a range the text never claimed to have.
        assert_eq!(delta(Counter::text(u64::MAX - 9), Counter::text(5)), None);
        assert_eq!(
            delta(Counter::text(u64::from(u32::MAX)), Counter::text(5)),
            None
        );
        // And a number off the CLI is not compared with one off SNMP.
        assert_eq!(delta(Counter::text(10), Counter::bits64(20)), None);
    }

    #[test]
    fn an_uptime_that_went_backwards_is_a_reboot_not_a_spike() {
        // 10 minutes of uptime, then 5 seconds: the device restarted.
        assert_eq!(
            interval(TimeBase::Ticks(60_000), TimeBase::Ticks(500), 60_000),
            Interval::Rebooted
        );
        assert_eq!(
            interval(TimeBase::Seconds(86_400), TimeBase::Seconds(12), 60_000),
            Interval::Rebooted
        );
        // Forwards is an ordinary interval, in hundredths of a second.
        assert_eq!(
            interval(TimeBase::Ticks(60_000), TimeBase::Ticks(66_000), 60_000),
            Interval::Elapsed(60.0)
        );
        assert_eq!(
            interval(TimeBase::Seconds(100), TimeBase::Seconds(160), 60_000),
            Interval::Elapsed(60.0)
        );
    }

    #[test]
    fn sysuptime_running_off_the_end_of_its_32_bits_is_still_an_interval() {
        let before = TimeBase::Ticks(u32::MAX - 1_000);
        let after = TimeBase::Ticks(5_000);
        assert_eq!(interval(before, after, 60_000), Interval::Elapsed(60.01));
        // But a device that was up for a week and now says five seconds rebooted.
        assert_eq!(
            interval(TimeBase::Ticks(60_480_000), TimeBase::Ticks(500), 60_000),
            Interval::Rebooted
        );
    }

    #[test]
    fn intervals_that_cannot_be_measured_are_unusable_rather_than_zero() {
        assert_eq!(
            interval(TimeBase::Ticks(100), TimeBase::Ticks(100), 60_000),
            Interval::Unusable
        );
        assert_eq!(
            interval(TimeBase::Ticks(100), TimeBase::Seconds(100), 60_000),
            Interval::Unusable
        );
        assert_eq!(
            interval(TimeBase::Wall, TimeBase::Wall, 0),
            Interval::Unusable
        );
        assert_eq!(
            interval(TimeBase::Wall, TimeBase::Wall, 30_000),
            Interval::Elapsed(30.0)
        );
        assert_eq!(rate(Counter::bits64(0), Counter::bits64(10), 0.0), None);
    }

    #[test]
    fn utilisation_needs_a_speed_and_a_rate() {
        assert_eq!(
            utilisation(Some(500_000_000.0), Some(10_000.0), Some(1_000_000_000)),
            Some(50.0)
        );
        assert_eq!(utilisation(Some(1.0), None, None), None);
        assert_eq!(utilisation(Some(1.0), None, Some(0)), None);
        assert_eq!(utilisation(None, None, Some(1_000)), None);
    }
}
