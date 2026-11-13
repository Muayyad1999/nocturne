//! The subset of ASN.1 BER that SNMP uses: definite-length TLVs, INTEGER, OCTET STRING, NULL,
//! OBJECT IDENTIFIER and the SMI application types.
//!
//! Decoding is strict about structure (a length may never run past its container, indefinite
//! lengths and multi-byte tags are refused) and lenient about the harmless quirks real agents
//! produce: non-minimal lengths and integers, zero-length integers, unsigned values encoded as
//! negative integers. Nothing in here panics, whatever the input.

use crate::{Oid, SnmpError};

pub(crate) type Result<T> = std::result::Result<T, SnmpError>;

/// Universal, application and context tags that appear in SNMP messages.
pub(crate) mod tag {
    pub const INTEGER: u8 = 0x02;
    pub const OCTET_STRING: u8 = 0x04;
    pub const NULL: u8 = 0x05;
    pub const OBJECT_ID: u8 = 0x06;
    pub const SEQUENCE: u8 = 0x30;

    pub const IP_ADDRESS: u8 = 0x40;
    pub const COUNTER32: u8 = 0x41;
    pub const GAUGE32: u8 = 0x42;
    pub const TIME_TICKS: u8 = 0x43;
    pub const OPAQUE: u8 = 0x44;
    /// NsapAddress — obsolete (RFC 1442), still emitted by a few museum pieces.
    pub const NSAP_ADDRESS: u8 = 0x45;
    pub const COUNTER64: u8 = 0x46;
    /// UInteger32 — obsolete (RFC 1442) twin of Gauge32/Unsigned32.
    pub const UINTEGER32: u8 = 0x47;

    pub const NO_SUCH_OBJECT: u8 = 0x80;
    pub const NO_SUCH_INSTANCE: u8 = 0x81;
    pub const END_OF_MIB_VIEW: u8 = 0x82;
}

fn malformed(message: impl Into<String>) -> SnmpError {
    SnmpError::Decode(message.into())
}

// ---------------------------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------------------------

/// A cursor over BER data that knows where it is inside the datagram being decoded.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    /// Offset of `buf[0]` from the start of the datagram.
    offset: usize,
}

impl<'a> Reader<'a> {
    pub fn new(datagram: &'a [u8]) -> Self {
        Reader {
            buf: datagram,
            offset: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// The bytes not consumed yet.
    pub fn remaining(&self) -> &'a [u8] {
        self.buf
    }

    /// Offset of the next unread byte from the start of the datagram.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// The tag of the next element, if there is one.
    pub fn peek_tag(&self) -> Option<u8> {
        self.buf.first().copied()
    }

    /// Reads one element and returns its tag and a reader over its contents.
    pub fn read_tlv(&mut self) -> Result<(u8, Reader<'a>)> {
        let (&tag, rest) = self
            .buf
            .split_first()
            .ok_or_else(|| malformed("unexpected end of data"))?;
        if tag & 0x1f == 0x1f {
            return Err(malformed("multi-byte tags are not used by SNMP"));
        }
        let (&first, rest) = rest
            .split_first()
            .ok_or_else(|| malformed("truncated length"))?;

        let (length, header_len, rest) = if first < 0x80 {
            (usize::from(first), 2, rest)
        } else {
            let count = usize::from(first & 0x7f);
            if count == 0 {
                return Err(malformed("indefinite lengths are not allowed"));
            }
            if count > rest.len() {
                return Err(malformed("truncated length"));
            }
            let (length_octets, rest) = rest.split_at(count);
            // Leading zero octets (a non-minimal length) are harmless; overflow is not.
            let mut length = 0usize;
            for &octet in length_octets {
                if length > usize::MAX >> 8 {
                    return Err(malformed("length does not fit in memory"));
                }
                length = (length << 8) | usize::from(octet);
            }
            (length, 2 + count, rest)
        };

        if length > rest.len() {
            return Err(malformed(format!(
                "element of {length} bytes overruns the {} bytes available",
                rest.len()
            )));
        }
        let (content, rest) = rest.split_at(length);
        let content = Reader {
            buf: content,
            offset: self.offset + header_len,
        };
        self.buf = rest;
        self.offset += header_len + length;
        Ok((tag, content))
    }

    /// Reads one element, insisting on its tag.
    pub fn expect(&mut self, expected: u8, what: &str) -> Result<Reader<'a>> {
        let (tag, content) = self.read_tlv()?;
        if tag != expected {
            return Err(malformed(format!(
                "expected {what} (tag 0x{expected:02x}), found tag 0x{tag:02x}"
            )));
        }
        Ok(content)
    }

