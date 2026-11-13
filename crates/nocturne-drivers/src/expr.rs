//! The expression language profiles use to map parsed rows to the model.
//!
//! Profiles are data, but mapping `LINK_STATUS = "administratively down"` to
//! `admin_up = false` takes a little logic. Rather than invent a DSL, mapping
//! values are [MiniJinja](https://docs.rs/minijinja) expressions — the Jinja2
//! expression syntax network engineers already know from Ansible — evaluated
//! with the parsed row as context, plus a handful of networking filters:
//!
//! | filter | example | result |
//! |---|---|---|
//! | `duration` | `"2 weeks, 3 days" \| duration` | `1468800` (seconds) |
//! | `bps` | `"10 Gb/s" \| bps` | `10000000000` |
//! | `num` | `"1,234" \| num` | `1234` (none when not a number) |
//! | `mac` | `"aabb.ccdd.eeff" \| mac` | `"aa:bb:cc:dd:ee:ff"` |
//! | `ifname` | `"Gi1/0/1" \| ifname` | `"GigabitEthernet1/0/1"` |
//! | `cidr(mask)` | `"10.0.0.1" \| cidr("255.255.255.0")` | `"10.0.0.1/24"` |
//! | `regex_search(p)` | `"up (connected)" \| regex_search("\\((\\w+)\\)")` | `"connected"` |
//! | `regex_replace(p, r)` | `"Gi0/1," \| regex_replace(",$", "")` | `"Gi0/1"` |
//!
//! and the test `x is matching("^up")`.

use std::net::Ipv4Addr;
use std::sync::LazyLock;

use minijinja::value::{Serde, Value};
use minijinja::{Environment, Error, ErrorKind, UndefinedBehavior};
use regex::Regex;

use crate::ifname;

pub struct Expressions {
    env: Environment<'static>,
}

impl Default for Expressions {
    fn default() -> Self {
        Expressions::new()
    }
}

impl Expressions {
    pub fn new() -> Expressions {
        let mut env = Environment::new();
        // A field the template did not capture is simply empty, not an error.
        env.set_undefined_behavior(UndefinedBehavior::Chainable);
        // Filters take loose values: a field the parser left empty or absent
        // must flow through as "nothing", never as a type error.
        env.add_filter("duration", |value: Value| {
            parse_duration(&text(&value))
                .map(Value::from)
                .unwrap_or_default()
        });
        env.add_filter("bps", |value: Value| {
            parse_bps(&text(&value))
                .map(Value::from)
                .unwrap_or_default()
        });
        env.add_filter("num", |value: Value| to_number(&value));
        env.add_filter("mac", |value: Value| normalize_mac(&text(&value)));
        env.add_filter("ifname", |value: Value| ifname::long(&text(&value)));
        env.add_filter("cidr", |address: Value, mask: Value| {
            to_cidr(&text(&address), &text(&mask))
        });
        env.add_function("cidr", |address: Value, mask: Value| {
            to_cidr(&text(&address), &text(&mask))
        });
        env.add_filter("regex_search", |value: Value, pattern: String| {
            regex_search(&text(&value), &pattern)
        });
        env.add_filter(
            "regex_replace",
            |value: Value, pattern: String, replacement: String| {
                regex_replace(&text(&value), &pattern, &replacement)
            },
        );
        env.add_test(
            "matching",
            |value: Value, pattern: String| -> Result<bool, Error> {
                Ok(compile(&pattern)?.is_match(&text(&value)))
            },
        );
        Expressions { env }
    }

    /// Check that an expression parses.
    pub fn check(&self, expression: &str) -> Result<(), String> {
        self.env
            .compile_expression(expression)
            .map(|_| ())
            .map_err(|err| describe(&err))
    }

    /// Evaluate against a JSON context (usually one parsed row).
    pub fn eval(
        &self,
        expression: &str,
        context: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let compiled = self
            .env
            .compile_expression(expression)
            .map_err(|err| describe(&err))?;
        let value = compiled
            .eval(Value::from(Serde(context)))
            .map_err(|err| describe(&err))?;
        serde_json::to_value(&value).map_err(|err| err.to_string())
    }

    /// Evaluate and decide truthiness the Jinja way (empty string, 0, none, [] are false).
    pub fn truthy(&self, expression: &str, context: &serde_json::Value) -> Result<bool, String> {
        let compiled = self
            .env
            .compile_expression(expression)
            .map_err(|err| describe(&err))?;
        let value = compiled
            .eval(Value::from(Serde(context)))
            .map_err(|err| describe(&err))?;
        Ok(value.is_true())
    }

