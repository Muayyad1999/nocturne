//! Object identifiers.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::SnmpError;

/// Object identifier. Displays and parses dotted form (`"1.3.6.1.2.1.1.1.0"`, leading dot accepted).
///
/// Ordering is the lexicographic ordering SNMP uses for GETNEXT, so an `Oid` can be used
/// directly as a `BTreeMap` key to model a MIB view.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Oid(Vec<u32>);

impl Oid {
    /// Builds an OID from its sub-identifiers.
    pub fn new(parts: impl Into<Vec<u32>>) -> Oid {
        Oid(parts.into())
    }

    /// The sub-identifiers.
    pub fn parts(&self) -> &[u32] {
        &self.0
    }

    /// Number of sub-identifiers.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` for the empty OID.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// `true` if `self` equals `prefix` or lies underneath it.
    pub fn starts_with(&self, prefix: &Oid) -> bool {
        self.0.starts_with(&prefix.0)
    }

    /// `self` with one more sub-identifier appended.
    pub fn child(&self, sub: u32) -> Oid {
        let mut parts = Vec::with_capacity(self.0.len() + 1);
        parts.extend_from_slice(&self.0);
        parts.push(sub);
        Oid(parts)
    }

    /// `self` with several sub-identifiers appended (a column OID plus a table index, say).
    pub fn join(&self, suffix: &[u32]) -> Oid {
        let mut parts = Vec::with_capacity(self.0.len() + suffix.len());
        parts.extend_from_slice(&self.0);
        parts.extend_from_slice(suffix);
        Oid(parts)
    }

    /// `self` without its last sub-identifier, or `None` for the empty OID.
    pub fn parent(&self) -> Option<Oid> {
        self.0.split_last().map(|(_, rest)| Oid(rest.to_vec()))
    }

    /// The part of `self` after `prefix` (table index), if `self` is under it.
    pub fn suffix(&self, prefix: &Oid) -> Option<&[u32]> {
        self.0.strip_prefix(prefix.0.as_slice())
    }
}

impl From<Vec<u32>> for Oid {
    fn from(parts: Vec<u32>) -> Self {
        Oid(parts)
    }
}

impl From<&[u32]> for Oid {
    fn from(parts: &[u32]) -> Self {
        Oid(parts.to_vec())
    }
}

impl<const N: usize> From<[u32; N]> for Oid {
    fn from(parts: [u32; N]) -> Self {
        Oid(parts.to_vec())
    }
}

impl AsRef<[u32]> for Oid {
    fn as_ref(&self) -> &[u32] {
        &self.0
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, part) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            write!(f, "{part}")?;
        }
        Ok(())
    }
}

/// `Oid(1.3.6.1.2.1.1.1.0)` — far easier to read in logs and assertion failures than a vector.
impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Oid({self})")
    }
}

impl FromStr for Oid {
    type Err = SnmpError;

    /// Parses `"1.3.6.1"` or `".1.3.6.1"`. Every component must be a decimal `u32`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = |why: &str| SnmpError::Decode(format!("invalid OID {s:?}: {why}"));
        let dotted = s.strip_prefix('.').unwrap_or(s);
        if dotted.is_empty() {
            return Err(invalid("no sub-identifiers"));
        }
        dotted
            .split('.')
            .map(|part| {
                if part.is_empty() {
                    return Err(invalid("empty sub-identifier"));
                }
                if !part.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(invalid("sub-identifiers must be decimal numbers"));
                }
                part.parse::<u32>()
                    .map_err(|_| invalid("sub-identifier does not fit in 32 bits"))
            })
            .collect::<Result<Vec<u32>, _>>()
            .map(Oid)
    }
}

impl Serialize for Oid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Oid {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OidVisitor;