    pub fn sequence(&mut self) -> Result<Reader<'a>> {
        self.expect(tag::SEQUENCE, "SEQUENCE")
    }

    /// An INTEGER of any size up to 64 bits.
    pub fn integer(&mut self) -> Result<i64> {
        let content = self.expect(tag::INTEGER, "INTEGER")?;
        decode_integer(content.remaining())
    }

    /// An Integer32. Values up to 2^32-1 are accepted and wrap, because some agents treat
    /// request ids as unsigned.
    pub fn integer32(&mut self) -> Result<i32> {
        let content = self.expect(tag::INTEGER, "INTEGER")?;
        decode_integer32(content.remaining())
    }

    /// A non-negative INTEGER that fits in 31 bits (error-status, engine boots, …).
    pub fn non_negative(&mut self, what: &str) -> Result<u32> {
        let value = self.integer()?;
        u32::try_from(value)
            .ok()
            .filter(|v| *v <= i32::MAX as u32)
            .ok_or_else(|| malformed(format!("{what} out of range: {value}")))
    }

    pub fn octet_string(&mut self) -> Result<&'a [u8]> {
        Ok(self
            .expect(tag::OCTET_STRING, "OCTET STRING")?
            .remaining())
    }

    pub fn oid(&mut self) -> Result<Oid> {
        let content = self.expect(tag::OBJECT_ID, "OBJECT IDENTIFIER")?;
        decode_oid(content.remaining())
    }

    /// Fails if anything is left; used where trailing data would mean a misparse.
    pub fn finish(&self, what: &str) -> Result<()> {
        if self.buf.is_empty() {
            Ok(())
        } else {
            Err(malformed(format!(
                "{} unexpected trailing bytes after {what}",
                self.buf.len()
            )))
        }
    }
}

/// Decodes two's-complement content of any length into a wide integer.
///
/// Redundant sign octets are dropped first, so over-long encodings of small numbers are fine,
/// and a zero-length INTEGER (seen in the wild) reads as 0.
fn decode_wide(content: &[u8]) -> Result<i128> {
    let mut bytes = content;
    while let [first, second, ..] = bytes {
        let redundant = (*first == 0x00 && second & 0x80 == 0) || (*first == 0xff && second & 0x80 != 0);
        if !redundant {
            break;
        }
        bytes = &bytes[1..];
    }
    // Nine octets hold every value SNMP knows (a Counter64 with its sign octet).
    if bytes.len() > 9 {
        return Err(malformed(format!(
            "integer of {} bytes is too large",
            bytes.len()
        )));
    }
    let negative = bytes.first().is_some_and(|b| b & 0x80 != 0);
    let mut value: i128 = if negative { -1 } else { 0 };
    for &byte in bytes {
        value = (value << 8) | i128::from(byte);
    }
    Ok(value)
}

pub(crate) fn decode_integer(content: &[u8]) -> Result<i64> {
    let value = decode_wide(content)?;
    i64::try_from(value).map_err(|_| malformed("INTEGER does not fit in 64 bits"))
}

pub(crate) fn decode_integer32(content: &[u8]) -> Result<i32> {
    let value = decode_wide(content)?;
    if let Ok(v) = i32::try_from(value) {
        Ok(v)
    } else if let Ok(v) = u32::try_from(value) {
        Ok(v as i32)
    } else {
        Err(malformed("Integer32 out of range"))
    }
}

/// Decodes an unsigned 32-bit application type (Counter32, Gauge32, TimeTicks).
///
/// Agents that build these with a signed-integer encoder send large values as negative
/// numbers; like net-snmp we take those modulo 2^32.
pub(crate) fn decode_unsigned32(content: &[u8]) -> Result<u32> {
    let value = decode_wide(content)?;
    if let Ok(v) = u32::try_from(value) {
        Ok(v)
    } else if let Ok(v) = i32::try_from(value) {
        Ok(v as u32)
    } else {
        Err(malformed("unsigned 32-bit value out of range"))
    }
}

/// Decodes a Counter64, with the same leniency as [`decode_unsigned32`].
pub(crate) fn decode_unsigned64(content: &[u8]) -> Result<u64> {
    let value = decode_wide(content)?;
    if let Ok(v) = u64::try_from(value) {
        Ok(v)
    } else if let Ok(v) = i64::try_from(value) {
        Ok(v as u64)
    } else {
        Err(malformed("Counter64 out of range"))
    }
}