    /// Render a template string (`show interfaces {{ interface }}`).
    pub fn render(&self, template: &str, context: &serde_json::Value) -> Result<String, String> {
        self.env
            .render_str(template, Value::from(Serde(context)))
            .map_err(|err| describe(&err))
    }
}

fn describe(err: &Error) -> String {
    match err.detail() {
        Some(detail) => format!("{}: {detail}", err.kind()),
        None => err.kind().to_string(),
    }
}

fn compile(pattern: &str) -> Result<Regex, Error> {
    Regex::new(pattern).map_err(|err| {
        Error::new(
            ErrorKind::InvalidOperation,
            format!("invalid regular expression: {err}"),
        )
    })
}

/// A value as text: nothing for undefined/none, the string itself, or its display form.
fn text(value: &Value) -> String {
    if value.is_undefined() || value.is_none() {
        String::new()
    } else if let Some(text) = value.as_str() {
        text.to_string()
    } else {
        value.to_string()
    }
}

fn regex_search(text: &str, pattern: &str) -> Result<Value, Error> {
    let regex = compile(pattern)?;
    Ok(match regex.captures(text) {
        Some(caps) => caps
            .get(1)
            .or_else(|| caps.get(0))
            .map(|found| Value::from(found.as_str()))
            .unwrap_or_default(),
        None => Value::default(),
    })
}

fn regex_replace(text: &str, pattern: &str, replacement: &str) -> Result<String, Error> {
    Ok(compile(pattern)?
        .replace_all(text, replacement)
        .into_owned())
}

/// Lenient number conversion: `"1,234"` → 1234, `"17%"` → 17, `"n/a"` → none.
fn to_number(value: &Value) -> Value {
    if value.is_number() {
        return value.clone();
    }
    let Some(text) = value.as_str() else {
        return Value::default();
    };
    let cleaned: String = text
        .trim()
        .trim_end_matches('%')
        .chars()
        .filter(|c| *c != ',' && *c != '_')
        .collect();
    if let Ok(integer) = cleaned.parse::<i64>() {
        Value::from(integer)
    } else if let Ok(float) = cleaned.parse::<f64>() {
        Value::from(float)
    } else {
        Value::default()
    }
}

static DURATION_UNIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\d+)\s*(years?|yrs?|y|weeks?|wks?|w|days?|d|hours?|hrs?|h|minutes?|mins?|m|seconds?|secs?|s)(?:\(s\))?(?:[^a-z]|$)")
        .expect("duration unit pattern")
});
static DURATION_CLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^\d:])(\d{1,3}):(\d{2})(?::(\d{2}))?(?:\.\d+)?(?:[^\d:]|$)")
        .expect("duration clock pattern")
});

/// Seconds from the many ways devices print an uptime:
/// `2 weeks, 3 days, 4 hours, 5 minutes`, `0 day(s), 3 hour(s)`, `3w2d1h4m5s`,
/// `12 days, 3:04`, `1d02:03:04`, `(123456) 0:20:34.56`, or a bare number of seconds.
pub fn parse_duration(text: &str) -> Option<u64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(seconds) = text.parse::<u64>() {
        return Some(seconds);
    }
    let mut total = 0u64;
    let mut matched = false;
    // Units may be packed together ("3w2d1h"), so scan with a moving start.
    let mut start = 0;
    while let Some(caps) = DURATION_UNIT.captures_at(text, start) {
        let (Some(number), Some(unit)) = (caps.get(1), caps.get(2)) else {
            break;
        };
        let amount = number.as_str().parse::<u64>().ok()?;
        let factor = match unit.as_str().to_ascii_lowercase().chars().next()? {
            'y' => 365 * 86_400,
            'w' => 7 * 86_400,
            'd' => 86_400,
            'h' => 3_600,
            'm' => 60,
            _ => 1,
        };
        total = total.checked_add(amount.checked_mul(factor)?)?;
        matched = true;
        start = unit.end();
    }
    if let Some(caps) = DURATION_CLOCK.captures(&text[start.min(text.len())..]) {
        let hours = caps[1].parse::<u64>().ok()?;
        let minutes = caps[2].parse::<u64>().ok()?;
        let clock = match caps.get(3) {
            Some(seconds) => hours * 3_600 + minutes * 60 + seconds.as_str().parse::<u64>().ok()?,
            // "3:04" after a day count means hours:minutes.
            None => hours * 3_600 + minutes * 60,
        };
        total = total.checked_add(clock)?;
        matched = true;
    }
    matched.then_some(total)
}

static RATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\d+(?:\.\d+)?)\s*([kmgt])?(?:i?b(?:it)?s?(?:/s(?:ec)?|ps)?)?")
        .expect("rate pattern")
});

