//! Interface identifiers: parsing every usual spelling (`g0/1`, `gig 0/1`,
//! `GigabitEthernet0/1`, `po1`, `vl10`) and formatting the long and short names.

use std::fmt;

/// Declaration order is the order interfaces appear in a running configuration.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) enum IfKind {
    Loopback,
    PortChannel,
    Ethernet,
    FastEthernet,
    GigabitEthernet,
    TenGigabitEthernet,
    Dot11Radio,
    Vlan,
}

impl IfKind {
    const ALL: [IfKind; 8] = [
        IfKind::Loopback,
        IfKind::PortChannel,
        IfKind::Ethernet,
        IfKind::FastEthernet,
        IfKind::GigabitEthernet,
        IfKind::TenGigabitEthernet,
        IfKind::Dot11Radio,
        IfKind::Vlan,
    ];

    pub(crate) fn full(self) -> &'static str {
        match self {
            IfKind::Loopback => "Loopback",
            IfKind::PortChannel => "Port-channel",
            IfKind::Ethernet => "Ethernet",
            IfKind::FastEthernet => "FastEthernet",
            IfKind::GigabitEthernet => "GigabitEthernet",
            IfKind::TenGigabitEthernet => "TenGigabitEthernet",
            IfKind::Dot11Radio => "Dot11Radio",
            IfKind::Vlan => "Vlan",
        }
    }

    pub(crate) fn short(self) -> &'static str {
        match self {
            IfKind::Loopback => "Lo",
            IfKind::PortChannel => "Po",
            IfKind::Ethernet => "Et",
            IfKind::FastEthernet => "Fa",
            IfKind::GigabitEthernet => "Gi",
            IfKind::TenGigabitEthernet => "Te",
            IfKind::Dot11Radio => "Do",
            IfKind::Vlan => "Vl",
        }
    }

    /// Nominal speed of a physical port of this kind, in Mbit/s (0 = not physical).
    pub(crate) fn max_mbps(self) -> u32 {
        match self {
            IfKind::Ethernet | IfKind::GigabitEthernet => 1000,
            IfKind::FastEthernet => 100,
            IfKind::TenGigabitEthernet => 10_000,
            IfKind::Dot11Radio => 54,
            IfKind::Loopback | IfKind::PortChannel | IfKind::Vlan => 0,
        }
    }

    pub(crate) fn is_physical(self) -> bool {
        self.max_mbps() > 0
    }

    /// Every kind's first letter is distinct, so any prefix of the full name
    /// (or of the short name) identifies it.
    fn from_abbrev(word: &str) -> Option<IfKind> {
        if word.is_empty() {
            return None;
        }
        IfKind::ALL.into_iter().find(|kind| {
            crate::util::text::is_abbrev(word, kind.full())
                || word.eq_ignore_ascii_case(kind.short())
        })
    }
}

/// One interface of a device: physical port, sub-interface or logical interface.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct IfId {
    pub(crate) kind: IfKind,
    /// Slot path, e.g. `1/0/24` → `[1, 0, 24]`; unused positions are zero.
    unit: [u32; 3],
    depth: u8,
    /// Sub-interface number, 0 = none.
    pub(crate) sub: u32,
}

impl IfId {
    pub(crate) fn new(kind: IfKind, path: &[u32]) -> IfId {
        let mut unit = [0; 3];
        let depth = path.len().clamp(1, 3);
        for (slot, value) in unit.iter_mut().zip(path) {
            *slot = *value;
        }
        IfId {
            kind,
            unit,
            depth: depth as u8,
            sub: 0,
        }
    }

    pub(crate) fn vlan(id: u16) -> IfId {
        IfId::new(IfKind::Vlan, &[u32::from(id)])
    }

    pub(crate) fn port_channel(n: u32) -> IfId {
        IfId::new(IfKind::PortChannel, &[n])
    }

    /// The single number of a one-level interface (`Vlan10` → 10, `Loopback0` → 0).
    pub(crate) fn number(self) -> u32 {
        self.unit[0]
    }

    /// The last number of the slot path (`1/0/24` → 24).
    pub(crate) fn port_number(self) -> u32 {
        self.unit[usize::from(self.depth.clamp(1, 3)) - 1]
    }

    pub(crate) fn is_sub(self) -> bool {
        self.sub != 0
    }

    /// The main interface a sub-interface belongs to (identity otherwise).
    pub(crate) fn parent(self) -> IfId {
        IfId { sub: 0, ..self }
    }

    pub(crate) fn with_sub(self, sub: u32) -> IfId {
        IfId { sub, ..self }
    }

    pub(crate) fn is_physical(self) -> bool {
        self.kind.is_physical() && self.sub == 0
    }