/// Largest value of the first sub-identifier octet group: arcs `2.(2^32-1)`.
const MAX_FIRST_SUBID: u64 = 80 + u32::MAX as u64;

fn read_subid(bytes: &mut &[u8]) -> Result<u64> {
    let mut value = 0u64;
    loop {
        let (&octet, rest) = bytes
            .split_first()
            .ok_or_else(|| malformed("truncated OID sub-identifier"))?;
        *bytes = rest;
        if value > MAX_FIRST_SUBID >> 7 {
            return Err(malformed("OID sub-identifier does not fit in 32 bits"));
        }
        value = (value << 7) | u64::from(octet & 0x7f);
        if octet & 0x80 == 0 {
            return Ok(value);
        }
    }
}

pub(crate) fn decode_oid(content: &[u8]) -> Result<Oid> {
    let too_large = || malformed("OID sub-identifier does not fit in 32 bits");
    let mut bytes = content;
    if bytes.is_empty() {
        // Some agents send a zero-length OID where they mean zeroDotZero.
        return Ok(Oid::new([0, 0]));
    }
    let mut parts = Vec::with_capacity(content.len() + 1);

    // The first group packs the first two arcs as 40 * X + Y, with X limited to 0..=2.
    let first = read_subid(&mut bytes)?;
    let (x, y) = match first {
        0..=39 => (0, first),
        40..=79 => (1, first - 40),
        _ => (2, first - 80),
    };
    parts.push(x);
    parts.push(u32::try_from(y).map_err(|_| too_large())?);

    while !bytes.is_empty() {
        let subid = read_subid(&mut bytes)?;
        parts.push(u32::try_from(subid).map_err(|_| too_large())?);
    }
    Ok(Oid::new(parts))
}

// ---------------------------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------------------------

/// Checks that an OID can be represented in BER: the first arc is 0, 1 or 2 and, below arc 2,
/// the second arc is at most 39.
pub(crate) fn check_oid(oid: &Oid) -> Result<()> {
    let unencodable = |why: &str| SnmpError::Unsupported(format!("OID {oid} cannot be encoded: {why}"));
    match *oid.parts() {
        [first, ..] if first > 2 => Err(unencodable("the first arc must be 0, 1 or 2")),
        [first, second, ..] if first < 2 && second > 39 => {
            Err(unencodable("the second arc must be below 40"))
        }
        [2, second, ..] if u64::from(second) + 80 > MAX_FIRST_SUBID => {
            Err(unencodable("the second arc is too large"))
        }
        _ => Ok(()),
    }
}