/// Bits per second from `1000Mb/s`, `10 Gbps`, `100000 Kbit`, `1G`, `2.5Gb/s`
/// or a bare number (taken as bits per second).
pub fn parse_bps(text: &str) -> Option<u64> {
    let caps = RATE.captures(text.trim())?;
    let amount: f64 = caps[1].parse().ok()?;
    let factor = match caps.get(2).map(|unit| unit.as_str().to_ascii_lowercase()) {
        Some(unit) if unit == "k" => 1e3,
        Some(unit) if unit == "m" => 1e6,
        Some(unit) if unit == "g" => 1e9,
        Some(unit) if unit == "t" => 1e12,
        _ => 1.0,
    };
    let bps = amount * factor;
    (bps.is_finite() && (0.0..1e18).contains(&bps)).then_some(bps as u64)
}

/// `aabb.ccdd.eeff`, `AA-BB-CC-DD-EE-FF`, `aabbccddeeff` → `aa:bb:cc:dd:ee:ff`.
/// Anything that is not twelve hex digits is returned trimmed but otherwise untouched.
pub fn normalize_mac(text: &str) -> String {
    let digits: String = text.chars().filter(char::is_ascii_hexdigit).collect();
    let separators_only = text
        .chars()
        .all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '-' | '.' | ' '));
    if digits.len() != 12 || !separators_only {
        return text.trim().to_string();
    }
    let lower = digits.to_ascii_lowercase();
    (0..6)
        .map(|index| &lower[index * 2..index * 2 + 2])
        .collect::<Vec<_>>()
        .join(":")
}

