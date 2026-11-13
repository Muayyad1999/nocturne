//! Decoder for NetFlow v5, NetFlow v9 (RFC 3954) and IPFIX (RFC 7011) export datagrams.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use serde::{Deserialize, Serialize};

use super::FlowRecord;
use crate::ProbeError;

/// Counters of a [`Decoder`].
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecoderStats {
    /// Export datagrams decoded without error.
    pub packets: u64,
    /// Flow records produced.
    pub records: u64,
    /// Templates currently known.
    pub templates: u64,
    /// Data sets dropped because their template had not arrived yet.
    pub missing_template_sets: u64,
    /// Datagrams rejected as malformed.
    pub errors: u64,
}

/// Stateful decoder: keeps v9 / IPFIX templates per (exporter, source id, template id).
#[derive(Debug, Default)]
pub struct Decoder {
    templates: HashMap<TemplateKey, Template>,
    /// The sampling rate each exporter announced in its options data.
    sampling: HashMap<(SocketAddr, u32), u32>,
    /// Counts template (re)definitions; orders templates by age for eviction.
    clock: u64,
    stats: DecoderStats,
}

/// The most templates kept at once. An exporter has a handful; this bound only matters when
/// someone sends junk, and then the oldest templates make room.
const MAX_TEMPLATES: usize = 4096;
/// More fields than any real template has.
const MAX_FIELDS: usize = 1024;

