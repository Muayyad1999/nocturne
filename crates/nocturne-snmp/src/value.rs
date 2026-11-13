//! SNMP values and variable bindings.

use std::fmt;
use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

use crate::Oid;

/// The value half of a variable binding.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    /// INTEGER / Integer32 (enumerations, signed gauges).
    Integer(i64),
    /// OCTET STRING: text (DisplayString), MAC addresses, bitmaps…
    OctetString(Vec<u8>),
    /// NULL — what requests carry in place of a value.
    Null,
    /// OBJECT IDENTIFIER.
    Oid(Oid),
    /// IpAddress (IPv4 only, by definition).
    IpAddress([u8; 4]),
    /// Counter32: wraps at 2^32.
    Counter32(u32),
    /// Gauge32 / Unsigned32.
    Gauge32(u32),
    /// TimeTicks: hundredths of a second.
    TimeTicks(u32),
    /// Opaque: arbitrary BER wrapped in an octet string (net-snmp floats, for example).
    Opaque(Vec<u8>),
    /// Counter64 (SNMPv2c/v3 only).
    Counter64(u64),
    /// SNMPv2 exception: the agent does not implement this object.
    NoSuchObject,
    /// SNMPv2 exception: the object exists but this instance does not.
    NoSuchInstance,
    /// SNMPv2 exception: GETNEXT/GETBULK ran off the end of the MIB.
    EndOfMibView,
}

impl Value {
    /// The value as an unsigned number: `Integer` (≥ 0), `Counter32`/`Counter64`, `Gauge32`,
    /// `TimeTicks`.
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Value::Integer(v) => u64::try_from(v).ok(),
            Value::Counter32(v) | Value::Gauge32(v) | Value::TimeTicks(v) => Some(u64::from(v)),
            Value::Counter64(v) => Some(v),
            _ => None,
        }
    }

    /// The value as a signed number; a `Counter64` beyond `i64::MAX` yields `None`.
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Value::Integer(v) => Some(v),
            Value::Counter32(v) | Value::Gauge32(v) | Value::TimeTicks(v) => Some(i64::from(v)),
            Value::Counter64(v) => i64::try_from(v).ok(),
            _ => None,
        }
    }

    /// The value as a float: any numeric type, plus net-snmp's `Opaque`-wrapped Float/Double
    /// (UCD `laLoadFloat` and friends).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Integer(v) => Some(*v as f64),
            Value::Counter32(v) | Value::Gauge32(v) | Value::TimeTicks(v) => Some(f64::from(*v)),
            Value::Counter64(v) => Some(*v as f64),
            Value::Opaque(bytes) => opaque_float(bytes),
            _ => None,
        }
    }

    /// `OctetString` as UTF-8 (lossy) if printable, else `None`.
    ///
    /// "Printable" means: no control characters other than tab/CR/LF, and at most a quarter of
    /// the characters undecodable (so a stray Latin-1 letter in a description still reads as
    /// text while MAC addresses and bitmaps do not). Trailing NUL padding is dropped.
    pub fn as_text(&self) -> Option<String> {
        match self {
            Value::OctetString(bytes) => printable_text(bytes),
            _ => None,
        }
    }

    /// The raw bytes of an `OctetString` or `Opaque`.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::OctetString(bytes) | Value::Opaque(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// The inner OID of an `Oid` value.
    pub fn as_oid(&self) -> Option<&Oid> {
        match self {
            Value::Oid(oid) => Some(oid),
            _ => None,
        }
    }

    /// Human rendering: text if printable, MAC `aa:bb:…` for 6 bytes, hex otherwise; ticks as
    /// duration.
    pub fn display(&self) -> String {
        match self {
            Value::Integer(v) => v.to_string(),
            Value::OctetString(bytes) => match printable_text(bytes) {
                Some(text) => text,
                None if bytes.len() == 6 => hex(bytes, ":"),
                None => hex(bytes, " "),
            },
            Value::Null => "null".to_owned(),
            Value::Oid(oid) => oid.to_string(),
            Value::IpAddress(octets) => Ipv4Addr::from(*octets).to_string(),
            Value::Counter32(v) | Value::Gauge32(v) => v.to_string(),
            Value::TimeTicks(ticks) => ticks_as_duration(*ticks),
            Value::Opaque(bytes) => match opaque_float(bytes) {
                Some(number) => number.to_string(),
                None => hex(bytes, " "),
            },
            Value::Counter64(v) => v.to_string(),
            Value::NoSuchObject => "noSuchObject".to_owned(),
            Value::NoSuchInstance => "noSuchInstance".to_owned(),
            Value::EndOfMibView => "endOfMibView".to_owned(),
        }
    }

    /// `true` for the SNMPv2 exceptions `NoSuchObject`, `NoSuchInstance` and `EndOfMibView`.
    pub fn is_exception(&self) -> bool {
        matches!(
            self,
            Value::NoSuchObject | Value::NoSuchInstance | Value::EndOfMibView
        )
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

impl From<&str> for Value {
    fn from(text: &str) -> Self {
        Value::OctetString(text.as_bytes().to_vec())
    }
}

impl From<String> for Value {
    fn from(text: String) -> Self {
        Value::OctetString(text.into_bytes())
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Integer(i64::from(v))
    }
}

impl From<Oid> for Value {
    fn from(oid: Oid) -> Self {
        Value::Oid(oid)
    }
}

impl From<Ipv4Addr> for Value {
    fn from(addr: Ipv4Addr) -> Self {
        Value::IpAddress(addr.octets())
    }
}

/// One variable binding: an object instance and its value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VarBind {
    pub oid: Oid,
    pub value: Value,
}

