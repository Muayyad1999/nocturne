//! Durations as network devices print them.

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
const WEEK: u64 = 7 * DAY;
/// Uptimes that count years (Cisco) mean calendar-ish years of 365 days.
const YEAR: u64 = 365 * DAY;

/// The units of [`format_duration`], largest first.
const UNITS: [(u64, &str); 5] = [
    (WEEK, "w"),
    (DAY, "d"),
    (HOUR, "h"),
    (MINUTE, "m"),
    (1, "s"),
];

/// Parse a duration into whole seconds.
///
/// Understands what uptime and timer columns look like across vendors:
/// `2w3d`, `3d04h`, `1y2w` (Cisco), `1w2d3h4m5s` (RouterOS), `3w2d 04:05:06`
/// (Junos), `2 weeks, 3 days, 4 hours, 5 minutes`, `1 day(s), 4 hour(s)`,
/// `5 days, 3:04` (hours:minutes), `12:34:56`, `1:02:03:04`
/// (days:hours:minutes:seconds) and SNMP TimeTicks as net-snmp prints them,
/// `(8640000) 1 day, 0:00:00.00`. A leading `... uptime is`, `Uptime:` or
/// `up` is ignored. A bare number is a number of seconds.
///
/// Returns `None` for anything else, and for durations beyond `u64` seconds.
pub fn parse_duration_s(input: &str) -> Option<u64> {
    // No device prints an uptime anywhere near this long.
    const MAX_LEN: usize = 256;
    if input.len() > MAX_LEN {
        return None;
    }
    let lower = input.trim().to_ascii_lowercase();
    let mut text = lower.as_str();
    for label in ["uptime is", "uptime:"] {
        if let Some(position) = text.find(label) {
            text = &text[position + label.len()..];
        }
    }
    text = text.trim_start();
    text = text.strip_prefix("up ").unwrap_or(text);
    text = text.trim_end_matches(['.', ',', ';']).trim();

    // TimeTicks are hundredths of a second; the tick count is authoritative.
    let labelled = text.strip_prefix("timeticks:").map(str::trim_start);
    if let Some((ticks, _)) = labelled
        .unwrap_or(text)
        .strip_prefix('(')
        .and_then(|rest| rest.split_once(')'))
    {
        return ticks.trim().parse::<u64>().ok().map(|ticks| ticks / 100);
    }
    if let Some(ticks) = labelled {
        return ticks.parse::<u64>().ok().map(|ticks| ticks / 100);
    }

    let mut total: u64 = 0;
    let mut parts = 0usize;
    let mut rest = text;
    loop {
        rest = rest.trim_start_matches([' ', '\t', ',']);
        if let Some(after) = rest.strip_prefix("and ") {
            rest = after;
            continue;
        }
        if rest.is_empty() {
            break;
        }
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let amount: u64 = rest[..digits].parse().ok()?;
        let after_number = &rest[digits..];
        if after_number.starts_with(':') {
            // A clock ends the duration: `5 days, 3:04`.
            total = total.checked_add(clock(rest)?)?;
            parts += 1;
            break;
        }
        let after_number = after_number.trim_start();
        let letters = after_number
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(after_number.len());
        let (unit, after_unit) = after_number.split_at(letters);
        if unit.is_empty() {
            // A number without a unit is only meaningful on its own, as seconds.
            return (parts == 0 && after_number.is_empty()).then_some(amount);
        }
        let seconds = match unit {
            "y" | "yr" | "yrs" | "year" | "years" => amount.checked_mul(YEAR)?,
            "w" | "wk" | "wks" | "week" | "weeks" => amount.checked_mul(WEEK)?,
            "d" | "day" | "days" => amount.checked_mul(DAY)?,
            "h" | "hr" | "hrs" | "hour" | "hours" => amount.checked_mul(HOUR)?,
            "m" | "min" | "mins" | "minute" | "minutes" => amount.checked_mul(MINUTE)?,
            "s" | "sec" | "secs" | "second" | "seconds" => amount,
            "ms" | "msec" | "msecs" | "millisecond" | "milliseconds" => amount / 1000,
            _ => return None,
        };
        total = total.checked_add(seconds)?;
        parts += 1;
        // NX-OS writes `3 day(s), 4 hour(s)`.
        rest = after_unit.strip_prefix("(s)").unwrap_or(after_unit);
    }
    (parts > 0).then_some(total)
}

