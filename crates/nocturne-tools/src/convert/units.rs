//! Data sizes, link rates and how long a transfer takes.
//!
//! Units follow the standards: `kB`/`MB`/`GB` are powers of 1000 and
//! `KiB`/`MiB`/`GiB` powers of 1024; `b` is a bit and `B` a byte. Because
//! people type units in whatever case comes to hand, the case of `b`/`B` is
//! only taken literally when the unit is written in mixed case (`Mb`,
//! `MB/s`, `kB/s`). A unit in a single case (`mb`, `MB`, `MBPS`, `gb`) means
//! bytes for a size and bits for a rate, the natural quantity of each.

use serde::{Deserialize, Serialize};

use super::duration::format_duration_short;
use crate::ToolError;
use crate::error::snip;

/// How long a transfer takes.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TransferEstimate {
    /// Duration in seconds.
    pub seconds: f64,
    /// The duration for people: `350 ms`, `56 s`, `1m 10s`, `1w 2d 6h`.
    pub human: String,
    /// The payload rate after subtracting the protocol overhead, in bit/s.
    pub effective_bps: f64,
}

/// Estimate how long `size` takes to cross a link of `rate`.
///
/// `size` is a data size (`700 MB`, `4.7GiB`, a number of bytes), `rate` a
/// link rate (`100 Mbps`, `1.5G`, a number of bit/s). `overhead_pct` is the
/// share of the link taken by protocol headers and retransmissions, from 0
/// up to (not including) 100: with 5, a 100 Mbps link carries 95 Mbps of payload.
pub fn transfer_time(size: &str, rate: &str, overhead_pct: f64) -> Result<TransferEstimate, ToolError> {
    let bytes = parse_size_bytes(size)?;
    let bps = parse_rate_bps(rate)?;
    if bps == 0 {
        return Err(ToolError::invalid("rate", rate.trim(), "nothing ever finishes over a link of 0 bit/s"));
    }
    if !(0.0..100.0).contains(&overhead_pct) {
        return Err(ToolError::invalid(
            "overhead",
            &overhead_pct.to_string(),
            "the protocol overhead is a percentage from 0 up to (not including) 100",
        ));
    }
    let effective_bps = bps as f64 * (1.0 - overhead_pct / 100.0);
    let seconds = bytes as f64 * 8.0 / effective_bps;
    Ok(TransferEstimate { seconds, human: human_seconds(seconds), effective_bps })
}

/// Milliseconds below a second, tenths below a minute, then the three
/// largest units.
fn human_seconds(seconds: f64) -> String {
    let milliseconds = (seconds * 1000.0).round();
    let tenths = (seconds * 10.0).round() / 10.0;
    if seconds == 0.0 {
        "0 s".to_owned()
    } else if milliseconds < 1.0 {
        "less than 1 ms".to_owned()
    } else if milliseconds < 1000.0 {
        format!("{milliseconds} ms")
    } else if tenths < 60.0 {
        format!("{tenths} s")
    } else {
        // `as` saturates, which is the right answer for absurd durations.
        format_duration_short(seconds.round() as u64)
    }
}

/// Parse a data size into bytes.
///
/// `700 MB`, `4.7GiB`, `1.5 TB`, `512k`, `1024` (bytes), `10 Mbit`. Decimal
/// prefixes are powers of 1000, `Ki`/`Mi`/`Gi`/... powers of 1024. The
/// result is rounded to the nearest byte.
pub fn parse_size_bytes(input: &str) -> Result<u64, ToolError> {
    let text = input.trim();
    let fail = |reason: String| ToolError::invalid("size", text, reason);
    let (amount, unit) = split_quantity(text).map_err(fail)?;
    let unknown_unit = || {
        fail(format!(
            "`{}` is not a size unit; use B, kB, MB, GB, TB, PB, EB (powers of 1000) or \
             KiB, MiB, GiB, TiB, PiB, EiB (powers of 1024)",
            snip(unit, 20)
        ))
    };
    let (multiplier, base) = split_unit(unit).ok_or_else(unknown_unit)?;
    let in_bits = match base {
        Base::None | Base::Bytes | Base::UpperB => false,
        Base::Bits => true,
        // A lower-case b is a bit only when the prefix was deliberately capitalised (`Mb`).
        Base::LowerB => unit.bytes().any(|byte| byte.is_ascii_uppercase()),
    };
    scale(&amount, multiplier, if in_bits { 8 } else { 1 })
        .ok_or_else(|| ToolError::TooLarge(format!("`{}` is more than a 64-bit byte count can hold (16 EiB)", snip(text, 60))))
}

