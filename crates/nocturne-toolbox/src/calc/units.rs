//! Converters: sizes and rates, transfer time, durations, optical power, and
//! MTU with the encapsulations stacked on a link.
//!
//! Parsing is `nocturne_tools::convert` throughout — it reads `700 MB`,
//! `4.7GiB`, `100Mb/s`, `2w3d` and `12:34:56` and knows which `b` is a bit.
//! Two things are this module's own and are marked where they are:
//!
//! * the **factors used to emit** a value in a unit the caller asked for. The
//!   engine parses every unit but formats only in the one it chooses, so
//!   turning bytes back into MiB needs the divisor, and that is arithmetic
//!   rather than data.
//! * **dBm and milliwatts**, which the engine does not do at all (and whose
//!   values are negative, which its number parser refuses by design). A
//!   missing engine function is listed in the report; in the meantime this is
//!   three lines of logarithm and an optical budget is a daily question.

use nocturne_app::{App, AppError, AppResult, Router};
use nocturne_tools::convert;
use serde::{Deserialize, Serialize};

use crate::errors::tool;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ConvertParams {
    /// The amount, with or without its unit: `700 MB`, `4.7GiB`, `9600`, `-12`.
    pub value: String,
    /// The unit of `value`, when the value does not carry it.
    pub from: Option<String>,
    /// The unit to answer in. Left out, every unit of the family is returned.
    pub to: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct TransferParams {
    pub size: String,
    pub rate: String,
    /// Protocol overhead as a percentage of the link rate (TCP/IP ≈ 3–6 %).
    pub overhead_pct: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct MtuParams {
    pub link_mtu: u32,
    /// Stacked outermost first: `["ipsec-esp-tunnel", "gre"]`.
    pub encapsulations: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Conversion {
    /// What was parsed, as it was handed to the engine.
    pub input: String,
    /// `size`, `rate`, `duration` or `power`.
    pub family: &'static str,
    /// The amount in the family's own unit.
    pub canonical: f64,
    pub canonical_unit: &'static str,
    /// The engine's own rendering of it (`667.57 MiB`, `1.25 Gbps`, `2w 3d`).
    pub human: String,
    /// The amount in `to`, when a unit was asked for.
    pub value: Option<f64>,
    pub unit: Option<String>,
    /// The amount in every unit of the family.
    pub forms: Vec<Form>,
}

#[derive(Debug, Serialize)]
pub struct Form {
    pub unit: &'static str,
    pub value: f64,
}

/// A unit this module can emit, and how many canonical units it is worth.
struct Unit {
    name: &'static str,
    family: &'static str,
    /// One of these is this many canonical units (bytes, bit/s, seconds, mW).
    factor: f64,
    /// Accepted spellings, lower-cased, besides `name`.
    aliases: &'static [&'static str],
}

/// The emitting side of the converters. Parsing is the engine's.
const UNITS: &[Unit] = &[
    // Sizes: canonical unit is the byte.
    unit("B", "size", 1.0, &["byte", "bytes"]),
    unit("kB", "size", 1e3, &["kilobyte", "kilobytes"]),
    unit("MB", "size", 1e6, &["megabyte", "megabytes"]),
    unit("GB", "size", 1e9, &["gigabyte", "gigabytes"]),
    unit("TB", "size", 1e12, &["terabyte", "terabytes"]),
    unit("PB", "size", 1e15, &[]),
    unit("KiB", "size", 1024.0, &[]),
    unit("MiB", "size", 1_048_576.0, &[]),
    unit("GiB", "size", 1_073_741_824.0, &[]),
    unit("TiB", "size", 1_099_511_627_776.0, &[]),
    unit("bit", "size", 0.125, &["bits", "b"]),
    unit("kbit", "size", 125.0, &["kb"]),
    unit("Mbit", "size", 125_000.0, &["mb"]),
    unit("Gbit", "size", 1.25e8, &["gb"]),
    // Rates: canonical unit is the bit per second.
    unit("bps", "rate", 1.0, &["bit/s", "bits/s"]),
    unit("kbps", "rate", 1e3, &["kbit/s", "kb/s"]),
    unit("Mbps", "rate", 1e6, &["mbit/s", "mb/s"]),
    unit("Gbps", "rate", 1e9, &["gbit/s", "gb/s"]),
    unit("Tbps", "rate", 1e12, &["tbit/s"]),
    unit("B/s", "rate", 8.0, &["bytes/s"]),
    unit("kB/s", "rate", 8e3, &[]),
    unit("MB/s", "rate", 8e6, &[]),
    unit("GB/s", "rate", 8e9, &[]),
    // Durations: canonical unit is the second.
    unit("ms", "duration", 0.001, &["millisecond", "milliseconds"]),
    unit("s", "duration", 1.0, &["sec", "secs", "second", "seconds"]),
    unit("min", "duration", 60.0, &["m", "minute", "minutes"]),
    unit("h", "duration", 3600.0, &["hour", "hours"]),
    unit("d", "duration", 86_400.0, &["day", "days"]),
    unit("w", "duration", 604_800.0, &["week", "weeks"]),
    // Optical power: canonical unit is the milliwatt. dBm is not a factor of
    // anything (it is a logarithm) and is handled on its own.
    unit("mW", "power", 1.0, &["milliwatt", "milliwatts"]),
    unit("uW", "power", 0.001, &["µw", "microwatt"]),
    unit("W", "power", 1000.0, &["watt", "watts"]),
    unit("nW", "power", 1e-6, &[]),
];

const DBM: &str = "dBm";

const fn unit(
    name: &'static str,
    family: &'static str,
    factor: f64,
    aliases: &'static [&'static str],
) -> Unit {
    Unit {
        name,
        family,
        factor,
        aliases,
    }
}

/// The unit a name means.
///
/// Spelling is matched exactly first and only then case-insensitively,
/// because in `MB/s` and `Mb/s` the case of the `b` is the whole difference
/// between bytes and bits — the same distinction the engine's parser makes.
fn find_unit(name: &str) -> Option<&'static Unit> {
    let text = name.trim();
    UNITS
        .iter()
        .find(|unit| unit.name == text || unit.aliases.contains(&text))
        .or_else(|| {
            let lower = text.to_ascii_lowercase();
            UNITS.iter().find(|unit| {
                unit.name.to_ascii_lowercase() == lower
                    || unit.aliases.iter().any(|alias| *alias == lower)
            })
        })
}