    fn numbers(self) -> String {
        let mut out = String::new();
        for (i, value) in self.unit.iter().take(usize::from(self.depth)).enumerate() {
            if i > 0 {
                out.push('/');
            }
            out.push_str(&value.to_string());
        }
        if self.sub != 0 {
            out.push('.');
            out.push_str(&self.sub.to_string());
        }
        out
    }

    /// `GigabitEthernet0/1`.
    pub(crate) fn name(self) -> String {
        format!("{}{}", self.kind.full(), self.numbers())
    }

    /// `Gi0/1`.
    pub(crate) fn short(self) -> String {
        format!("{}{}", self.kind.short(), self.numbers())
    }

    /// Replace the last number of the slot path (used by `interface range`).
    pub(crate) fn with_port_number(self, n: u32) -> IfId {
        let mut out = self;
        out.unit[usize::from(self.depth.clamp(1, 3)) - 1] = n;
        out
    }

    /// Parse any accepted spelling. The type word and the numbers may have been
    /// typed with a space between them; callers join the two tokens first.
    pub(crate) fn parse(text: &str) -> Option<IfId> {
        let text = text.trim();
        let split = text.find(|c: char| c.is_ascii_digit())?;
        let (word, numbers) = text.split_at(split);
        let kind = IfKind::from_abbrev(word.trim())?;
        let (path, sub) = match numbers.split_once('.') {
            Some((path, sub)) => (path, Some(sub)),
            None => (numbers, None),
        };
        let mut unit = Vec::new();
        for part in path.split('/') {
            if part.is_empty() || part.len() > 10 || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            unit.push(part.parse::<u32>().ok()?);
        }
        if unit.len() > 3 {
            return None;
        }
        let mut id = IfId::new(kind, &unit);
        if let Some(sub) = sub {
            let sub: u32 = sub.parse().ok()?;
            if sub == 0 || !kind.is_physical() && kind != IfKind::PortChannel {
                return None;
            }
            id.sub = sub;
        }
        Some(id)
    }

    /// Is `word` (no digits) a valid, possibly abbreviated, interface type name?
    pub(crate) fn is_type_word(word: &str) -> bool {
        word.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-')
            && IfKind::from_abbrev(word).is_some()
    }
}

impl fmt::Display for IfId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

impl fmt::Debug for IfId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_usual_spelling() {
        let gi = IfId::new(IfKind::GigabitEthernet, &[0, 1]);
        for s in [
            "g0/1",
            "gi0/1",
            "gig0/1",
            "Gi0/1",
            "GigabitEthernet0/1",
            "gigabitethernet 0/1",
            "GIG0/1",
        ] {
            assert_eq!(IfId::parse(s), Some(gi), "{s}");
        }
        assert_eq!(IfId::parse("fa0/24").unwrap().name(), "FastEthernet0/24");
        assert_eq!(
            IfId::parse("te1/1/1").unwrap().name(),
            "TenGigabitEthernet1/1/1"
        );
        assert_eq!(IfId::parse("po1").unwrap().name(), "Port-channel1");
        assert_eq!(IfId::parse("port-channel 12").unwrap().short(), "Po12");
        assert_eq!(IfId::parse("vl10").unwrap().name(), "Vlan10");
        assert_eq!(IfId::parse("lo0").unwrap().name(), "Loopback0");
        assert_eq!(IfId::parse("e0").unwrap().name(), "Ethernet0");
        assert_eq!(
            IfId::parse("g0/0.10").unwrap().name(),
            "GigabitEthernet0/0.10"
        );
        assert_eq!(
            IfId::parse("g0/0.10").unwrap().parent(),
            IfId::parse("g0/0").unwrap()
        );
        for bad in [
            "",
            "g",
            "0/1",
            "x0/1",
            "g0/",
            "g0//1",
            "g0/1/2/3",
            "g0/1.0",
            "vl10.5",
            "g0/1.x",
            "g99999999999",
        ] {
            assert_eq!(IfId::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn running_config_order() {
        let mut ids: Vec<IfId> = [
            "vl1", "g0/1", "fa0/2", "lo0", "g0/0.20", "g0/0", "po1", "fa0/10", "g0/0.3", "te1/1/1",
        ]
        .iter()
        .map(|s| IfId::parse(s).unwrap())
        .collect();
        ids.sort();
        let names: Vec<String> = ids.iter().map(|i| i.short()).collect();
        assert_eq!(
            names,
            [
                "Lo0", "Po1", "Fa0/2", "Fa0/10", "Gi0/0", "Gi0/0.3", "Gi0/0.20", "Gi0/1",
                "Te1/1/1", "Vl1"
            ]
        );
    }
}