/// Parse a link rate into bits per second.
///
/// `1G`, `10 Gbps`, `100Mb/s`, `2.5 Gbit/s`, `9600`, and byte rates as file
/// transfer tools print them (`11.2 MB/s`, `850 kB/s`). Prefixes are powers
/// of 1000. The result is rounded to the nearest bit per second.
pub fn parse_rate_bps(input: &str) -> Result<u64, ToolError> {
    let text = input.trim();
    let fail = |reason: String| ToolError::invalid("rate", text, reason);
    let (amount, unit) = split_quantity(text).map_err(fail)?;
    let unknown_unit = || {
        fail(format!(
            "`{}` is not a rate unit; use bps, kbps, Mbps, Gbps, Tbps (or just k, M, G, T), or a byte rate such as MB/s",
            snip(unit, 20)
        ))
    };
    // "per second" in any of its spellings; ASCII lower-casing keeps the byte offsets.
    let lower = unit.to_ascii_lowercase();
    let per_second =
        ["/second", "/sec", "/s", "ps"].iter().find(|suffix| lower.ends_with(**suffix)).map_or(0, |suffix| suffix.len());
    let quantity = unit[..unit.len() - per_second].trim_end();
    if quantity.is_empty() && per_second > 0 {
        return Err(unknown_unit());
    }
    let (multiplier, base) = split_unit(quantity).ok_or_else(unknown_unit)?;
    let in_bytes = match base {
        Base::None | Base::Bits | Base::LowerB => false,
        Base::Bytes => true,
        // An upper-case B is a byte only in a mixed-case unit (`MB/s`, `kB/s`), not in `MBPS`.
        Base::UpperB => unit.bytes().any(|byte| byte.is_ascii_lowercase()),
    };
    // No prefix comes near overflowing u128 when multiplied by 8.
    scale(&amount, multiplier * if in_bytes { 8 } else { 1 }, 1)
        .ok_or_else(|| ToolError::TooLarge(format!("`{}` is more than a 64-bit rate can hold", snip(text, 60))))
}

/// A non-negative decimal number: `digits / 10^scale`.
struct Decimal {
    digits: u128,
    scale: u32,
}

/// The most fractional digits kept; more cannot change a whole number of
/// bytes or bits by any amount that matters.
const MAX_FRACTION_DIGITS: usize = 18;

/// Split `4.7 GiB` into the number and the unit text after it.
fn split_quantity(text: &str) -> Result<(Decimal, &str), String> {
    if text.is_empty() {
        return Err("nothing was entered".to_owned());
    }
    if text.starts_with('-') {
        return Err("it cannot be negative".to_owned());
    }
    let number_len = text.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '_')).unwrap_or(text.len());
    let (number, unit) = text.split_at(number_len);
    let unit = unit.trim_start();
    if unit.starts_with(',') {
        return Err("write the number with `.` as the decimal mark and without thousands separators".to_owned());
    }
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if fraction.contains('.') {
        return Err("the number has more than one decimal point".to_owned());
    }
    let digits_of =
        |part: &str| part.bytes().filter(u8::is_ascii_digit).map(|byte| u128::from(byte - b'0')).collect::<Vec<_>>();
    let (whole, fraction) = (digits_of(whole), digits_of(fraction));
    if whole.is_empty() && fraction.is_empty() {
        return Err("it does not start with a number".to_owned());
    }
    let fraction = &fraction[..fraction.len().min(MAX_FRACTION_DIGITS)];
    let mut digits: u128 = 0;
    for digit in whole.iter().chain(fraction) {
        digits = digits
            .checked_mul(10)
            .and_then(|digits| digits.checked_add(*digit))
            .ok_or_else(|| "the number has too many digits".to_owned())?;
    }
    Ok((Decimal { digits, scale: fraction.len() as u32 }, unit))
}