/// The family a `from` / `to` unit belongs to (`dBm` counts as power).
fn family_of(name: &str) -> Option<&'static str> {
    if name.trim().eq_ignore_ascii_case(DBM) {
        return Some("power");
    }
    find_unit(name).map(|unit| unit.family)
}

/// Parse the amount of a power value: the one family the engine cannot read,
/// because an optical level is negative and its number parser refuses that.
fn parse_power(value: &str, from: Option<&str>) -> AppResult<f64> {
    let text = value.trim();
    let digits: String = text
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'))
        .collect();
    let spelt_unit = text[digits.len()..].trim();
    let unit = if spelt_unit.is_empty() {
        from.unwrap_or(DBM).trim()
    } else {
        spelt_unit
    };
    let amount: f64 = digits.parse().map_err(|_| {
        AppError::invalid(format!(
            "`{text}` does not start with a number; write an optical level as `-12 dBm` or a power as `0.063 mW`"
        ))
    })?;
    if unit.eq_ignore_ascii_case(DBM) {
        // dBm is ten times the base-ten logarithm of the power in milliwatts.
        return Ok(10f64.powf(amount / 10.0));
    }
    let unit = find_unit(unit)
        .filter(|unit| unit.family == "power")
        .ok_or_else(|| {
            AppError::invalid(format!(
                "`{unit}` is not a power unit; use dBm, mW, uW, nW or W"
            ))
        })?;
    Ok(amount * unit.factor)
}

/// The refusal for a unit this module does not know, with the list.
fn unknown_unit(named: &str) -> AppError {
    AppError::invalid(format!(
        "`{named}` is not a unit this converter knows. Sizes: B, kB … PB, KiB … TiB, bit, Mbit. Rates: bps … Tbps, MB/s. Durations: ms, s, min, h, d, w. Power: dBm, mW, uW, W."
    ))
}