const V5_HEADER: usize = 24;
const V5_RECORD: usize = 48;
const V9_HEADER: usize = 20;
const IPFIX_HEADER: usize = 16;
/// "Variable length" marker in an IPFIX field specifier.
const VARIABLE: u16 = 0xffff;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TemplateKey {
    exporter: SocketAddr,
    version: u8,
    source_id: u32,
    template_id: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Field {
    id: u16,
    /// Enterprise-specific (IPFIX): the number space is the vendor's, so the field is skipped.
    enterprise: bool,
    /// Length in bytes; [`VARIABLE`] for IPFIX variable-length fields.
    length: u16,
}

#[derive(Debug, Clone)]
struct Template {
    fields: Vec<Field>,
    /// An options template: its records describe the exporter, not flows.
    options: bool,
    /// The fewest bytes one record can occupy.
    min_record: usize,
    /// When it was (re)defined, in decoder ticks; the oldest is evicted first.
    stamp: u64,
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder::default()
    }

    /// Decodes one export datagram: NetFlow v5, v9 or IPFIX.
    ///
    /// Templates are learned as they arrive; a data set whose template is not known yet is
    /// counted and dropped. Options templates are understood only as far as needed to skip
    /// their records and to pick up the exporter's sampling rate; enterprise-specific fields
    /// are skipped. A malformed datagram is an error and yields no records.
    pub fn decode(
        &mut self,
        datagram: &[u8],
        exporter: SocketAddr,
    ) -> Result<Vec<FlowRecord>, ProbeError> {
        let result = match datagram
            .get(..2)
            .map(|version| u16::from_be_bytes([version[0], version[1]]))
        {
            Some(5) => decode_v5(datagram, exporter),
            Some(9) => self.decode_v9(datagram, exporter),
            Some(10) => self.decode_ipfix(datagram, exporter),
            Some(version) => Err(malformed(format!(
                "version {version} is not NetFlow v5, v9 or IPFIX"
            ))),
            None => Err(malformed("the datagram is shorter than a version field")),
        };
        match &result {
            Ok(records) => {
                self.stats.packets += 1;
                self.stats.records += records.len() as u64;
            }
            Err(_) => self.stats.errors += 1,
        }
        self.stats.templates = self.templates.len() as u64;
        result
    }

    pub fn stats(&self) -> DecoderStats {
        self.stats
    }

    fn decode_v9(
        &mut self,
        datagram: &[u8],
        exporter: SocketAddr,
    ) -> Result<Vec<FlowRecord>, ProbeError> {
        let header = datagram
            .get(..V9_HEADER)
            .ok_or_else(|| malformed("the NetFlow v9 header is incomplete"))?;
        let source_id = be32(&header[16..20]);
        let context = Context {
            exporter,
            version: 9,
            source_id,
            export_secs: be32(&header[8..12]),
        };
        self.decode_sets(&datagram[V9_HEADER..], &context, 0, 1)
    }

    fn decode_ipfix(
        &mut self,
        datagram: &[u8],
        exporter: SocketAddr,
    ) -> Result<Vec<FlowRecord>, ProbeError> {
        let header = datagram
            .get(..IPFIX_HEADER)
            .ok_or_else(|| malformed("the IPFIX message header is incomplete"))?;
        let length = usize::from(be16(&header[2..4]));
        if length < IPFIX_HEADER || length > datagram.len() {
            return Err(malformed(format!(
                "the IPFIX message says it is {length} bytes long; the datagram has {}",
                datagram.len()
            )));
        }
        let context = Context {
            exporter,
            version: 10,
            source_id: be32(&header[12..16]),
            export_secs: be32(&header[4..8]),
        };
        self.decode_sets(&datagram[IPFIX_HEADER..length], &context, 2, 3)
    }

    /// Walks the sets (flow sets) of a v9 or IPFIX message. The two formats differ in the ids
    /// of their template sets and in details of the template records.
    fn decode_sets(
        &mut self,
        mut body: &[u8],
        context: &Context,
        template_set: u16,
        options_set: u16,
    ) -> Result<Vec<FlowRecord>, ProbeError> {
        let mut records = Vec::new();
        // Fewer than four bytes at the end are padding.
        while body.len() >= 4 {
            let set_id = be16(&body[..2]);
            let length = usize::from(be16(&body[2..4]));
            if length < 4 || length > body.len() {
                return Err(malformed(format!(
                    "set {set_id} claims {length} bytes; {} are left in the datagram",
                    body.len()
                )));
            }
            let (set, rest) = body.split_at(length);
            let content = &set[4..];
            body = rest;
            if set_id == template_set {
                self.learn_templates(content, context, false)?;
            } else if set_id == options_set {
                self.learn_templates(content, context, true)?;
            } else if set_id >= 256 {
                self.decode_data(content, set_id, context, &mut records);
            }
            // Other ids are reserved; a decoder skips what it does not know.
        }
        Ok(records)
    }

    /// Reads the template records of a template set.
    fn learn_templates(
        &mut self,
        mut content: &[u8],
        context: &Context,
        options: bool,
    ) -> Result<(), ProbeError> {
        let ipfix = context.version == 10;
        // What is left after the last record is padding (up to three bytes, zero).
        while content.len() >= 4 && content.iter().any(|byte| *byte != 0) {
            let template_id = be16(&content[..2]);
            let truncated = || malformed(format!("template {template_id} is cut off"));
            let (field_count, mut cursor) = match (options, ipfix) {
                // IPFIX: field count, then (options) how many of them are scope fields.
                (false, _) => (usize::from(be16(&content[2..4])), 4),
                (true, true) => {
                    let count = usize::from(be16(&content[2..4]));
                    // A withdrawal has no scope count.
                    (count, if count == 0 { 4 } else { 6 })
                }
                // NetFlow v9 options: scope and option lengths in bytes, four per field.
                (true, false) => {
                    let lengths = content.get(2..6).ok_or_else(truncated)?;
                    let bytes =
                        usize::from(be16(&lengths[..2])) + usize::from(be16(&lengths[2..4]));
                    (bytes / 4, 6)
                }
            };
            let key = TemplateKey {
                exporter: context.exporter,
                version: context.version,
                source_id: context.source_id,
                template_id,
            };
            if ipfix && field_count == 0 {
                // Template withdrawal; ids 2 and 3 withdraw every (options) template.
                if template_id < 256 {
                    self.templates.retain(|known, template| {
                        !(known.exporter == key.exporter
                            && known.version == 10
                            && known.source_id == key.source_id
                            && template.options == options)
                    });
                } else {
                    self.templates.remove(&key);
                }
                content = content.get(cursor..).ok_or_else(truncated)?;
                continue;
            }
            if field_count > MAX_FIELDS {
                return Err(malformed(format!(
                    "template {template_id} declares {field_count} fields"
                )));
            }
            let mut fields = Vec::with_capacity(field_count);
            let mut min_record = 0usize;
            for _ in 0..field_count {
                let specifier = content.get(cursor..cursor + 4).ok_or_else(truncated)?;
                cursor += 4;
                let raw_id = be16(&specifier[..2]);
                let length = be16(&specifier[2..4]);
                let enterprise = ipfix && raw_id & 0x8000 != 0;
                if enterprise {
                    // The enterprise number follows; the field itself is skipped when decoding.
                    content.get(cursor..cursor + 4).ok_or_else(truncated)?;
                    cursor += 4;
                }
                let variable = ipfix && length == VARIABLE;
                min_record += if variable { 1 } else { usize::from(length) };
                fields.push(Field {
                    id: if ipfix { raw_id & 0x7fff } else { raw_id },
                    enterprise,
                    length,
                });
            }
            content = content.get(cursor..).ok_or_else(truncated)?;
            if template_id < 256 {
                // Not a valid template id; its data sets could never be told from other sets.
                continue;
            }
            self.clock += 1;
            if !self.templates.contains_key(&key) && self.templates.len() >= MAX_TEMPLATES {
                let oldest = self
                    .templates
                    .iter()
                    .min_by_key(|(_, template)| template.stamp)
                    .map(|(key, _)| *key);
                if let Some(oldest) = oldest {
                    self.templates.remove(&oldest);
                }
            }
            self.templates.insert(
                key,
                Template {
                    fields,
                    options,
                    min_record,
                    stamp: self.clock,
                },
            );
        }
        Ok(())
    }

    /// Decodes the records of a data set with its template.
    fn decode_data(
        &mut self,
        mut content: &[u8],
        template_id: u16,
        context: &Context,
        records: &mut Vec<FlowRecord>,
    ) {
        let key = TemplateKey {
            exporter: context.exporter,
            version: context.version,
            source_id: context.source_id,
            template_id,
        };
        let Some(template) = self.templates.get(&key) else {
            self.stats.missing_template_sets += 1;
            return;
        };
        if template.min_record == 0 {
            // A template of zero-length fields describes nothing (and would never end).
            return;
        }
        let source = (context.exporter, context.source_id);
        let has_addresses = template
            .fields
            .iter()
            .any(|field| !field.enterprise && matches!(field.id, 8 | 12 | 27 | 28));
        // What remains when no further record fits is padding.
        while content.len() >= template.min_record {
            let mut draft = Draft::default();
            let mut complete = true;
            for field in &template.fields {
                let Some((value, rest)) = take_field(content, field.length) else {
                    complete = false;
                    break;
                };
                content = rest;
                if !field.enterprise {
                    draft.set(field.id, value);
                }
            }
            if !complete {
                // A variable-length field ran past the set: the rest cannot be interpreted.
                break;
            }
            if template.options {
                if let Some(rate) = draft.sampling.filter(|rate| *rate > 0) {
                    self.sampling.insert(source, rate);
                }
            } else if has_addresses {
                let announced = self.sampling.get(&source).copied().unwrap_or(0);
                records.push(draft.into_record(context, announced));
            }
        }
    }
}