impl VarBind {
    pub fn new(oid: Oid, value: Value) -> Self {
        VarBind { oid, value }
    }
}

fn printable_text(bytes: &[u8]) -> Option<String> {
    // Plenty of agents NUL-terminate or NUL-pad their strings.
    let end = bytes.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    let trimmed = &bytes[..end];
    if trimmed.is_empty() {
        // An empty string is text; a run of NULs is binary data.
        return bytes.is_empty().then(String::new);
    }

    let text = String::from_utf8_lossy(trimmed);
    let mut total = 0usize;
    let mut undecodable = 0usize;
    for c in text.chars() {
        total += 1;
        if c == char::REPLACEMENT_CHARACTER {
            undecodable += 1;
        } else if c.is_control() && !matches!(c, '\t' | '\n' | '\r') {
            return None;
        }
    }
    (undecodable * 4 <= total).then(|| text.into_owned())
}

fn hex(bytes: &[u8], separator: &str) -> String {
    let mut out = String::with_capacity(bytes.len() * (2 + separator.len()));
    for (i, byte) in bytes.iter().enumerate() {
        if i > 0 {
            out.push_str(separator);
        }
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// `3d 4h 5m 6s`, dropping leading zero units; below one minute the hundredths are kept.
fn ticks_as_duration(ticks: u32) -> String {
    let hundredths = ticks % 100;
    let total_seconds = ticks / 100;
    let seconds = total_seconds % 60;
    let minutes = total_seconds / 60 % 60;
    let hours = total_seconds / 3600 % 24;
    let days = total_seconds / 86_400;
    if days > 0 {
        format!("{days}d {hours}h {minutes}m {seconds}s")
    } else if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}.{hundredths:02}s")
    }
}