/// What a unit counts, after its prefix.
enum Base {
    /// Nothing: just a prefix (`512k`) or no unit at all.
    None,
    /// `b`
    LowerB,
    /// `B`
    UpperB,
    /// `bit`, `bits`
    Bits,
    /// `byte`, `bytes`
    Bytes,
}

/// Split a unit into the multiplier of its prefix and what it counts:
/// `MiB` → (2^20, `B`), `k` → (1000, nothing), `bits` → (1, bits).
fn split_unit(unit: &str) -> Option<(u128, Base)> {
    let mut chars = unit.chars();
    let (multiplier, rest) = match chars.next() {
        None => return Some((1, Base::None)),
        Some(prefix) => {
            let power = match prefix {
                'k' | 'K' => 1,
                // Nobody measures millibytes: a lower-case m is a sloppy mega.
                'm' | 'M' => 2,
                'g' | 'G' => 3,
                't' | 'T' => 4,
                'p' | 'P' => 5,
                'e' | 'E' => 6,
                _ => 0,
            };
            match (power, chars.as_str()) {
                (0, _) => (1, unit),
                (power, rest) => match rest.strip_prefix(['i', 'I']) {
                    Some(rest) => (1u128 << (10 * power), rest),
                    None => (1000u128.pow(power), rest),
                },
            }
        }
    };
    let base = match rest {
        "" => Base::None,
        "b" => Base::LowerB,
        "B" => Base::UpperB,
        word if word.eq_ignore_ascii_case("bit") || word.eq_ignore_ascii_case("bits") => Base::Bits,
        word if word.eq_ignore_ascii_case("byte") || word.eq_ignore_ascii_case("bytes") => Base::Bytes,
        _ => return None,
    };
    Some((multiplier, base))
}

/// `amount * multiplier / divisor`, rounded half up; `None` if it exceeds `u64`.
fn scale(amount: &Decimal, multiplier: u128, divisor: u128) -> Option<u64> {
    // The whole and the fractional part are scaled separately, so that the
    // multiplication cannot overflow for any amount whose result fits.
    let unit = 10u128.pow(amount.scale);
    let (whole, fraction) = (amount.digits / unit, amount.digits % unit);
    let scaled_whole = whole.checked_mul(multiplier)?;
    let denominator = unit * divisor;
    let carried = (scaled_whole % divisor) * unit + fraction * multiplier;
    let total = (scaled_whole / divisor).checked_add((carried + denominator / 2) / denominator)?;
    u64::try_from(total).ok()
}

/// Format a rate in bit/s with a decimal prefix: `1.25 Gbps`, `9.6 kbps`, `100 Mbps`.
pub fn format_rate(bps: f64) -> String {
    format_scaled(bps, 1000.0, &["bps", "kbps", "Mbps", "Gbps", "Tbps", "Pbps", "Ebps"])
}

/// Format a byte count with a binary prefix: `4.38 GiB`, `512 B`, `1.5 KiB`.
pub fn format_bytes(bytes: f64) -> String {
    format_scaled(bytes, 1024.0, &["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"])
}