/// An append-only BER encoder.
#[derive(Debug, Default)]
pub(crate) struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Writer::default()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    fn header(&mut self, tag: u8, length: usize) {
        self.buf.push(tag);
        let (octets, count) = length_octets(length);
        self.buf.extend_from_slice(&octets[..count]);
    }

    /// INTEGER in the shortest two's-complement form.
    pub fn integer(&mut self, value: i64) {
        let bytes = value.to_be_bytes();
        let mut start = 0;
        while start < bytes.len() - 1 {
            let redundant = (bytes[start] == 0x00 && bytes[start + 1] & 0x80 == 0)
                || (bytes[start] == 0xff && bytes[start + 1] & 0x80 != 0);
            if !redundant {
                break;
            }
            start += 1;
        }
        self.header(tag::INTEGER, bytes.len() - start);
        self.buf.extend_from_slice(&bytes[start..]);
    }

    /// An unsigned application type: shortest form, with a leading zero octet whenever the top
    /// bit of the first content octet would otherwise make the value read as negative.
    pub fn unsigned(&mut self, tag: u8, value: u64) {
        let bytes = value.to_be_bytes();
        let start = bytes
            .iter()
            .position(|&b| b != 0)
            .unwrap_or(bytes.len() - 1);
        let pad = bytes[start] & 0x80 != 0;
        self.header(tag, bytes.len() - start + usize::from(pad));
        if pad {
            self.buf.push(0x00);
        }
        self.buf.extend_from_slice(&bytes[start..]);
    }

    /// OCTET STRING, or any other primitive type whose content is raw bytes.
    pub fn octets(&mut self, tag: u8, content: &[u8]) {
        self.header(tag, content.len());
        self.buf.extend_from_slice(content);
    }

    pub fn octet_string(&mut self, content: &[u8]) {
        self.octets(tag::OCTET_STRING, content);
    }

    /// NULL, or one of the zero-length exception markers.
    pub fn null(&mut self, tag: u8) {
        self.header(tag, 0);
    }

    /// OBJECT IDENTIFIER. An OID with fewer than two arcs is completed with zeros, as net-snmp
    /// does, so that `walk("1")` works; see [`check_oid`] for what cannot be encoded.
    pub fn oid(&mut self, oid: &Oid) {
        let parts = oid.parts();
        let first = u64::from(parts.first().copied().unwrap_or(0).min(2)) * 40
            + u64::from(parts.get(1).copied().unwrap_or(0));
        let mut content = Vec::with_capacity(parts.len() + 4);
        push_subid(&mut content, first);
        for &part in parts.iter().skip(2) {
            push_subid(&mut content, u64::from(part));
        }
        self.octets(tag::OBJECT_ID, &content);
    }

    /// A constructed element (SEQUENCE, PDU, …) whose contents are written by `fill`.
    pub fn constructed(&mut self, tag: u8, fill: impl FnOnce(&mut Writer)) {
        self.buf.push(tag);
        let start = self.buf.len();
        fill(self);
        let (octets, count) = length_octets(self.buf.len() - start);
        self.buf.splice(start..start, octets[..count].iter().copied());
    }

    pub fn sequence(&mut self, fill: impl FnOnce(&mut Writer)) {
        self.constructed(tag::SEQUENCE, fill);
    }

    /// Appends bytes that are already BER.
    pub fn raw(&mut self, encoded: &[u8]) {
        self.buf.extend_from_slice(encoded);
    }
}

/// The length octets for `length`: short form below 128, otherwise the minimal long form.
fn length_octets(length: usize) -> ([u8; 9], usize) {
    let mut out = [0u8; 9];
    if length < 0x80 {
        out[0] = length as u8;
        return (out, 1);
    }
    let bytes = (length as u64).to_be_bytes();
    let skip = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len() - 1);
    let count = bytes.len() - skip;
    out[0] = 0x80 | count as u8;
    out[1..=count].copy_from_slice(&bytes[skip..]);
    (out, count + 1)
}