/// What a v9 / IPFIX message header says about all its records.
struct Context {
    exporter: SocketAddr,
    version: u8,
    source_id: u32,
    /// The exporter's clock when it sent the message (Unix seconds).
    export_secs: u32,
}

/// Splits one field value off the front of a data record.
fn take_field(content: &[u8], length: u16) -> Option<(&[u8], &[u8])> {
    if length != VARIABLE {
        return content.split_at_checked(usize::from(length));
    }
    // RFC 7011 section 7: one length byte, or 255 followed by a two-byte length.
    let (&first, rest) = content.split_first()?;
    if first < 255 {
        return rest.split_at_checked(usize::from(first));
    }
    let (long, rest) = rest.split_at_checked(2)?;
    rest.split_at_checked(usize::from(be16(long)))
}

/// The fields of a record as they are collected from a data record.
#[derive(Default)]
struct Draft {
    src: Option<IpAddr>,
    dst: Option<IpAddr>,
    next_hop: Option<IpAddr>,
    src_port: u16,
    dst_port: u16,
    icmp_type_code: Option<u16>,
    protocol: u8,
    tos: u8,
    tcp_flags: u8,
    bytes: Option<u64>,
    packets: Option<u64>,
    fallback_bytes: Option<u64>,
    fallback_packets: Option<u64>,
    input_if: u32,
    output_if: u32,
    src_as: u32,
    dst_as: u32,
    sampling: Option<u32>,
    /// Flow start / end relative to the exporter's boot (milliseconds).
    start_uptime: Option<u64>,
    end_uptime: Option<u64>,
    /// Flow start / end as Unix milliseconds.
    start_epoch: Option<u64>,
    end_epoch: Option<u64>,
    /// Flow start / end as microseconds before the export time.
    start_delta_micros: Option<u64>,
    end_delta_micros: Option<u64>,
    /// When the exporter booted (Unix milliseconds).
    boot_epoch: Option<u64>,
    duration: Option<u64>,
}