fn convert(params: ConvertParams) -> AppResult<Conversion> {
    let from = params
        .from
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let to = params
        .to
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    if params.value.trim().is_empty() {
        return Err(AppError::invalid("give the amount to convert in `value`"));
    }
    // The engine reads the unit out of the text, so `from` is only appended
    // when the value does not already carry one: `90m` with `from: "min"` is
    // 90 minutes, not `90m min`, which is nothing at all.
    let value = params.value.trim();
    let carries_unit = value
        .chars()
        .last()
        .is_some_and(|last| !last.is_ascii_digit() && last != '.');
    let input = match from {
        Some(unit) if !carries_unit => format!("{value} {unit}"),
        _ => value.to_string(),
    };

    // The unit to answer in has to be one this module can emit; the unit the
    // value is *in* only has to be one the engine's parser reads, which is a
    // longer list.
    if let Some(named) = to.filter(|name| family_of(name).is_none()) {
        return Err(unknown_unit(named));
    }
    let family = match (to.and_then(family_of), from.and_then(family_of)) {
        (Some(family), _) => family,
        (None, Some(family)) => family,
        (None, None) => {
            if let Some(named) = from {
                return Err(unknown_unit(named));
            }
            // Nothing named a family, so the text has to decide. The three
            // parsers accept disjoint units, with one exception: a bare
            // prefix (`90m`) is a size to the size parser, so sizes come
            // first and `to`/`from` exist for when that is not what was meant.
            if convert::parse_size_bytes(&input).is_ok() {
                "size"
            } else if convert::parse_rate_bps(&input).is_ok() {
                "rate"
            } else if convert::parse_duration_s(&input).is_some() {
                "duration"
            } else {
                return Err(tool(
                    convert::parse_size_bytes(&input).expect_err("not a size"),
                ));
            }
        }
    };

    let canonical = match family {
        "size" => convert::parse_size_bytes(&input).map_err(tool)? as f64,
        "rate" => convert::parse_rate_bps(&input).map_err(tool)? as f64,
        "duration" => convert::parse_duration_s(&input).ok_or_else(|| {
            AppError::invalid(format!(
                "`{input}` is not a duration; write `2w3d`, `90m`, `1:30:00`, `4 hours` or a number of seconds"
            ))
        })? as f64,
        _ => parse_power(&params.value, from)?,
    };

    let (canonical_unit, human) = match family {
        "size" => ("B", convert::format_bytes(canonical)),
        "rate" => ("bps", convert::format_rate(canonical)),
        "duration" => ("s", convert::format_duration(canonical as u64)),
        _ => (
            "mW",
            format!("{:.2} dBm", 10.0 * canonical.max(f64::MIN_POSITIVE).log10()),
        ),
    };

    let mut forms: Vec<Form> = UNITS
        .iter()
        .filter(|unit| unit.family == family)
        .map(|unit| Form {
            unit: unit.name,
            value: canonical / unit.factor,
        })
        .collect();
    if family == "power" {
        forms.push(Form {
            unit: DBM,
            value: 10.0 * canonical.max(f64::MIN_POSITIVE).log10(),
        });
    }

    let value = match to {
        Some(name) if name.eq_ignore_ascii_case(DBM) => {
            Some(10.0 * canonical.max(f64::MIN_POSITIVE).log10())
        }
        Some(name) => Some(canonical / find_unit(name).expect("the family came from it").factor),
        None => None,
    };
    Ok(Conversion {
        input,
        family,
        canonical,
        canonical_unit,
        human,
        value,
        unit: to.map(str::to_string),
        forms,
    })
}