/// `value` in the largest unit that keeps it at or above 1, rounded to two
/// decimals. Not-a-number and the infinities are `n/a`.
fn format_scaled(value: f64, step: f64, units: &[&str]) -> String {
    if !value.is_finite() {
        return "n/a".to_owned();
    }
    let hundredths = |value: f64| (value * 100.0).round() / 100.0;
    let mut scaled = value.abs();
    let mut unit = 0;
    // Round before comparing: 999.999 kbps is 1 Mbps, not 1000 kbps.
    while unit + 1 < units.len() && hundredths(scaled) >= step {
        scaled /= step;
        unit += 1;
    }
    let sign = if value < 0.0 && hundredths(scaled) > 0.0 { "-" } else { "" };
    format!("{sign}{} {}", hundredths(scaled), units[unit])
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn sizes_decimal_and_binary() {
        for (input, bytes) in [
            ("0", 0u64),
            ("1024", 1024),
            ("1024 B", 1024),
            ("10 bytes", 10),
            ("1 kB", 1_000),
            ("1 KB", 1_000),
            ("1KiB", 1_024),
            ("700 MB", 700_000_000),
            ("700MB", 700_000_000),
            ("700 mb", 700_000_000),
            ("700 MiB", 734_003_200),
            ("4.7GiB", 5_046_586_573), // 4.7 * 2^30 = 5046586572.8
            ("4.7 GB", 4_700_000_000),
            ("1.5 TB", 1_500_000_000_000),
            ("2 TiB", 2_199_023_255_552),
            ("1 PB", 1_000_000_000_000_000),
            ("1 PiB", 1_125_899_906_842_624),
            ("1 EB", 1_000_000_000_000_000_000),
            ("15 EiB", 17_293_822_569_102_704_640),
            ("512k", 512_000),
            ("512K", 512_000),
            ("2G", 2_000_000_000),
            ("2Gi", 2_147_483_648),
            ("0.5 kB", 500),
            (".5 kB", 500),
            ("1_000_000", 1_000_000),
            ("18446744073709551615", u64::MAX),
            ("1.0000000000000000001 kB", 1_000),
            ("1.9999 B", 2),
            ("0.4999 B", 0),
        ] {
            assert_eq!(parse_size_bytes(input), Ok(bytes), "{input}");
        }
    }

    #[test]
    fn sizes_in_bits_need_deliberate_spelling() {
        // Mixed case is taken literally: Mb is megabit, MB megabyte.
        assert_eq!(parse_size_bytes("8 Mb"), Ok(1_000_000));
        assert_eq!(parse_size_bytes("8 Mbit"), Ok(1_000_000));
        assert_eq!(parse_size_bytes("8 mbit"), Ok(1_000_000));
        assert_eq!(parse_size_bytes("8 MBITS"), Ok(1_000_000));
        assert_eq!(parse_size_bytes("12 bits"), Ok(2)); // 1.5 bytes, rounded half up
        assert_eq!(parse_size_bytes("8 Kib"), Ok(1_024));
        // Single-case units are bytes.
        assert_eq!(parse_size_bytes("8 mb"), Ok(8_000_000));
        assert_eq!(parse_size_bytes("8 MB"), Ok(8_000_000));
        assert_eq!(parse_size_bytes("8 b"), Ok(8));
    }

    #[test]
    fn rates() {
        for (input, bps) in [
            ("9600", 9_600u64),
            ("9600 bps", 9_600),
            ("64k", 64_000),
            ("64 kbps", 64_000),
            ("1.544 Mbps", 1_544_000),
            ("100Mb/s", 100_000_000),
            ("100 Mbps", 100_000_000),
            ("100 mbps", 100_000_000),
            ("100M", 100_000_000),
            ("100 Mbit/s", 100_000_000),
            ("1G", 1_000_000_000),
            ("1.5G", 1_500_000_000),
            ("10 Gbps", 10_000_000_000),
            ("10GBPS", 10_000_000_000),
            ("2.5 Gbit/s", 2_500_000_000),
            ("400 Gb/s", 400_000_000_000),
            ("1 Tbps", 1_000_000_000_000),
            ("1 Gb/sec", 1_000_000_000),
            ("1 Gbit/second", 1_000_000_000),
            ("0", 0),
            ("0.4 bps", 0),
            ("0.5 bps", 1),
            ("18446744073709551615 bps", u64::MAX),
        ] {
            assert_eq!(parse_rate_bps(input), Ok(bps), "{input}");
        }
    }

    #[test]
    fn byte_rates_as_transfer_tools_print_them() {
        assert_eq!(parse_rate_bps("11.2 MB/s"), Ok(89_600_000));
        assert_eq!(parse_rate_bps("850 kB/s"), Ok(6_800_000));
        assert_eq!(parse_rate_bps("1 MiB/s"), Ok(8_388_608));
        assert_eq!(parse_rate_bps("125 MBps"), Ok(1_000_000_000));
        assert_eq!(parse_rate_bps("1 byte/s"), Ok(8));
        assert_eq!(parse_rate_bps("3 B/s"), Ok(24));
        assert_eq!(parse_rate_bps("0.3 B/s"), Ok(2)); // 2.4 bit/s
        // All upper case is how people shout megabits, not megabytes.
        assert_eq!(parse_rate_bps("100 MBPS"), Ok(100_000_000));
        assert_eq!(parse_rate_bps("100 MB"), Ok(100_000_000));
    }

    #[test]
    fn quantity_rejections() {
        let size = |input: &str| parse_size_bytes(input).unwrap_err().to_string();
        assert_eq!(size(""), "invalid size: nothing was entered");
        assert_eq!(size("-5 MB"), "invalid size `-5 MB`: it cannot be negative");
        assert_eq!(size("MB"), "invalid size `MB`: it does not start with a number");
        assert_eq!(size("."), "invalid size `.`: it does not start with a number");
        assert_eq!(size("1.2.3 MB"), "invalid size `1.2.3 MB`: the number has more than one decimal point");
        assert_eq!(
            size("4,7 GiB"),
            "invalid size `4,7 GiB`: write the number with `.` as the decimal mark and without thousands separators"
        );
        assert!(size("5 parsecs").starts_with("invalid size `5 parsecs`: `parsecs` is not a size unit; use B, kB, MB"));
        assert!(size("5 MBs").contains("`MBs` is not a size unit"));
        assert!(size("5 Mbps").contains("`Mbps` is not a size unit"));
        assert!(size("1e6").contains("`e6` is not a size unit"));
        assert_eq!(size("16 EiB"), "`16 EiB` is more than a 64-bit byte count can hold (16 EiB)");
        assert_eq!(size("18446744073709551616"), "`18446744073709551616` is more than a 64-bit byte count can hold (16 EiB)");
        assert!(matches!(parse_size_bytes(&format!("{} EB", "9".repeat(30))), Err(ToolError::TooLarge(_))));
        assert!(size(&"9".repeat(60)).contains("the number has too many digits"));

        let rate = |input: &str| parse_rate_bps(input).unwrap_err().to_string();
        assert_eq!(rate(""), "invalid rate: nothing was entered");
        assert!(rate("fast").contains("it does not start with a number"));
        assert!(rate("10 furlongs/s").contains("`furlongs/s` is not a rate unit"));
        assert!(rate("10 Mbpm").contains("`Mbpm` is not a rate unit"));
        assert!(rate("10 ps").contains("`ps` is not a rate unit"));
        assert!(rate("10 /s").contains("`/s` is not a rate unit"));
        assert_eq!(rate("99999999 Tbps"), "`99999999 Tbps` is more than a 64-bit rate can hold");
        assert!(matches!(parse_rate_bps("3 EB/s"), Err(ToolError::TooLarge(_))));
    }

    #[test]
    fn formatting_rates() {
        for (bps, text) in [
            (0.0, "0 bps"),
            (999.0, "999 bps"),
            (1_000.0, "1 kbps"),
            (9_600.0, "9.6 kbps"),
            (64_000.0, "64 kbps"),
            (1_544_000.0, "1.54 Mbps"),
            (100_000_000.0, "100 Mbps"),
            (1_250_000_000.0, "1.25 Gbps"),
            (10_000_000_000.0, "10 Gbps"),
            (400_000_000_000.0, "400 Gbps"),
            (1_000_000_000_000.0, "1 Tbps"),
            (999_994.0, "999.99 kbps"),
            (999_996.0, "1 Mbps"),
            (999_999.0, "1 Mbps"),
            (0.5, "0.5 bps"),
            (0.001, "0 bps"),
            (-0.001, "0 bps"),
            (-2_000_000.0, "-2 Mbps"),
            (1e21, "1000 Ebps"),
            (f64::NAN, "n/a"),
            (f64::INFINITY, "n/a"),
        ] {
            assert_eq!(format_rate(bps), text, "{bps}");
        }
    }

    #[test]
    fn formatting_bytes() {
        for (bytes, text) in [
            (0.0, "0 B"),
            (512.0, "512 B"),
            (1023.0, "1023 B"),
            (1024.0, "1 KiB"),
            (1536.0, "1.5 KiB"),
            (4_700_000_000.0, "4.38 GiB"),
            (1_048_576.0, "1 MiB"),
            (1_048_575.0, "1 MiB"),
            (1_099_511_627_776.0, "1 TiB"),
            (18_446_744_073_709_551_615.0, "16 EiB"),
            (-1536.0, "-1.5 KiB"),
            (f64::NEG_INFINITY, "n/a"),
        ] {
            assert_eq!(format_bytes(bytes), text, "{bytes}");
        }
    }

    #[test]
    fn parsing_and_formatting_round_trip() {
        for bps in [0u64, 1, 999, 1_000, 64_000, 1_500_000, 100_000_000, 2_500_000_000, 400_000_000_000] {
            assert_eq!(parse_rate_bps(&format_rate(bps as f64)), Ok(bps), "{bps}");
        }
        for bytes in [0u64, 1, 1023, 1024, 1536, 1 << 20, 3 << 30, 1 << 40, 1 << 50, 1 << 60] {
            assert_eq!(parse_size_bytes(&format_bytes(bytes as f64)), Ok(bytes), "{bytes}");
        }
    }

    #[test]
    fn transfer_times() {
        // 700 MB over 100 Mbps: 5.6e9 bits / 1e8 bit/s = 56 s.
        let estimate = transfer_time("700 MB", "100 Mbps", 0.0).unwrap();
        assert_eq!((estimate.seconds, estimate.human.as_str(), estimate.effective_bps), (56.0, "56 s", 100_000_000.0));

        // With 20% overhead the payload rate is 80 Mbps: 70 s.
        let slower = transfer_time("700 MB", "100 Mbps", 20.0).unwrap();
        assert_eq!((slower.seconds, slower.human.as_str(), slower.effective_bps), (70.0, "1m 10s", 80_000_000.0));

        // A DVD image over a T1: 4.7e9 * 8 / 1.544e6 = 24352.3 s.
        let dvd = transfer_time("4.7 GB", "1.544 Mbps", 0.0).unwrap();
        assert!((dvd.seconds - 24_352.33).abs() < 0.01, "{}", dvd.seconds);
        assert_eq!(dvd.human, "6h 45m 52s");

        // 4.7 GiB over 1.5 Gbps: 5046586573 * 8 / 1.5e9 = 26.9 s.
        assert_eq!(transfer_time("4.7GiB", "1.5G", 0.0).unwrap().human, "26.9 s");
        assert_eq!(transfer_time("1 kB", "1G", 0.0).unwrap().human, "less than 1 ms");
        assert_eq!(transfer_time("1 MB", "1G", 0.0).unwrap().human, "8 ms");
        assert_eq!(transfer_time("0", "1G", 0.0).unwrap().human, "0 s");
        // 8e12 bits / 1e7 bit/s = 800,000 s = 1w 2d 6h 13m 20s, shown to three units.
        assert_eq!(transfer_time("1 TB", "10 Mbps", 0.0).unwrap().human, "1w 2d 6h");
        assert_eq!(transfer_time("125 MB", "1 Gbps", 0.0).unwrap().human, "1 s");
        // 59.992 s rounds up to a minute rather than showing "60 s".
        assert_eq!(transfer_time("7499 kB", "1 Mbps", 0.0).unwrap().human, "1m");
        // Longer than a u64 of seconds: the duration saturates instead of overflowing.
        let forever = transfer_time("16 EB", "1 bps", 99.0).unwrap();
        assert_eq!(forever.human, format_duration_short(u64::MAX));
        assert!(forever.seconds.is_finite());
    }

    #[test]
    fn transfer_time_rejections() {
        let message = |size: &str, rate: &str, overhead: f64| transfer_time(size, rate, overhead).unwrap_err().to_string();
        assert_eq!(message("1 GB", "0", 0.0), "invalid rate `0`: nothing ever finishes over a link of 0 bit/s");
        assert_eq!(
            message("1 GB", "1G", 100.0),
            "invalid overhead `100`: the protocol overhead is a percentage from 0 up to (not including) 100"
        );
        assert!(message("1 GB", "1G", -1.0).contains("percentage from 0"));
        assert!(message("1 GB", "1G", f64::NAN).contains("percentage from 0"));
        assert!(message("1 GB", "1G", f64::INFINITY).contains("percentage from 0"));
        assert!(message("big", "1G", 0.0).starts_with("invalid size"));
        assert!(message("1 GB", "quick", 0.0).starts_with("invalid rate"));
    }
}