/// net-snmp wraps IEEE floats in `Opaque` as `9f 78 04 <f32>` and doubles as `9f 79 08 <f64>`.
fn opaque_float(bytes: &[u8]) -> Option<f64> {
    match bytes {
        [0x9f, 0x78, 0x04, rest @ ..] => {
            let raw: [u8; 4] = rest.try_into().ok()?;
            Some(f64::from(f32::from_be_bytes(raw)))
        }
        [0x9f, 0x79, 0x08, rest @ ..] => {
            let raw: [u8; 8] = rest.try_into().ok()?;
            Some(f64::from_be_bytes(raw))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn octets(bytes: &[u8]) -> Value {
        Value::OctetString(bytes.to_vec())
    }

    #[test]
    fn numeric_accessors() {
        assert_eq!(Value::Integer(42).as_u64(), Some(42));
        assert_eq!(Value::Integer(-1).as_u64(), None);
        assert_eq!(Value::Integer(-1).as_i64(), Some(-1));
        assert_eq!(Value::Counter32(u32::MAX).as_u64(), Some(4_294_967_295));
        assert_eq!(Value::Gauge32(7).as_i64(), Some(7));
        assert_eq!(Value::TimeTicks(100).as_u64(), Some(100));
        assert_eq!(Value::Counter64(u64::MAX).as_u64(), Some(u64::MAX));
        assert_eq!(Value::Counter64(u64::MAX).as_i64(), None);
        assert_eq!(Value::Counter64(5).as_i64(), Some(5));
        assert_eq!(octets(b"12").as_u64(), None);
        assert_eq!(Value::Null.as_i64(), None);
        assert_eq!(Value::NoSuchInstance.as_u64(), None);
        assert_eq!(Value::Gauge32(3).as_f64(), Some(3.0));
    }

    #[test]
    fn text_detection() {
        assert_eq!(octets(b"Linux router 6.1").as_text().as_deref(), Some("Linux router 6.1"));
        assert_eq!(octets(b"").as_text().as_deref(), Some(""));
        assert_eq!(octets(b"two\r\nlines\t!").as_text().as_deref(), Some("two\r\nlines\t!"));
        assert_eq!(octets("Zürich — core".as_bytes()).as_text().as_deref(), Some("Zürich — core"));
        // NUL-terminated strings are still strings.
        assert_eq!(octets(b"eth0\0\0").as_text().as_deref(), Some("eth0"));
        // A lone Latin-1 letter is tolerated (lossy), binary data is not.
        assert_eq!(
            octets(b"B\xfcro 2. Stock").as_text().as_deref(),
            Some("B\u{fffd}ro 2. Stock")
        );
        assert_eq!(octets(&[0x00, 0x1b, 0x21, 0x3c, 0x9d, 0xf8]).as_text(), None);
        assert_eq!(octets(&[0xa4, 0xb1, 0xc2, 0xd3, 0xe4, 0xf5]).as_text(), None);
        assert_eq!(octets(&[0xc0, 0xa8, 0x80, 0xfe]).as_text(), None);
        assert_eq!(octets(&[0, 0, 0, 0, 0, 0]).as_text(), None);
        assert_eq!(octets(b"bell\x07").as_text(), None);
        assert_eq!(Value::Integer(1).as_text(), None);
        assert_eq!(Value::Opaque(b"text".to_vec()).as_text(), None);
    }

    #[test]
    fn display_renders_for_humans() {
        assert_eq!(Value::Integer(-7).display(), "-7");
        assert_eq!(octets(b"core-sw1").display(), "core-sw1");
        assert_eq!(
            octets(&[0x00, 0x1b, 0x21, 0x3c, 0x9d, 0xf8]).display(),
            "00:1b:21:3c:9d:f8"
        );
        assert_eq!(octets(&[0, 0, 0, 0, 0, 0]).display(), "00:00:00:00:00:00");
        assert_eq!(octets(&[0x80, 0x00, 0x1f, 0x88]).display(), "80 00 1f 88");
        assert_eq!(octets(&[0xff]).display(), "ff");
        assert_eq!(Value::Null.display(), "null");
        assert_eq!(
            Value::Oid("1.3.6.1.4.1.8072.3.2.10".parse().unwrap()).display(),
            "1.3.6.1.4.1.8072.3.2.10"
        );
        assert_eq!(Value::IpAddress([192, 0, 2, 1]).display(), "192.0.2.1");
        assert_eq!(Value::Counter32(4_000_000_000).display(), "4000000000");
        assert_eq!(Value::Gauge32(1000).display(), "1000");
        assert_eq!(Value::Counter64(u64::MAX).display(), "18446744073709551615");
        assert_eq!(Value::NoSuchObject.display(), "noSuchObject");
        assert_eq!(Value::NoSuchInstance.display(), "noSuchInstance");
        assert_eq!(Value::EndOfMibView.display(), "endOfMibView");
        assert_eq!(Value::Opaque(vec![1, 2]).display(), "01 02");
        assert_eq!(Value::Integer(5).to_string(), "5");
    }

    #[test]
    fn ticks_render_as_durations() {
        assert_eq!(Value::TimeTicks(0).display(), "0.00s");
        assert_eq!(Value::TimeTicks(156).display(), "1.56s");
        assert_eq!(Value::TimeTicks(5_999).display(), "59.99s");
        assert_eq!(Value::TimeTicks(6_000).display(), "1m 0s");
        assert_eq!(Value::TimeTicks(12_345).display(), "2m 3s");
        assert_eq!(Value::TimeTicks(360_000).display(), "1h 0m 0s");
        assert_eq!(Value::TimeTicks(8_640_000).display(), "1d 0h 0m 0s");
        assert_eq!(Value::TimeTicks(27_547_612).display(), "3d 4h 31m 16s");
        assert_eq!(Value::TimeTicks(u32::MAX).display(), "497d 2h 27m 52s");
    }

    #[test]
    fn opaque_floats() {
        let float = Value::Opaque(vec![0x9f, 0x78, 0x04, 0x3f, 0x00, 0x00, 0x00]);
        assert_eq!(float.as_f64(), Some(0.5));
        assert_eq!(float.display(), "0.5");
        let double = Value::Opaque(vec![0x9f, 0x79, 0x08, 0x40, 0x09, 0x21, 0xfb, 0x54, 0x44, 0x2d, 0x18]);
        assert_eq!(double.as_f64(), Some(std::f64::consts::PI));
        assert_eq!(Value::Opaque(vec![0x9f, 0x78, 0x04, 0x3f]).as_f64(), None);
    }

    #[test]
    fn exceptions() {
        assert!(Value::NoSuchObject.is_exception());
        assert!(Value::NoSuchInstance.is_exception());
        assert!(Value::EndOfMibView.is_exception());
        assert!(!Value::Null.is_exception());
        assert!(!Value::Integer(0).is_exception());
    }

    #[test]
    fn conversions() {
        assert_eq!(Value::from("up"), octets(b"up"));
        assert_eq!(Value::from(String::from("up")), octets(b"up"));
        assert_eq!(Value::from(2), Value::Integer(2));
        assert_eq!(
            Value::from(Ipv4Addr::new(10, 0, 0, 1)),
            Value::IpAddress([10, 0, 0, 1])
        );
        assert_eq!(octets(b"ab").as_bytes(), Some(&b"ab"[..]));
        assert_eq!(Value::Integer(1).as_bytes(), None);
        let oid: Oid = "1.3.6".parse().unwrap();
        assert_eq!(Value::from(oid.clone()).as_oid(), Some(&oid));
    }

    #[test]
    fn serde_representation_is_tagged() {
        let cases = [
            (Value::Integer(-3), r#"{"type":"integer","value":-3}"#),
            (octets(b"hi"), r#"{"type":"octet_string","value":[104,105]}"#),
            (Value::Null, r#"{"type":"null"}"#),
            (
                Value::Oid("1.3.6.1".parse().unwrap()),
                r#"{"type":"oid","value":"1.3.6.1"}"#,
            ),
            (
                Value::IpAddress([10, 1, 2, 3]),
                r#"{"type":"ip_address","value":[10,1,2,3]}"#,
            ),
            (Value::Counter32(9), r#"{"type":"counter32","value":9}"#),
            (Value::Gauge32(9), r#"{"type":"gauge32","value":9}"#),
            (Value::TimeTicks(9), r#"{"type":"time_ticks","value":9}"#),
            (Value::Opaque(vec![1]), r#"{"type":"opaque","value":[1]}"#),
            (
                Value::Counter64(u64::MAX),
                r#"{"type":"counter64","value":18446744073709551615}"#,
            ),
            (Value::NoSuchObject, r#"{"type":"no_such_object"}"#),
            (Value::NoSuchInstance, r#"{"type":"no_such_instance"}"#),
            (Value::EndOfMibView, r#"{"type":"end_of_mib_view"}"#),
        ];
        for (value, json) in cases {
            assert_eq!(serde_json::to_string(&value).unwrap(), json);
            assert_eq!(serde_json::from_str::<Value>(json).unwrap(), value);
        }

        let bind = VarBind::new("1.3.6.1.2.1.1.3.0".parse().unwrap(), Value::TimeTicks(42));
        let json = serde_json::to_string(&bind).unwrap();
        assert_eq!(
            json,
            r#"{"oid":"1.3.6.1.2.1.1.3.0","value":{"type":"time_ticks","value":42}}"#
        );
        assert_eq!(serde_json::from_str::<VarBind>(&json).unwrap(), bind);
    }
}