/// `h:mm`, `h:mm:ss` or `d:h:mm:ss`, optionally with a fraction of a second.
fn clock(text: &str) -> Option<u64> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, "0"));
    if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut fields = [0u64; 4];
    let mut count = 0;
    for field in whole.split(':') {
        if count == fields.len() || field.is_empty() || !field.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        fields[count] = field.parse().ok()?;
        count += 1;
    }
    let (days, hours, minutes, seconds) = match fields[..count] {
        [hours, minutes] => (0, hours, minutes, 0),
        [hours, minutes, seconds] => (0, hours, minutes, seconds),
        [days, hours, minutes, seconds] if hours < 24 => (days, hours, minutes, seconds),
        _ => return None,
    };
    if minutes >= 60 || seconds >= 60 {
        return None;
    }
    days.checked_mul(DAY)?
        .checked_add(hours.checked_mul(HOUR)?)?
        .checked_add(minutes * MINUTE + seconds)
}

/// Format seconds as weeks, days, hours, minutes and seconds, leaving out
/// the units that are zero: `2w 3d 4h 5m`, `45s`, `0s`.
pub fn format_duration(seconds: u64) -> String {
    format_units(seconds, UNITS.len())
}

/// As [`format_duration`], but only the three largest units from the first
/// one that is not zero: `1w 2d 6h` — enough for an estimate.
pub(crate) fn format_duration_short(seconds: u64) -> String {
    format_units(seconds, 3)
}