/// Base-128, most significant group first, continuation bit on all but the last octet.
fn push_subid(out: &mut Vec<u8>, value: u64) {
    let groups = (64 - value.leading_zeros()).div_ceil(7).max(1);
    for group in (0..groups).rev() {
        let septet = ((value >> (7 * group)) & 0x7f) as u8;
        out.push(if group == 0 { septet } else { septet | 0x80 });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn oid(s: &str) -> Oid {
        s.parse().unwrap()
    }

    fn encode(fill: impl FnOnce(&mut Writer)) -> Vec<u8> {
        let mut w = Writer::new();
        fill(&mut w);
        w.into_bytes()
    }

    fn integer_bytes(v: i64) -> Vec<u8> {
        encode(|w| w.integer(v))
    }

    fn unsigned_bytes(tag: u8, v: u64) -> Vec<u8> {
        encode(|w| w.unsigned(tag, v))
    }

    fn oid_bytes(s: &str) -> Vec<u8> {
        encode(|w| w.oid(&oid(s)))
    }

    fn is_decode_error<T: std::fmt::Debug>(result: Result<T>) -> bool {
        matches!(result, Err(SnmpError::Decode(_)))
    }

    // ---- INTEGER --------------------------------------------------------------------------

    #[test]
    fn integer_vectors() {
        // (value, hand-checked encoding)
        let vectors: &[(i64, &[u8])] = &[
            (0, &[0x02, 0x01, 0x00]),
            (1, &[0x02, 0x01, 0x01]),
            (127, &[0x02, 0x01, 0x7f]),
            (128, &[0x02, 0x02, 0x00, 0x80]),
            (255, &[0x02, 0x02, 0x00, 0xff]),
            (256, &[0x02, 0x02, 0x01, 0x00]),
            (32_767, &[0x02, 0x02, 0x7f, 0xff]),
            (32_768, &[0x02, 0x03, 0x00, 0x80, 0x00]),
            (-1, &[0x02, 0x01, 0xff]),
            (-128, &[0x02, 0x01, 0x80]),
            (-129, &[0x02, 0x02, 0xff, 0x7f]),
            (-32_768, &[0x02, 0x02, 0x80, 0x00]),
            (-32_769, &[0x02, 0x03, 0xff, 0x7f, 0xff]),
            (2_147_483_647, &[0x02, 0x04, 0x7f, 0xff, 0xff, 0xff]),
            (-2_147_483_648, &[0x02, 0x04, 0x80, 0x00, 0x00, 0x00]),
            (1_042_844_701, &[0x02, 0x04, 0x3e, 0x28, 0x8c, 0x1d]),
            (
                i64::MAX,
                &[0x02, 0x08, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            ),
            (
                i64::MIN,
                &[0x02, 0x08, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ),
        ];
        for &(value, bytes) in vectors {
            assert_eq!(integer_bytes(value), bytes, "encoding {value}");
            assert_eq!(Reader::new(bytes).integer().unwrap(), value, "decoding {value}");
        }
    }

    #[test]
    fn integer_decoding_tolerates_field_quirks() {
        // Zero-length INTEGER.
        assert_eq!(decode_integer(&[]).unwrap(), 0);
        // Non-minimal encodings, positive and negative.
        assert_eq!(decode_integer(&[0x00, 0x00, 0x00, 0x05]).unwrap(), 5);
        assert_eq!(decode_integer(&[0xff, 0xff, 0xff, 0xfe]).unwrap(), -2);
        assert_eq!(decode_integer(&[0x00; 12]).unwrap(), 0);
        assert_eq!(decode_integer(&[0xff; 12]).unwrap(), -1);
        assert_eq!(
            decode_integer(&[0x00, 0x00, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).unwrap(),
            i64::MAX
        );
        // Too large for 64 bits.
        assert!(is_decode_error(decode_integer(&[0x00, 0x80, 0, 0, 0, 0, 0, 0, 0])));
        assert!(is_decode_error(decode_integer(&[0x01, 0, 0, 0, 0, 0, 0, 0, 0])));
        assert!(is_decode_error(decode_integer(&[0x7f; 17])));
    }

    #[test]
    fn integer32_accepts_unsigned_request_ids() {
        assert_eq!(decode_integer32(&[0x7f, 0xff, 0xff, 0xff]).unwrap(), i32::MAX);
        assert_eq!(decode_integer32(&[0x80, 0x00, 0x00, 0x00]).unwrap(), i32::MIN);
        // 0xFFFFFFFE sent as an unsigned value wraps to -2.
        assert_eq!(decode_integer32(&[0x00, 0xff, 0xff, 0xff, 0xfe]).unwrap(), -2);
        assert!(is_decode_error(decode_integer32(&[0x01, 0x00, 0x00, 0x00, 0x00])));
        assert!(is_decode_error(decode_integer32(&[0xff, 0x7f, 0xff, 0xff, 0xff])));
    }

    #[test]
    fn non_negative_rejects_out_of_range() {
        assert_eq!(Reader::new(&[0x02, 0x01, 0x05]).non_negative("x").unwrap(), 5);
        assert_eq!(
            Reader::new(&[0x02, 0x04, 0x7f, 0xff, 0xff, 0xff]).non_negative("x").unwrap(),
            i32::MAX as u32
        );
        assert!(is_decode_error(Reader::new(&[0x02, 0x01, 0xff]).non_negative("x")));
        assert!(is_decode_error(
            Reader::new(&[0x02, 0x05, 0x00, 0x80, 0x00, 0x00, 0x00]).non_negative("x")
        ));
    }

    // ---- unsigned application types -------------------------------------------------------

    #[test]
    fn unsigned_vectors() {
        let vectors: &[(u8, u64, &[u8])] = &[
            (tag::COUNTER32, 0, &[0x41, 0x01, 0x00]),
            (tag::COUNTER32, 127, &[0x41, 0x01, 0x7f]),
            // The top bit of the first content octet forces a leading zero.
            (tag::COUNTER32, 128, &[0x41, 0x02, 0x00, 0x80]),
            (tag::GAUGE32, 255, &[0x42, 0x02, 0x00, 0xff]),
            (tag::GAUGE32, 65_535, &[0x42, 0x03, 0x00, 0xff, 0xff]),
            (tag::TIME_TICKS, 12_345, &[0x43, 0x02, 0x30, 0x39]),
            (tag::TIME_TICKS, 87_109, &[0x43, 0x03, 0x01, 0x54, 0x45]),
            (tag::COUNTER32, 2_147_483_647, &[0x41, 0x04, 0x7f, 0xff, 0xff, 0xff]),
            (
                tag::COUNTER32,
                2_147_483_648,
                &[0x41, 0x05, 0x00, 0x80, 0x00, 0x00, 0x00],
            ),
            (
                tag::COUNTER32,
                4_294_967_295,
                &[0x41, 0x05, 0x00, 0xff, 0xff, 0xff, 0xff],
            ),
            (tag::COUNTER64, 0, &[0x46, 0x01, 0x00]),
            (tag::COUNTER64, 8_519_442, &[0x46, 0x04, 0x00, 0x81, 0xff, 0x12]),
            (
                tag::COUNTER64,
                4_294_967_296,
                &[0x46, 0x05, 0x01, 0x00, 0x00, 0x00, 0x00],
            ),
            (
                tag::COUNTER64,
                u64::MAX >> 1,
                &[0x46, 0x08, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            ),
            (
                tag::COUNTER64,
                u64::MAX,
                &[0x46, 0x09, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            ),
        ];
        for &(tag, value, bytes) in vectors {
            assert_eq!(unsigned_bytes(tag, value), bytes, "encoding {value}");
            let content = &bytes[2..];
            assert_eq!(decode_unsigned64(content).unwrap(), value, "decoding {value}");
            if tag != tag::COUNTER64 {
                assert_eq!(u64::from(decode_unsigned32(content).unwrap()), value);
            }
        }
    }

    #[test]
    fn unsigned_decoding_tolerates_field_quirks() {
        // Missing leading zero: an agent that used a signed encoder for 4294967295 / 3000000000.
        assert_eq!(decode_unsigned32(&[0xff, 0xff, 0xff, 0xff]).unwrap(), u32::MAX);
        assert_eq!(decode_unsigned32(&[0xb2, 0xd0, 0x5e, 0x00]).unwrap(), 3_000_000_000);
        // ... which, shortened by that encoder, is sign-extended like net-snmp does.
        assert_eq!(decode_unsigned32(&[0xff]).unwrap(), u32::MAX);
        assert_eq!(decode_unsigned32(&[0x80]).unwrap(), 0xffff_ff80);
        // Zero length and redundant padding.
        assert_eq!(decode_unsigned32(&[]).unwrap(), 0);
        assert_eq!(decode_unsigned32(&[0, 0, 0, 0, 0, 0, 0x2a]).unwrap(), 42);
        // Out of range for 32 bits.
        assert!(is_decode_error(decode_unsigned32(&[0x01, 0x00, 0x00, 0x00, 0x00])));
        assert!(is_decode_error(decode_unsigned32(&[0xfe, 0xff, 0xff, 0xff, 0xff])));

        assert_eq!(decode_unsigned64(&[0xff; 8]).unwrap(), u64::MAX);
        assert_eq!(decode_unsigned64(&[0xff]).unwrap(), u64::MAX);
        assert!(is_decode_error(decode_unsigned64(&[0x01, 0, 0, 0, 0, 0, 0, 0, 0])));
        assert!(is_decode_error(decode_unsigned64(&[0x7f; 10])));
    }

    // ---- OBJECT IDENTIFIER ----------------------------------------------------------------

    #[test]
    fn oid_vectors() {
        let vectors: &[(&str, &[u8])] = &[
            // 40 * 1 + 3 = 0x2b
            (
                "1.3.6.1.2.1.1.1.0",
                &[0x06, 0x08, 0x2b, 0x06, 0x01, 0x02, 0x01, 0x01, 0x01, 0x00],
            ),
            // 8072 = 0xbf 0x08 in base 128
            (
                "1.3.6.1.4.1.8072.3.2.10",
                &[0x06, 0x0a, 0x2b, 0x06, 0x01, 0x04, 0x01, 0xbf, 0x08, 0x03, 0x02, 0x0a],
            ),
            // LLDP-MIB lives under 1.0.8802: 40 * 1 + 0 = 0x28, 8802 = 0xc4 0x62
            ("1.0.8802.1.1.2", &[0x06, 0x06, 0x28, 0xc4, 0x62, 0x01, 0x01, 0x02]),
            ("0.0", &[0x06, 0x01, 0x00]),
            ("0.39", &[0x06, 0x01, 0x27]),
            ("1.0", &[0x06, 0x01, 0x28]),
            ("1.39", &[0x06, 0x01, 0x4f]),
            ("2.0", &[0x06, 0x01, 0x50]),
            // 2.999: 80 + 999 = 1079 = 0x88 0x37 (the X.690 example)
            ("2.999.3", &[0x06, 0x03, 0x88, 0x37, 0x03]),
            ("1.3.127", &[0x06, 0x02, 0x2b, 0x7f]),
            ("1.3.128", &[0x06, 0x03, 0x2b, 0x81, 0x00]),
            ("1.3.16383", &[0x06, 0x03, 0x2b, 0xff, 0x7f]),
            ("1.3.16384", &[0x06, 0x04, 0x2b, 0x81, 0x80, 0x00]),
            (
                "1.3.4294967295",
                &[0x06, 0x06, 0x2b, 0x8f, 0xff, 0xff, 0xff, 0x7f],
            ),
            // 2.4294967295: 80 + 2^32 - 1 needs 33 bits.
            (
                "2.4294967295",
                &[0x06, 0x05, 0x90, 0x80, 0x80, 0x80, 0x4f],
            ),
        ];
        for &(text, bytes) in vectors {
            assert_eq!(oid_bytes(text), bytes, "encoding {text}");
            assert_eq!(Reader::new(bytes).oid().unwrap(), oid(text), "decoding {text}");
        }
    }

    #[test]
    fn short_oids_are_completed_with_zeros() {
        assert_eq!(encode(|w| w.oid(&Oid::default())), [0x06, 0x01, 0x00]);
        assert_eq!(oid_bytes("1"), [0x06, 0x01, 0x28]);
        assert_eq!(oid_bytes("2"), [0x06, 0x01, 0x50]);
    }

    #[test]
    fn oid_decoding_edge_cases() {
        // Zero length means zeroDotZero.
        assert_eq!(decode_oid(&[]).unwrap(), oid("0.0"));
        // Redundant 0x80 padding in a sub-identifier.
        assert_eq!(decode_oid(&[0x2b, 0x80, 0x80, 0x06]).unwrap(), oid("1.3.6"));
        // Truncated: the continuation bit is set on the last octet.
        assert!(is_decode_error(decode_oid(&[0x2b, 0x86])));
        assert!(is_decode_error(decode_oid(&[0x80])));
        // Sub-identifier beyond 32 bits.
        assert!(is_decode_error(decode_oid(&[0x2b, 0x90, 0x80, 0x80, 0x80, 0x00])));
        assert!(is_decode_error(decode_oid(&[0x2b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f])));
        // First group beyond 2.(2^32-1).
        assert!(is_decode_error(decode_oid(&[0x90, 0x80, 0x80, 0x80, 0x50])));
    }

    #[test]
    fn unencodable_oids_are_refused() {
        assert!(check_oid(&oid("1.3.6.1")).is_ok());
        assert!(check_oid(&oid("0.39")).is_ok());
        assert!(check_oid(&oid("2.4294967295")).is_ok());
        assert!(check_oid(&oid("1")).is_ok());
        assert!(check_oid(&Oid::default()).is_ok());
        for bad in ["3.1", "7", "1.40", "0.40.1"] {
            assert!(
                matches!(check_oid(&oid(bad)), Err(SnmpError::Unsupported(_))),
                "{bad} should be refused"
            );
        }
    }

    // ---- lengths --------------------------------------------------------------------------

    fn octet_string_of(len: usize) -> Vec<u8> {
        encode(|w| w.octet_string(&vec![0xab; len]))
    }

    #[test]
    fn length_boundaries() {
        assert_eq!(octet_string_of(0), [0x04, 0x00]);
        assert_eq!(&octet_string_of(1)[..2], [0x04, 0x01]);
        assert_eq!(&octet_string_of(127)[..2], [0x04, 0x7f]);
        assert_eq!(&octet_string_of(128)[..3], [0x04, 0x81, 0x80]);
        assert_eq!(&octet_string_of(255)[..3], [0x04, 0x81, 0xff]);
        assert_eq!(&octet_string_of(256)[..4], [0x04, 0x82, 0x01, 0x00]);
        assert_eq!(&octet_string_of(65_535)[..4], [0x04, 0x82, 0xff, 0xff]);
        assert_eq!(&octet_string_of(65_536)[..5], [0x04, 0x83, 0x01, 0x00, 0x00]);

        for len in [0, 1, 127, 128, 255, 256, 1000, 65_535, 65_536] {
            let bytes = octet_string_of(len);
            let mut reader = Reader::new(&bytes);
            let content = reader.octet_string().unwrap();
            assert_eq!(content.len(), len);
            assert!(reader.is_empty());
        }
    }

    #[test]
    fn constructed_lengths_are_patched_in() {
        let small = encode(|w| w.sequence(|w| w.integer(5)));
        assert_eq!(small, [0x30, 0x03, 0x02, 0x01, 0x05]);

        let empty = encode(|w| w.sequence(|_| {}));
        assert_eq!(empty, [0x30, 0x00]);

        // 127 bytes of content keep the short form, 128 need the long form.
        let fits = encode(|w| w.sequence(|w| w.octet_string(&[0; 125])));
        assert_eq!(&fits[..4], [0x30, 0x7f, 0x04, 0x7d]);
        let long = encode(|w| w.sequence(|w| w.octet_string(&[0; 126])));
        assert_eq!(&long[..5], [0x30, 0x81, 0x80, 0x04, 0x7e]);

        let nested = encode(|w| {
            w.sequence(|w| {
                w.integer(1);
                w.constructed(0xa0, |w| w.octet_string(&[0; 300]));
            })
        });
        assert_eq!(
            &nested[..12],
            [0x30, 0x82, 0x01, 0x37, 0x02, 0x01, 0x01, 0xa0, 0x82, 0x01, 0x30, 0x04]
        );
        let mut outer = Reader::new(&nested).sequence().unwrap();
        assert_eq!(outer.integer().unwrap(), 1);
        let mut inner = outer.expect(0xa0, "PDU").unwrap();
        assert_eq!(inner.octet_string().unwrap().len(), 300);
    }

    #[test]
    fn lengths_are_checked_strictly() {
        // Non-minimal long forms are accepted...
        let mut r = Reader::new(&[0x04, 0x81, 0x02, 0xaa, 0xbb]);
        assert_eq!(r.octet_string().unwrap(), [0xaa, 0xbb]);
        let mut r = Reader::new(&[0x04, 0x84, 0x00, 0x00, 0x00, 0x01, 0xaa]);
        assert_eq!(r.octet_string().unwrap(), [0xaa]);

        // ...but nothing may run past the end, be indefinite or be truncated.
        let bad: &[&[u8]] = &[
            &[],
            &[0x04],
            &[0x04, 0x01],
            &[0x04, 0x05, 0x01, 0x02],
            &[0x04, 0x80, 0x00, 0x00],
            &[0x04, 0x81],
            &[0x04, 0x82, 0x01],
            &[0x04, 0x82, 0x01, 0x00, 0x00],
            &[0x04, 0x84, 0xff, 0xff, 0xff, 0xff],
            &[0x04, 0x88, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            &[0x04, 0x89, 0x01, 0, 0, 0, 0, 0, 0, 0, 0],
            &[0x1f, 0x01, 0x00],
            &[0x3f, 0x81, 0x00],
        ];
        for bytes in bad {
            assert!(
                is_decode_error(Reader::new(bytes).read_tlv()),
                "{bytes:02x?} should be rejected"
            );
        }
    }

    #[test]
    fn reader_tracks_offsets_and_tags() {
        let bytes = [0x30, 0x06, 0x02, 0x01, 0x07, 0x04, 0x01, 0x41, 0xff];
        let mut top = Reader::new(&bytes);
        assert_eq!(top.peek_tag(), Some(0x30));
        let mut seq = top.sequence().unwrap();
        assert_eq!(top.offset(), 8);
        assert_eq!(top.remaining(), [0xff]);
        assert!(is_decode_error(top.finish("message")));

        assert_eq!(seq.offset(), 2);
        assert_eq!(seq.integer().unwrap(), 7);
        assert_eq!(seq.offset(), 5);
        let (tag, content) = seq.read_tlv().unwrap();
        assert_eq!(tag, tag::OCTET_STRING);
        assert_eq!(content.offset(), 7);
        assert_eq!(content.remaining(), b"A");
        assert!(seq.finish("sequence").is_ok());
        assert_eq!(seq.peek_tag(), None);

        // A wrong tag is reported, not misread.
        assert!(is_decode_error(Reader::new(&bytes).integer()));
        assert!(is_decode_error(Reader::new(&[0x02, 0x01, 0x00]).octet_string()));
        assert!(is_decode_error(Reader::new(&[0x04, 0x01, 0x00]).oid()));
    }
}
