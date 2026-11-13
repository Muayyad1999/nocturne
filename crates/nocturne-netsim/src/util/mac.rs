//! Ethernet addresses.

use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub(crate) struct Mac(pub(crate) [u8; 6]);

impl Mac {
    pub(crate) const BROADCAST: Mac = Mac([0xff; 6]);
    pub(crate) const ZERO: Mac = Mac([0; 6]);

    pub(crate) fn is_broadcast(self) -> bool {
        self == Mac::BROADCAST
    }

    /// Group bit set: broadcast or multicast.
    pub(crate) fn is_multicast(self) -> bool {
        self.0[0] & 1 == 1
    }

    /// `0050.7966.6801`, the form used by IOS-style CLIs.
    pub(crate) fn dotted(self) -> String {
        let b = self.0;
        format!("{:02x}{:02x}.{:02x}{:02x}.{:02x}{:02x}", b[0], b[1], b[2], b[3], b[4], b[5])
    }

    /// `00-50-79-66-68-01`, the form used by the host shell.
    pub(crate) fn dashed(self) -> String {
        let b = self.0;
        format!("{:02X}-{:02X}-{:02X}-{:02X}-{:02X}-{:02X}", b[0], b[1], b[2], b[3], b[4], b[5])
    }

    /// Accepts `aabb.ccdd.eeff`, `aa:bb:cc:dd:ee:ff` and `aa-bb-cc-dd-ee-ff`.
    pub(crate) fn parse(s: &str) -> Option<Mac> {
        let hex: String = s.chars().filter(|c| !matches!(c, '.' | ':' | '-')).collect();
        if hex.len() != 12 || !hex.is_ascii() {
            return None;
        }
        let groups_ok = if s.contains('.') {
            s.split('.').count() == 3 && s.split('.').all(|g| g.len() == 4)
        } else {
            let sep = if s.contains(':') { ':' } else { '-' };
            s.split(sep).count() == 6 && s.split(sep).all(|g| g.len() == 2)
        };
        if !groups_ok {
            return None;
        }
        let mut out = [0u8; 6];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
        }
        Some(Mac(out))
    }

    /// The address `n` above this one (wrapping in the low three octets).
    pub(crate) fn offset(self, n: u32) -> Mac {
        let low = (u32::from(self.0[3]) << 16) | (u32::from(self.0[4]) << 8) | u32::from(self.0[5]);
        let low = low.wrapping_add(n) & 0x00ff_ffff;
        Mac([self.0[0], self.0[1], self.0[2], (low >> 16) as u8, (low >> 8) as u8, low as u8])
    }
}

impl fmt::Display for Mac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.dotted())
    }
}

impl fmt::Debug for Mac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.dotted())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format() {
        let m = Mac([0x00, 0x50, 0x79, 0x66, 0x68, 0x01]);
        assert_eq!(m.dotted(), "0050.7966.6801");
        assert_eq!(m.dashed(), "00-50-79-66-68-01");
        for s in ["0050.7966.6801", "00:50:79:66:68:01", "00-50-79-66-68-01"] {
            assert_eq!(Mac::parse(s), Some(m));
        }
        for s in ["", "0050.7966", "0050.7966.68011", "zz50.7966.6801", "00:50:79:66:68", "005.07966.6801"] {
            assert_eq!(Mac::parse(s), None, "{s}");
        }
        assert_eq!(m.offset(0xff).dotted(), "0050.7966.6900");
        assert!(Mac::BROADCAST.is_broadcast() && Mac::BROADCAST.is_multicast());
        assert!(!m.is_multicast());
    }
}