impl Draft {
    /// Stores the value of information element `id` (IANA IPFIX numbers, which NetFlow v9
    /// shares for everything used here).
    fn set(&mut self, id: u16, value: &[u8]) {
        let number = unsigned(value);
        match id {
            1 => self.bytes = Some(number),
            2 => self.packets = Some(number),
            4 => self.protocol = number as u8,
            5 => self.tos = number as u8,
            6 => self.tcp_flags = number as u8,
            7 => self.src_port = number as u16,
            8 | 27 => self.src = address(value).or(self.src),
            10 => self.input_if = number as u32,
            11 => self.dst_port = number as u16,
            12 | 28 => self.dst = address(value).or(self.dst),
            14 => self.output_if = number as u32,
            15 | 62 => {
                self.next_hop = address(value)
                    .filter(|hop| !hop.is_unspecified())
                    .or(self.next_hop)
            }
            16 => self.src_as = number as u32,
            17 => self.dst_as = number as u32,
            21 => self.end_uptime = Some(number),
            22 => self.start_uptime = Some(number),
            // Post-NAT / egress counters and totals stand in when the delta counters are absent.
            23 | 85 => self.fallback_bytes = self.fallback_bytes.or(Some(number)),
            24 | 86 => self.fallback_packets = self.fallback_packets.or(Some(number)),
            32 | 139 => self.icmp_type_code = Some(number as u16),
            34 | 50 => self.sampling = Some(number.min(u64::from(u32::MAX)) as u32),
            150 => self.start_epoch = Some(number.saturating_mul(1000)),
            151 => self.end_epoch = Some(number.saturating_mul(1000)),
            152 => self.start_epoch = Some(number),
            153 => self.end_epoch = Some(number),
            154 | 156 => self.start_epoch = ntp_to_epoch_millis(value).or(self.start_epoch),
            155 | 157 => self.end_epoch = ntp_to_epoch_millis(value).or(self.end_epoch),
            158 => self.start_delta_micros = Some(number),
            159 => self.end_delta_micros = Some(number),
            160 => self.boot_epoch = Some(number),
            161 => self.duration = Some(number),
            _ => {}
        }
    }

    fn into_record(self, context: &Context, announced_sampling: u32) -> FlowRecord {
        let export_millis = u64::from(context.export_secs) * 1000;
        // Absolute times win; uptime-relative ones become absolute when the record also says
        // when the exporter booted; otherwise they stay relative, as in NetFlow v5.
        let resolve = |epoch: Option<u64>, delta_micros: Option<u64>, uptime: Option<u64>| {
            epoch
                .or(delta_micros.map(|delta| export_millis.saturating_sub(delta / 1000)))
                .or(uptime.map(|uptime| {
                    self.boot_epoch
                        .map_or(uptime, |boot| boot.saturating_add(uptime))
                }))
        };
        let start_ms = resolve(self.start_epoch, self.start_delta_micros, self.start_uptime);
        let end_ms = resolve(self.end_epoch, self.end_delta_micros, self.end_uptime).or(start_ms
            .zip(self.duration)
            .map(|(start, duration)| start.saturating_add(duration)));
        FlowRecord {
            exporter: context.exporter.ip().to_string(),
            version: context.version,
            src: self.src.map(|src| src.to_string()).unwrap_or_default(),
            dst: self.dst.map(|dst| dst.to_string()).unwrap_or_default(),
            src_port: self.src_port,
            // As NetFlow v5 does: ICMP type and code travel in the destination port.
            dst_port: match self.icmp_type_code {
                Some(type_code) if self.dst_port == 0 => type_code,
                _ => self.dst_port,
            },
            protocol: self.protocol,
            bytes: self.bytes.or(self.fallback_bytes).unwrap_or(0),
            packets: self.packets.or(self.fallback_packets).unwrap_or(0),
            tos: self.tos,
            tcp_flags: self.tcp_flags,
            input_if: self.input_if,
            output_if: self.output_if,
            src_as: self.src_as,
            dst_as: self.dst_as,
            next_hop: self.next_hop.map(|hop| hop.to_string()),
            start_ms: start_ms.unwrap_or(0),
            end_ms: end_ms.unwrap_or(0),
            sampling: self.sampling.unwrap_or(announced_sampling),
        }
    }
}