/// Combine an address with a netmask (`255.255.255.0`), a prefix length (`24`, `/24`)
/// into CIDR form. An address that already has a prefix is returned as is.
pub fn to_cidr(address: &str, mask: &str) -> String {
    let address = address.trim();
    let mask = mask.trim().trim_start_matches('/');
    if address.is_empty() {
        return String::new();
    }
    if address.contains('/') || mask.is_empty() {
        return address.to_string();
    }
    if let Ok(prefix) = mask.parse::<u8>() {
        return format!("{address}/{prefix}");
    }
    match mask.parse::<Ipv4Addr>() {
        Ok(netmask) => {
            let bits = u32::from(netmask);
            // Only contiguous masks have a prefix length.
            if bits.leading_ones() + bits.trailing_zeros() == 32 {
                format!("{address}/{}", bits.leading_ones())
            } else {
                address.to_string()
            }
        }
        Err(_) => address.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn eval(expression: &str, row: serde_json::Value) -> serde_json::Value {
        Expressions::new().eval(expression, &row).unwrap()
    }

    #[test]
    fn maps_a_parsed_interface_row() {
        let row = json!({
            "INTERFACE": "GigabitEthernet0/1",
            "LINK_STATUS": "administratively down",
            "PROTOCOL_STATUS": "down",
            "IP_ADDRESS": "10.0.0.1",
            "PREFIX_LENGTH": "24",
            "SPEED": "1000Mb/s",
            "MAC_ADDRESS": "aabb.cc00.0100",
            "DESCRIPTION": "",
            "INPUT_ERRORS": "1,204",
        });
        assert_eq!(eval("INTERFACE", row.clone()), json!("GigabitEthernet0/1"));
        assert_eq!(
            eval("'administratively' not in LINK_STATUS", row.clone()),
            json!(false)
        );
        assert_eq!(
            eval("PROTOCOL_STATUS is startingwith('up')", row.clone()),
            json!(false)
        );
        assert_eq!(
            eval(
                "[cidr(IP_ADDRESS, PREFIX_LENGTH)] if IP_ADDRESS else []",
                row.clone()
            ),
            json!(["10.0.0.1/24"])
        );
        assert_eq!(eval("SPEED | bps", row.clone()), json!(1_000_000_000u64));
        assert_eq!(
            eval("MAC_ADDRESS | mac", row.clone()),
            json!("aa:bb:cc:00:01:00")
        );
        assert_eq!(eval("INPUT_ERRORS | num", row.clone()), json!(1204));
        assert_eq!(
            eval("DESCRIPTION or MISSING_FIELD or 'none'", row.clone()),
            json!("none")
        );
        assert_eq!(eval("MISSING_FIELD", row.clone()), json!(null));
        // Absent fields pass through every filter as "nothing".
        assert_eq!(eval("MISSING_FIELD | duration", row.clone()), json!(null));
        assert_eq!(eval("MISSING_FIELD | bps", row.clone()), json!(null));
        assert_eq!(eval("MISSING_FIELD | num", row.clone()), json!(null));
        assert_eq!(eval("MISSING_FIELD | mac", row.clone()), json!(""));
        assert_eq!(
            eval("cidr(MISSING_FIELD, PREFIX_LENGTH)", row.clone()),
            json!("")
        );
        assert_eq!(eval("MISSING_FIELD is matching('x')", row), json!(false));
    }

    #[test]
    fn list_values_and_regex_helpers() {
        let row = json!({"HARDWARE": ["WS-C3750X-48P", "WS-C3750X-24"], "STATUS": "up (connected)", "PORTS": "Gi0/1, Gi0/2,"});
        assert_eq!(
            eval("HARDWARE | first", row.clone()),
            json!("WS-C3750X-48P")
        );
        assert_eq!(
            eval("HARDWARE | join(', ')", row.clone()),
            json!("WS-C3750X-48P, WS-C3750X-24")
        );
        assert_eq!(
            eval("STATUS | regex_search('\\\\((\\\\w+)\\\\)')", row.clone()),
            json!("connected")
        );
        assert_eq!(
            eval("STATUS | regex_search('nomatch')", row.clone()),
            json!(null)
        );
        assert_eq!(
            eval("PORTS | regex_replace(',\\\\s*$', '')", row.clone()),
            json!("Gi0/1, Gi0/2")
        );
        assert_eq!(eval("STATUS is matching('^up')", row.clone()), json!(true));
        assert_eq!(
            eval("'Gi1/0/3' | ifname", row),
            json!("GigabitEthernet1/0/3")
        );
    }

    #[test]
    fn durations_in_every_dialect() {
        for (text, seconds) in [
            (
                "2 weeks, 3 days, 4 hours, 5 minutes",
                2 * 604_800 + 3 * 86_400 + 4 * 3_600 + 5 * 60,
            ),
            (
                "1 year, 2 weeks, 1 day, 3 hours, 5 minutes",
                31_536_000 + 1_209_600 + 86_400 + 10_800 + 300,
            ),
            ("0 day(s), 3 hour(s), 2 minute(s), 10 second(s)", 10_930),
            ("3w2d1h4m5s", 3 * 604_800 + 2 * 86_400 + 3_600 + 245),
            ("12 days, 3:04", 12 * 86_400 + 3 * 3_600 + 4 * 60),
            ("1d02:03:04", 86_400 + 7_384),
            ("00:20:34", 1_234),
            ("5 minutes", 300),
            ("42", 42),
        ] {
            assert_eq!(parse_duration(text), Some(seconds), "{text}");
        }
        assert_eq!(parse_duration("never"), None);
        assert_eq!(parse_duration(""), None);
    }

    #[test]
    fn rates_macs_and_cidrs() {
        assert_eq!(parse_bps("1000Mb/s"), Some(1_000_000_000));
        assert_eq!(parse_bps("10 Gbps"), Some(10_000_000_000));
        assert_eq!(parse_bps("100000 Kbit"), Some(100_000_000));
        assert_eq!(parse_bps("2.5Gb/s"), Some(2_500_000_000));
        assert_eq!(parse_bps("1G"), Some(1_000_000_000));
        assert_eq!(parse_bps("9600"), Some(9_600));
        assert_eq!(parse_bps("auto"), None);

        assert_eq!(normalize_mac("AABB.CCDD.EEFF"), "aa:bb:cc:dd:ee:ff");
        assert_eq!(normalize_mac("4C-5E-0C-11-22-33"), "4c:5e:0c:11:22:33");
        assert_eq!(normalize_mac("Incomplete"), "Incomplete");

        assert_eq!(to_cidr("10.0.0.1", "255.255.255.0"), "10.0.0.1/24");
        assert_eq!(to_cidr("10.0.0.1", "/30"), "10.0.0.1/30");
        assert_eq!(to_cidr("10.0.0.1/24", "255.255.0.0"), "10.0.0.1/24");
        assert_eq!(to_cidr("10.0.0.1", "255.0.255.0"), "10.0.0.1");
        assert_eq!(to_cidr("", "24"), "");
    }

    #[test]
    fn bad_expressions_are_reported() {
        let expressions = Expressions::new();
        assert!(expressions.check("INTERFACE | upper").is_ok());
        assert!(expressions.check("INTERFACE |").is_err());
        assert!(
            expressions
                .eval("X | regex_search('(')", &json!({"X": "a"}))
                .unwrap_err()
                .contains("regular expression")
        );
        assert_eq!(
            expressions
                .render("show interfaces {{ name }}", &json!({"name": "Gi0/1"}))
                .unwrap(),
            "show interfaces Gi0/1"
        );
        assert!(
            expressions
                .truthy("A and not B", &json!({"A": "x", "B": ""}))
                .unwrap()
        );
    }
}