        impl serde::de::Visitor<'_> for OidVisitor {
            type Value = Oid;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a dotted object identifier such as \"1.3.6.1.2.1.1.1.0\"")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Oid, E> {
                v.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_str(OidVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn oid(s: &str) -> Oid {
        s.parse().unwrap()
    }

    #[test]
    fn parses_and_displays_dotted_form() {
        let sys_descr = oid("1.3.6.1.2.1.1.1.0");
        assert_eq!(sys_descr.parts(), &[1, 3, 6, 1, 2, 1, 1, 1, 0]);
        assert_eq!(sys_descr.to_string(), "1.3.6.1.2.1.1.1.0");
        assert_eq!(oid(".1.3.6.1.2.1.1.1.0"), sys_descr);
        assert_eq!(oid("0"), Oid::new([0]));
        assert_eq!(oid("1.4294967295").parts(), &[1, u32::MAX]);
        assert_eq!(format!("{sys_descr:?}"), "Oid(1.3.6.1.2.1.1.1.0)");
    }

    #[test]
    fn rejects_malformed_text() {
        for bad in [
            "",
            ".",
            "1..3",
            "1.3.",
            "..1",
            "1.3.x",
            "1.-3",
            "1.+3",
            "1. 3",
            "1.4294967296",
            "sysDescr.0",
            "1,3",
        ] {
            assert!(
                matches!(bad.parse::<Oid>(), Err(SnmpError::Decode(_))),
                "{bad:?} should not parse"
            );
        }
    }

    #[test]
    fn prefix_helpers() {
        let column = oid("1.3.6.1.2.1.2.2.1.10");
        let instance = column.child(3);
        assert_eq!(instance, oid("1.3.6.1.2.1.2.2.1.10.3"));
        assert!(instance.starts_with(&column));
        assert!(column.starts_with(&column));
        assert!(!column.starts_with(&instance));
        assert!(!oid("1.3.6.1.2.1.2.2.1.11.3").starts_with(&column));
        assert_eq!(instance.suffix(&column), Some(&[3u32][..]));
        assert_eq!(column.suffix(&column), Some(&[][..]));
        assert_eq!(column.suffix(&instance), None);
        assert_eq!(
            column.join(&[10, 0, 0, 1]),
            oid("1.3.6.1.2.1.2.2.1.10.10.0.0.1")
        );
        assert_eq!(instance.parent(), Some(column));
        assert_eq!(Oid::default().parent(), None);
        assert!(Oid::default().is_empty());
        assert_eq!(instance.len(), 11);
    }

    #[test]
    fn orders_like_getnext() {
        let mut oids = vec![
            oid("1.3.6.1.2.1.2.2.1.10.10"),
            oid("1.3.6.1.2.1.2.2.1.10.2"),
            oid("1.3.6.1.2.1.2.2.1.10"),
            oid("1.3.6.1.2.1.2.2.1.9.99"),
            oid("1.3.6.1.2.1.2.2.1.10.2.1"),
        ];
        oids.sort();
        let sorted: Vec<String> = oids.iter().map(Oid::to_string).collect();
        assert_eq!(
            sorted,
            [
                "1.3.6.1.2.1.2.2.1.9.99",
                "1.3.6.1.2.1.2.2.1.10",
                "1.3.6.1.2.1.2.2.1.10.2",
                "1.3.6.1.2.1.2.2.1.10.2.1",
                "1.3.6.1.2.1.2.2.1.10.10",
            ]
        );
    }

    #[test]
    fn serde_uses_the_dotted_string() {
        let sys_name = oid("1.3.6.1.2.1.1.5.0");
        let json = serde_json::to_string(&sys_name).unwrap();
        assert_eq!(json, "\"1.3.6.1.2.1.1.5.0\"");
        assert_eq!(serde_json::from_str::<Oid>(&json).unwrap(), sys_name);
        assert_eq!(
            serde_json::from_str::<Oid>("\".1.3.6\"").unwrap(),
            oid("1.3.6")
        );
        assert!(serde_json::from_str::<Oid>("\"1.3.banana\"").is_err());
        assert!(serde_json::from_str::<Oid>("[1,3,6]").is_err());
    }
}