fn decode_v5(datagram: &[u8], exporter: SocketAddr) -> Result<Vec<FlowRecord>, ProbeError> {
    let header = datagram
        .get(..V5_HEADER)
        .ok_or_else(|| malformed("the NetFlow v5 header is incomplete"))?;
    let count = usize::from(be16(&header[2..4]));
    // Cisco's exporters put at most 30 records in a datagram; none may hold zero.
    if count == 0 || count > 30 {
        return Err(malformed(format!(
            "a NetFlow v5 datagram cannot hold {count} records"
        )));
    }
    let records = datagram
        .get(V5_HEADER..V5_HEADER + count * V5_RECORD)
        .ok_or_else(|| {
            malformed(format!(
                "the NetFlow v5 header announces {count} records; the datagram is too short for them"
            ))
        })?;
    // The low 14 bits are the interval; the top two the sampling mode.
    let sampling = u32::from(be16(&header[22..24]) & 0x3fff);
    Ok(records
        .chunks_exact(V5_RECORD)
        .map(|record| {
            let ipv4 = |offset: usize| {
                Ipv4Addr::new(
                    record[offset],
                    record[offset + 1],
                    record[offset + 2],
                    record[offset + 3],
                )
            };
            let next_hop = ipv4(8);
            FlowRecord {
                exporter: exporter.ip().to_string(),
                version: 5,
                src: ipv4(0).to_string(),
                dst: ipv4(4).to_string(),
                src_port: be16(&record[32..34]),
                dst_port: be16(&record[34..36]),
                protocol: record[38],
                bytes: u64::from(be32(&record[20..24])),
                packets: u64::from(be32(&record[16..20])),
                tos: record[39],
                tcp_flags: record[37],
                input_if: u32::from(be16(&record[12..14])),
                output_if: u32::from(be16(&record[14..16])),
                src_as: u32::from(be16(&record[40..42])),
                dst_as: u32::from(be16(&record[42..44])),
                next_hop: (!next_hop.is_unspecified()).then(|| next_hop.to_string()),
                start_ms: u64::from(be32(&record[24..28])),
                end_ms: u64::from(be32(&record[28..32])),
                sampling,
            }
        })
        .collect())
}

fn malformed(reason: impl Into<String>) -> ProbeError {
    ProbeError::protocol(format!("malformed flow export: {}", reason.into()))
}

fn be16(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}

fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// A big-endian unsigned integer of any length; of a longer value the low 64 bits.
fn unsigned(value: &[u8]) -> u64 {
    value
        .iter()
        .rev()
        .take(8)
        .rev()
        .fold(0u64, |number, byte| (number << 8) | u64::from(*byte))
}

fn address(value: &[u8]) -> Option<IpAddr> {
    if let Ok(v4) = <[u8; 4]>::try_from(value) {
        return Some(IpAddr::V4(Ipv4Addr::from(v4)));
    }
    <[u8; 16]>::try_from(value)
        .ok()
        .map(|v6| IpAddr::V6(Ipv6Addr::from(v6)))
}

/// An NTP-format timestamp (seconds since 1900 and a 32-bit fraction), as IPFIX uses for
/// micro- and nanosecond times, to Unix milliseconds.
fn ntp_to_epoch_millis(value: &[u8]) -> Option<u64> {
    const NTP_TO_UNIX_SECS: u64 = 2_208_988_800;
    let bytes: [u8; 8] = value.try_into().ok()?;
    let seconds = u64::from(be32(&bytes[..4]));
    let fraction = u64::from(be32(&bytes[4..]));
    let millis = seconds.checked_sub(NTP_TO_UNIX_SECS)? * 1000 + ((fraction * 1000) >> 32);
    Some(millis)
}