pub fn register(router: &mut Router) {
    router.add(
        "tools.convert",
        |_: App, params: ConvertParams| async move { convert(params) },
    );
    router.add(
        "tools.transfer_time",
        |_: App, params: TransferParams| async move {
            let overhead = params.overhead_pct.unwrap_or(0.0);
            if !(0.0..100.0).contains(&overhead) {
                return Err(AppError::invalid(
                    "the overhead is a percentage of the link rate, from 0 to under 100",
                ));
            }
            convert::transfer_time(&params.size, &params.rate, overhead).map_err(tool)
        },
    );
    router.add("tools.mtu", |_: App, params: MtuParams| async move {
        convert::mtu(params.link_mtu, &params.encapsulations).map_err(tool)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn converted(value: &str, from: Option<&str>, to: Option<&str>) -> Conversion {
        convert(ConvertParams {
            value: value.into(),
            from: from.map(str::to_string),
            to: to.map(str::to_string),
        })
        .unwrap()
    }

    #[test]
    fn sizes_and_rates_answer_in_the_unit_that_was_asked_for() {
        // 700 MB is 700_000_000 bytes, which is 667.6171875 MiB exactly.
        let size = converted("700", Some("MB"), Some("MiB"));
        assert_eq!((size.family, size.canonical), ("size", 700_000_000.0));
        assert_eq!(size.value.unwrap(), 667.572021484375);
        assert_eq!(size.unit.as_deref(), Some("MiB"));
        assert_eq!(size.human, "667.57 MiB");

        // 1 Gbps is 125 MB/s.
        let rate = converted("1 Gbps", None, Some("MB/s"));
        assert_eq!((rate.family, rate.canonical), ("rate", 1e9));
        assert_eq!(rate.value.unwrap(), 125.0);
        // The case of the `b` is the difference between bytes and bits.
        assert_eq!(
            converted("1 Gbps", None, Some("Mb/s")).value.unwrap(),
            1000.0
        );

        // A duration in, a duration out: 90 minutes is 1.5 hours.
        let duration = converted("90m", Some("min"), Some("h"));
        assert_eq!((duration.family, duration.canonical), ("duration", 5400.0));
        assert_eq!(duration.value.unwrap(), 1.5);
        assert_eq!(duration.human, "1h 30m");
    }

    #[test]
    fn optical_power_crosses_the_logarithm_in_both_directions() {
        // -3 dBm is 0.501 mW; 1 mW is 0 dBm, and 0.063 mW is -12.0 dBm.
        let to_mw = converted("-3", Some("dBm"), Some("mW"));
        assert_eq!((to_mw.family, to_mw.canonical_unit), ("power", "mW"));
        assert!((to_mw.value.unwrap() - 0.5011872).abs() < 1e-6);

        let to_dbm = converted("1 mW", None, Some("dBm"));
        assert_eq!(to_dbm.value.unwrap(), 0.0);

        let level = converted("0.0631 mW", None, Some("dBm"));
        assert!((level.value.unwrap() + 12.0).abs() < 0.01, "{level:?}");

        let refused = convert(ConvertParams {
            value: "warm".into(),
            from: Some("dBm".into()),
            to: None,
        })
        .unwrap_err();
        assert_eq!(refused.code, "invalid");
        assert!(refused.message.contains("dBm"), "{}", refused.message);
    }

    #[test]
    fn a_unit_nobody_uses_is_refused_with_the_list_of_ones_that_work() {
        let refused = convert(ConvertParams {
            value: "5".into(),
            from: None,
            to: Some("furlongs".into()),
        })
        .unwrap_err();
        assert_eq!(refused.code, "invalid");
        assert!(refused.message.contains("Rates:"), "{}", refused.message);

        // A unit the value is in that this module cannot emit is still an
        // error rather than a panic, even when the other one is known.
        let one_known = convert(ConvertParams {
            value: "5".into(),
            from: Some("MB".into()),
            to: Some("furlongs".into()),
        })
        .unwrap_err();
        assert_eq!(one_known.code, "invalid");
        assert_eq!(
            convert(ConvertParams {
                value: "5".into(),
                from: Some("furlongs".into()),
                to: None,
            })
            .unwrap_err()
            .code,
            "invalid"
        );

        assert_eq!(
            convert(ConvertParams::default()).unwrap_err().code,
            "invalid"
        );
    }

    #[test]
    fn without_units_the_text_decides_and_says_which_family_it_chose() {
        assert_eq!(converted("4.7GiB", None, None).family, "size");
        assert_eq!(converted("100Mb/s", None, None).family, "rate");
        assert_eq!(converted("1:30:00", None, None).family, "duration");
        // Every form of the family comes back, so a UI needs one call.
        let forms = converted("1 GB", None, None).forms;
        assert_eq!(
            forms
                .iter()
                .find(|form| form.unit == "MiB")
                .map(|form| form.value),
            Some(953.67431640625)
        );
    }
}