fn format_units(seconds: u64, max_units: usize) -> String {
    let first = UNITS
        .iter()
        .position(|(size, _)| seconds >= *size)
        .unwrap_or(UNITS.len() - 1);
    let mut remaining = seconds;
    let mut parts = Vec::new();
    for (size, suffix) in UNITS.iter().skip(first).take(max_units) {
        let count = remaining / size;
        remaining %= size;
        if count > 0 {
            parts.push(format!("{count}{suffix}"));
        }
    }
    if parts.is_empty() {
        "0s".to_owned()
    } else {
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_uptime_formats() {
        for (text, seconds) in [
            // Cisco IOS / IOS-XE
            ("2w3d", 2 * WEEK + 3 * DAY),
            ("3d04h", 3 * DAY + 4 * HOUR),
            ("1y2w", YEAR + 2 * WEEK),
            ("00:12:34", 12 * MINUTE + 34),
            (
                "2 weeks, 3 days, 4 hours, 5 minutes",
                2 * WEEK + 3 * DAY + 4 * HOUR + 5 * MINUTE,
            ),
            (
                "1 year, 2 weeks, 3 days, 4 hours, 5 minutes",
                YEAR + 2 * WEEK + 3 * DAY + 4 * HOUR + 5 * MINUTE,
            ),
            (
                "core-sw-01 uptime is 1 day, 4 hours, 5 minutes",
                DAY + 4 * HOUR + 5 * MINUTE,
            ),
            // NX-OS
            (
                "Kernel uptime is 12 day(s), 3 hour(s), 4 minute(s), 5 second(s)",
                12 * DAY + 3 * HOUR + 4 * MINUTE + 5,
            ),
            // Arista EOS
            (
                "Uptime: 2 weeks, 1 days, 3 hours and 4 minutes",
                2 * WEEK + DAY + 3 * HOUR + 4 * MINUTE,
            ),
            (
                "2 weeks, 1 days, 3 hours and 4 minutes",
                2 * WEEK + DAY + 3 * HOUR + 4 * MINUTE,
            ),
            // Junos
            (
                "3w2d 04:05:06",
                3 * WEEK + 2 * DAY + 4 * HOUR + 5 * MINUTE + 6,
            ),
            ("1d 2:03", DAY + 2 * HOUR + 3 * MINUTE),
            ("12:34:56", 12 * HOUR + 34 * MINUTE + 56),
            // RouterOS
            ("1w2d3h4m5s", WEEK + 2 * DAY + 3 * HOUR + 4 * MINUTE + 5),
            ("4m5s", 4 * MINUTE + 5),
            ("58s", 58),
            // Linux `uptime`
            ("up 5 days, 3:04", 5 * DAY + 3 * HOUR + 4 * MINUTE),
            ("up 3 min", 3 * MINUTE),
            ("up 1:23", HOUR + 23 * MINUTE),
            // net-snmp
            ("1 day, 0:00:00.00", DAY),
            ("1:2:03:04.56", DAY + 2 * HOUR + 3 * MINUTE + 4),
            ("0:0:00:35.00", 35),
            // Plain words and numbers
            ("1 day, 4 hours, 5 minutes", DAY + 4 * HOUR + 5 * MINUTE),
            ("90 minutes", 90 * MINUTE),
            ("36 hrs", 36 * HOUR),
            ("2 HOURS", 2 * HOUR),
            ("1500 ms", 1),
            ("3600", 3600),
            ("0", 0),
            ("  45s. ", 45),
            ("250:00:00", 250 * HOUR),
        ] {
            assert_eq!(parse_duration_s(text), Some(seconds), "{text}");
        }
    }

    #[test]
    fn timeticks_are_hundredths_of_a_second() {
        assert_eq!(
            parse_duration_s("(8640000) 1 day, 0:00:00.00"),
            Some(86_400)
        );
        assert_eq!(
            parse_duration_s("Timeticks: (8640000) 1 day, 0:00:00.00"),
            Some(86_400)
        );
        assert_eq!(
            parse_duration_s("Timeticks: (123456) 0:20:34.56"),
            Some(1234)
        );
        assert_eq!(parse_duration_s("(99)"), Some(0));
        assert_eq!(parse_duration_s("timeticks: 360000"), Some(3600));
        assert_eq!(parse_duration_s("(abc) 1 day"), None);
        assert_eq!(parse_duration_s("(-5)"), None);
    }

    #[test]
    fn what_is_not_a_duration() {
        for text in [
            "",
            "   ",
            "never",
            "soon",
            "5 fortnights",
            "1.5h",
            "-5m",
            "5 days, 3",
            "3 5",
            "12:60:00",
            "12:00:60",
            "1:24:00:00",
            "1:2:3:4:5",
            "12:",
            ":30",
            "12:3x",
            "12:30.x",
            "12:30.5.5",
            "3:04 5 days",
            "5 months",
            "1h and",
            "99999999999999999999",
            "99999999999999999 years",
            "18446744073709551615w",
            "٣ days",
        ] {
            assert_eq!(parse_duration_s(text), None, "{text}");
        }
    }

    #[test]
    fn overlong_input_is_not_a_duration() {
        assert_eq!(parse_duration_s(&"1w".repeat(128)), Some(128 * WEEK));
        assert_eq!(parse_duration_s(&"1w".repeat(129)), None);
        assert_eq!(parse_duration_s(&" ".repeat(1 << 20)), None);
    }

    #[test]
    fn formatting() {
        for (seconds, text) in [
            (0, "0s"),
            (45, "45s"),
            (60, "1m"),
            (61, "1m 1s"),
            (3600, "1h"),
            (86_400, "1d"),
            (604_800, "1w"),
            (2 * WEEK + 3 * DAY + 4 * HOUR + 5 * MINUTE, "2w 3d 4h 5m"),
            (WEEK + 2 * DAY + 3 * HOUR + 4 * MINUTE + 5, "1w 2d 3h 4m 5s"),
            (DAY + 5, "1d 5s"),
        ] {
            assert_eq!(format_duration(seconds), text, "{seconds}");
        }
        // u64::MAX seconds = 30,500,568,904,943 weeks and a bit.
        assert!(format_duration(u64::MAX).starts_with("30500568904943w "));
        assert_eq!(format_duration_short(800_000), "1w 2d 6h");
        assert_eq!(format_duration_short(24_352), "6h 45m 52s");
        assert_eq!(format_duration_short(WEEK + 5), "1w");
        assert_eq!(format_duration_short(59), "59s");
        assert_eq!(format_duration_short(0), "0s");
    }

    #[test]
    fn format_and_parse_round_trip() {
        let mut seconds = 1u64;
        while seconds < u64::MAX / 3 {
            for value in [seconds - 1, seconds, seconds + 1] {
                assert_eq!(
                    parse_duration_s(&format_duration(value)),
                    Some(value),
                    "{value}"
                );
            }
            seconds = seconds.saturating_mul(3).saturating_add(7);
        }
        assert_eq!(parse_duration_s(&format_duration(u64::MAX)), Some(u64::MAX));
    }
}
