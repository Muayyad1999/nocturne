//! What voice calls cost on a link.
//!
//! The arithmetic is the one both Cisco documents print, and nothing more:
//!
//! ```text
//! packet   = layer-2 header + IP/UDP/RTP header + voice payload      (bytes)
//! rate     = 1000 ms / milliseconds of voice in one packet           (packets per second)
//! per call = packet * 8 * rate                                       (bits per second)
//! ```
//!
//! The IP/UDP/RTP header is 20 + 8 + 12 = 40 bytes, or 2 bytes where the
//! link compresses it (cRTP). What varies is the payload, which the codec
//! decides, and the layer-2 header, which the link decides — and those two
//! tables are the part of this module that could be *wrong* rather than
//! merely buggy. So every row is copied from a published table and names it:
//!
//! * [`SOURCE_PER_CALL`] — the codec table (sample size and interval, default
//!   payload, MOS), the 40-byte and 2-byte headers, Ethernet at 18 bytes,
//!   Multilink PPP and FRF.12 at 6 bytes plus a 1-byte flag, and what it says
//!   about voice-activity detection.
//! * [`SOURCE_SRND`] — PPP at 6 bytes, Frame Relay at 4, an MPLS label at 4
//!   and an 802.11 header at 24 (its table 3-8).
//!
//! The tests at the bottom reproduce those tables cell by cell. A codec or a
//! link that neither document tabulates is not offered: for a link, the
//! caller can give its own byte count instead (`custom`).
//!
//! Voice-activity detection is the one figure here that is not arithmetic.
//! It is an average the document quotes with a warning attached, so it is
//! returned *beside* the full figure with that warning, never in its place.

use serde::{Deserialize, Serialize};

use crate::ToolError;
use crate::convert::format_rate;

/// Where the codec table, the header sizes, the Ethernet and Multilink PPP
/// overheads and the voice-activity-detection figure come from. The title is
/// the one the page carries at this revision; its table is headed "VoIP - Per
/// Call Bandwidth".
pub const SOURCE_PER_CALL: &str = "Cisco, \"Modify Bandwidth Consumption Calculation for Voice Calls\", Document ID 7934, \
     revision 3.0 of 11 December 2023 \
     (https://www.cisco.com/c/en/us/support/docs/voice/voice-quality/7934-bwidth-consume.html)";

/// Where the PPP, Frame Relay, MPLS and wireless LAN overheads come from.
pub const SOURCE_SRND: &str = "Cisco Collaboration System 12.x SRND, chapter 3 \"Network Infrastructure\", \
     revised 1 March 2018, tables 3-7 and 3-8 \
     (https://www.cisco.com/c/en/us/td/docs/voice_ip_comm/cucm/srnd/collab12/collab12/netstruc.pdf)";

/// Where the size of a VLAN tag comes from; neither Cisco document has that row.
const SOURCE_DOT1Q: &str =
    "IEEE Std 802.1Q: a VLAN tag is 4 bytes (a 2-byte TPID and 2 bytes of tag control information)";

/// The most simultaneous calls [`voice`] works out.
pub const MAX_CALLS: u32 = 1_000_000;

/// The most voice [`voice`] puts in one packet, in milliseconds. Cisco IOS
/// stops at 230 ms for G.729 ([`SOURCE_PER_CALL`]); nothing is packetised
/// more coarsely than that.
pub const MAX_PACKET_MS: u32 = 240;

/// The largest layer-2 overhead accepted for a `custom` link, in bytes.
pub const MAX_L2_BYTES: u32 = 256;

/// The most voice-activity detection saves, in percent, "as an average on a
/// volume of more than 24 calls" ([`SOURCE_PER_CALL`]).
pub const VAD_SAVING_PCT: u32 = 35;

/// Below this many calls the document says the saving must not be counted on.
const VAD_MIN_CALLS: u32 = 24;

const IPV4_HEADER: u32 = 20;
const UDP_HEADER: u32 = 8;
const RTP_HEADER: u32 = 12;
/// What cRTP leaves of the 40 bytes when UDP checksums are off — the figure
/// the document's table uses. With checksums it is [`CRTP_HEADER_CHECKSUM`].
const CRTP_HEADER: u32 = 2;
const CRTP_HEADER_CHECKSUM: u32 = 4;

/// The longest codec or link name worth reading.
const MAX_INPUT_CHARS: usize = 64;

/// A codec [`voice`] knows: one row of the codec table of [`SOURCE_PER_CALL`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct VoiceCodec {
    /// The name to pass to [`voice`]: `g711`, `g729`, `g723.1-6.3`, ...
    pub id: String,
    /// The name as it is written: `G.729`.
    pub name: String,
    /// The nominal bit rate. Informative: the calculation uses the sample
    /// size, which for G.723.1 is rounded up to whole bytes.
    pub bit_rate_bps: u32,
    /// Bytes the codec produces per sample interval.
    pub sample_bytes: u32,
    /// The codec's sample interval in milliseconds; a packet holds a whole
    /// number of these.
    pub sample_ms: u32,
    /// Voice per packet by default, in milliseconds.
    pub packet_ms: u32,
    /// Voice payload per packet at that default, in bytes.
    pub payload_bytes: u32,
    /// The mean opinion score the document gives, where it gives one.
    pub mos: Option<f64>,
}

/// A link type [`voice`] knows, with what it adds to every packet.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct VoiceLink {
    /// The name to pass to [`voice`]: `ethernet`, `ppp`, `mpls`, ...
    pub id: String,
    /// The name as it is written: `Multilink PPP`.
    pub name: String,
    /// Layer-2 bytes added to each packet.
    pub overhead_bytes: u32,
    /// What those bytes are: `MP header 6 + End-of-frame flag 1`.
    pub parts: String,
    /// Whether compressed RTP is counted on this kind of link: the documents
    /// have it on PPP, Multilink PPP and Frame Relay, and rule out Ethernet.
    pub crtp: bool,
    /// Which document the figure is from.
    pub source: String,
    /// What the figure leaves out or disagrees with; empty when there is nothing to say.
    pub note: String,
}

/// The options of [`voice`]. The default is none of them.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct VoiceOptions {
    /// Compressed RTP on the link: 2 bytes of IP/UDP/RTP header instead of 40.
    pub crtp: bool,
    /// Also estimate what voice-activity detection saves.
    pub vad: bool,
    /// Milliseconds of voice per packet, when not the codec's default.
    pub packet_ms: Option<u32>,
    /// Layer-2 bytes per packet for the `custom` link, and only for it.
    pub l2_bytes: Option<u32>,
}

/// One part of a voice packet.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PacketPart {
    /// What it is: `Ethernet header`, `IPv4 header`, `Voice payload`, ...
    pub name: String,
    /// How many bytes it is.
    pub bytes: u32,
}

/// What voice-activity detection might save. An estimate, and labelled as one.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct VadEstimate {
    /// The saving assumed, in percent: the most the document says to expect.
    pub saving_pct: u32,
    /// The average per call with that saving, in bits per second: 65 % of
    /// the [`VoiceBandwidth::per_call_bps`] shown, to the nearest bit.
    pub per_call_bps: u64,
    /// The total with that saving, in bits per second: 65 % of the
    /// [`VoiceBandwidth::total_bps`] shown, to the nearest bit.
    pub total_bps: u64,
    /// `per_call_bps` for reading: `17.42 kbps`.
    pub per_call: String,
    /// `total_bps` for reading.
    pub total: String,
    /// Whether there are enough calls (more than 24) for the average to mean anything.
    pub enough_calls: bool,
    /// The document's warning, in words that fit this request.
    pub caveat: String,
}

/// The outcome of [`voice`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct VoiceBandwidth {
    /// The codec's row of the table.
    pub codec: VoiceCodec,
    /// The link's row of the table (for `custom`, the bytes that were given).
    pub link: VoiceLink,
    /// Simultaneous calls.
    pub calls: u32,
    /// Whether the IP/UDP/RTP header is compressed.
    pub crtp: bool,
    /// Milliseconds of voice in each packet.
    pub packet_ms: u32,
    /// Packets per second per call, to three decimals: `50.0`, `33.333`.
    pub packets_per_second: f64,
    /// Voice payload in each packet, in bytes.
    pub payload_bytes: u32,
    /// The packet, part by part, in the order the arithmetic adds them:
    /// layer 2, then IP/UDP/RTP, then the voice payload.
    pub packet: Vec<PacketPart>,
    /// The sum of `packet`.
    pub packet_bytes: u32,
    /// One call in one direction, in bits per second, rounded to the nearest one.
    pub per_call_bps: u64,
    /// All the calls, worked out from the exact per-call figure and then rounded.
    pub total_bps: u64,
    /// `per_call_bps` for reading: `26.8 kbps`.
    pub per_call: String,
    /// `total_bps` for reading.
    pub total: String,
    /// The voice-activity-detection estimate, when it was asked for.
    pub vad: Option<VadEstimate>,
    /// The calculation, one line per step, so that it can be checked by hand.
    pub arithmetic: Vec<String>,
    /// What the figure covers and what it leaves out.
    pub notes: Vec<String>,
    /// The documents the figures were taken from.
    pub sources: Vec<String>,
}

struct Codec {
    id: &'static str,
    name: &'static str,
    /// Other spellings, written the way [`squash`] leaves them.
    aliases: &'static [&'static str],
    bit_rate_bps: u32,
    sample_bytes: u32,
    sample_ms: u32,
    packet_ms: u32,
    mos: Option<f64>,
}

/// The codec table of [`SOURCE_PER_CALL`], in the document's order. The
/// document leaves the MOS of G.726 at 24 kbps blank and gives "NA" for iLBC.
#[rustfmt::skip]
static CODECS: [Codec; 10] = [
    Codec { id: "g711", name: "G.711", aliases: &["g711u", "g711a", "g711ulaw", "g711alaw", "g711mulaw", "pcmu", "pcma"], bit_rate_bps: 64_000, sample_bytes: 80, sample_ms: 10, packet_ms: 20, mos: Some(4.1) },
    // Annex A is the same bit stream from a simpler encoder and Annex B adds
    // silence suppression; neither changes the size of a frame.
    Codec { id: "g729", name: "G.729", aliases: &["g729a", "g729b", "g729ab", "g729r8", "g729ar8", "g729br8", "g729abr8"], bit_rate_bps: 8_000, sample_bytes: 10, sample_ms: 10, packet_ms: 20, mos: Some(3.92) },
    Codec { id: "g723.1-6.3", name: "G.723.1 (6.3 kbps)", aliases: &["g72363", "g723r63", "g723ar63"], bit_rate_bps: 6_300, sample_bytes: 24, sample_ms: 30, packet_ms: 30, mos: Some(3.9) },
    Codec { id: "g723.1-5.3", name: "G.723.1 (5.3 kbps)", aliases: &["g72353", "g723r53", "g723ar53"], bit_rate_bps: 5_300, sample_bytes: 20, sample_ms: 30, packet_ms: 30, mos: Some(3.8) },
    Codec { id: "g726-32", name: "G.726 (32 kbps)", aliases: &["g726r32"], bit_rate_bps: 32_000, sample_bytes: 20, sample_ms: 5, packet_ms: 20, mos: Some(3.85) },
    Codec { id: "g726-24", name: "G.726 (24 kbps)", aliases: &["g726r24"], bit_rate_bps: 24_000, sample_bytes: 15, sample_ms: 5, packet_ms: 20, mos: None },
    Codec { id: "g728", name: "G.728", aliases: &["g72816", "g728r16"], bit_rate_bps: 16_000, sample_bytes: 10, sample_ms: 5, packet_ms: 30, mos: Some(3.61) },
    Codec { id: "g722", name: "G.722 (64 kbps)", aliases: &["g72264", "g72264k", "g722r64"], bit_rate_bps: 64_000, sample_bytes: 80, sample_ms: 10, packet_ms: 20, mos: Some(4.13) },
    Codec { id: "ilbc-20", name: "iLBC (20 ms mode, 15.2 kbps)", aliases: &["ilbcmode20", "ilbc152"], bit_rate_bps: 15_200, sample_bytes: 38, sample_ms: 20, packet_ms: 20, mos: None },
    Codec { id: "ilbc-30", name: "iLBC (30 ms mode, 13.33 kbps)", aliases: &["ilbcmode30", "ilbc1333"], bit_rate_bps: 13_330, sample_bytes: 50, sample_ms: 30, packet_ms: 30, mos: None },
];

/// Names that mean two codecs, and the two they could mean.
const AMBIGUOUS_CODECS: [(&str, &str); 4] = [
    ("g723", "`g723.1-6.3` or `g723.1-5.3`"),
    ("g7231", "`g723.1-6.3` or `g723.1-5.3`"),
    ("g726", "`g726-32` or `g726-24`"),
    ("ilbc", "`ilbc-20` or `ilbc-30`"),
];

struct Link {
    id: &'static str,
    name: &'static str,
    /// Other spellings, written the way [`squash`] leaves them.
    aliases: &'static [&'static str],
    /// What the link adds to a packet, part by part.
    parts: &'static [(&'static str, u32)],
    /// Whether compressed RTP is counted on it. Only "not available over
    /// Ethernet" is a sentence of [`SOURCE_PER_CALL`]; wireless and MPLS are
    /// `false` because neither document has cRTP there, not because one says
    /// it cannot be done — and the refusal is worded accordingly.
    crtp: bool,
    /// The short name of the document, for the table.
    source: &'static str,
    /// The full citations, for the result.
    citations: &'static [&'static str],
    note: &'static str,
}

/// The id of the link whose overhead the caller supplies.
const CUSTOM_LINK: &str = "custom";

static LINKS: [Link; 9] = [
    Link {
        id: "ethernet",
        name: "Ethernet",
        aliases: &["eth", "lan"],
        parts: &[("Ethernet header", 14), ("Ethernet FCS", 4)],
        crtp: false,
        source: "Cisco 7934",
        citations: &[SOURCE_PER_CALL],
        note: "The frame as Cisco counts it: without the preamble and the inter-frame gap. \
               The Cisco SRND table counts the 14-byte header alone.",
    },
    Link {
        id: "dot1q",
        name: "Ethernet with an 802.1Q tag",
        aliases: &["8021q", "vlan", "trunk"],
        parts: &[
            ("Ethernet header", 14),
            ("802.1Q tag", 4),
            ("Ethernet FCS", 4),
        ],
        crtp: false,
        source: "Cisco 7934 + IEEE 802.1Q",
        citations: &[SOURCE_PER_CALL, SOURCE_DOT1Q],
        note: "Cisco's Ethernet figure plus the tag; neither Cisco document prints this row.",
    },
    Link {
        id: "ppp",
        name: "PPP",
        aliases: &[],
        parts: &[("PPP header", 6)],
        crtp: true,
        source: "Cisco SRND 12.x, table 3-8",
        citations: &[SOURCE_SRND],
        note: "",
    },
    Link {
        id: "mlppp",
        name: "Multilink PPP",
        aliases: &["mlp", "mp", "multilink", "multilinkppp"],
        parts: &[("MP header", 6), ("End-of-frame flag", 1)],
        crtp: true,
        source: "Cisco 7934",
        citations: &[SOURCE_PER_CALL],
        note: "The Cisco SRND table counts 10 bytes for MLPPP; use the `custom` link with 10 bytes for that figure.",
    },
    Link {
        id: "frame-relay",
        name: "Frame Relay",
        aliases: &["fr"],
        parts: &[("Frame Relay header", 4)],
        crtp: true,
        source: "Cisco SRND 12.x, table 3-8",
        citations: &[SOURCE_SRND],
        note: "",
    },
    Link {
        id: "frf12",
        name: "Frame Relay with FRF.12 fragmentation",
        aliases: &[],
        parts: &[("FRF.12 header", 6), ("End-of-frame flag", 1)],
        crtp: true,
        source: "Cisco 7934",
        citations: &[SOURCE_PER_CALL],
        note: "",
    },
    Link {
        id: "mpls",
        name: "MPLS",
        aliases: &[],
        parts: &[("MPLS label", 4)],
        crtp: false,
        source: "Cisco SRND 12.x, table 3-8",
        citations: &[SOURCE_SRND],
        note: "One label and nothing else, as Cisco's table counts it: the frame that carries the labelled packet is not included.",
    },
    Link {
        id: "wlan",
        name: "802.11 wireless LAN",
        aliases: &["wifi", "80211", "wireless"],
        parts: &[("802.11 header", 24)],
        crtp: false,
        source: "Cisco SRND 12.x, table 3-8",
        citations: &[SOURCE_SRND],
        note: "",
    },
    Link {
        id: "none",
        name: "No layer 2 (the IP packet only)",
        aliases: &["ip", "l3", "layer3"],
        parts: &[],
        crtp: true,
        source: "Cisco 7934; Cisco SRND 12.x, table 3-7",
        citations: &[SOURCE_PER_CALL, SOURCE_SRND],
        note: "The IP packet alone, as Cisco's layer-3 figures count it; provisioning a link has to add its header.",
    },
];

/// The codecs [`voice`] knows, in the order of the document's table.
pub fn voice_codecs() -> Vec<VoiceCodec> {
    CODECS.iter().map(Codec::row).collect()
}

/// The link types [`voice`] knows. `custom`, whose overhead the caller
/// gives, is not a row: it has no figure of its own to show.
pub fn voice_links() -> Vec<VoiceLink> {
    LINKS.iter().map(Link::row).collect()
}

/// Work out the bandwidth of `calls` simultaneous calls of `codec` over a
/// `link`.
///
/// * `codec` — an id from [`voice_codecs`], in any common spelling (`G.729`,
///   `g729a`, `g729r8`). A name that means two rates (`g723`, `g726`, `ilbc`)
///   is refused with the two to choose from.
/// * `calls` — 1 to [`MAX_CALLS`].
/// * `link` — an id from [`voice_links`], or `custom` with
///   [`VoiceOptions::l2_bytes`] for a link the table does not have.
/// * [`VoiceOptions::crtp`] — refused on links the documents do not have
///   header compression on (Ethernet, 802.1Q, wireless, MPLS).
/// * [`VoiceOptions::packet_ms`] — a whole number of the codec's sample
///   intervals, up to [`MAX_PACKET_MS`].
/// * [`VoiceOptions::vad`] — adds [`VoiceBandwidth::vad`]; the main figures
///   do not change, because the document says not to provision on it.
///
/// The figure is for one direction of the call and for the voice packets
/// alone: no signalling, no RTCP.
pub fn voice(
    codec: &str,
    calls: u32,
    link: &str,
    options: &VoiceOptions,
) -> Result<VoiceBandwidth, ToolError> {
    let codec = Codec::find(codec)?;
    let (link, link_parts, citations) = find_link(link, options.l2_bytes)?;
    if calls == 0 {
        return Err(ToolError::invalid(
            "number of calls",
            "0",
            "there has to be at least one call to work out",
        ));
    }
    if calls > MAX_CALLS {
        return Err(ToolError::TooLarge(format!(
            "{calls} simultaneous calls is more than this calculator works out; the most is {MAX_CALLS}"
        )));
    }
    let packet_ms = codec.packet_ms_or(options.packet_ms)?;
    if options.crtp && !link.crtp {
        return Err(ToolError::invalid(
            "link type",
            &link.id,
            format!(
                "compressed RTP is not counted on {}: Cisco has it hop by hop on PPP, Multilink PPP and Frame Relay, \
                 and not over Ethernet. If this link does compress the header, use `{CUSTOM_LINK}` and give its layer-2 bytes",
                link.name
            ),
        ));
    }

    let frames = packet_ms / codec.sample_ms;
    let payload_bytes = frames * codec.sample_bytes;
    let mut packet = link_parts;
    if options.crtp {
        packet.push(part("cRTP-compressed IP/UDP/RTP header", CRTP_HEADER));
    } else {
        packet.push(part("IPv4 header", IPV4_HEADER));
        packet.push(part("UDP header", UDP_HEADER));
        packet.push(part("RTP header", RTP_HEADER));
    }
    packet.push(part("Voice payload", payload_bytes));
    let packet_bytes: u32 = packet.iter().map(|part| part.bytes).sum();

    // Bits per second as an exact fraction: `bits_per_s_times_ms / packet_ms`.
    // Kept apart until the very end so that the total of 33.3-packet codecs
    // is the rounded sum and not the sum of rounded calls.
    let per_call_numerator = u64::from(packet_bytes) * 8 * 1000;
    let divisor = u64::from(packet_ms);
    let per_call_bps = rounded(per_call_numerator, divisor);
    let total_numerator = per_call_numerator * u64::from(calls);
    let total_bps = rounded(total_numerator, divisor);

    // 33.33 packets a second is a rounded number, and a line that multiplies
    // by it does not come to the figure beside it. Where the rate is not
    // whole the lines divide by the milliseconds instead, which is exact,
    // and say so when the result itself had to be rounded.
    let whole_rate = 1000_u32.is_multiple_of(packet_ms);
    let rate = if whole_rate {
        (1000 / packet_ms).to_string()
    } else {
        format!("{:.2}", 1000.0 / f64::from(packet_ms))
    };
    let nearest = |numerator: u64| nearest_of(numerator, divisor);
    let mut arithmetic = vec![
        format!(
            "Voice payload: {packet_ms} ms ÷ {} ms per sample × {} bytes = {payload_bytes} bytes",
            codec.sample_ms, codec.sample_bytes
        ),
        format!(
            "Packet: {} = {packet_bytes} bytes",
            packet
                .iter()
                .map(|part| format!("{} ({})", part.bytes, part.name))
                .collect::<Vec<_>>()
                .join(" + ")
        ),
        format!("Packet rate: 1000 ms ÷ {packet_ms} ms = {rate} packets per second"),
        if whole_rate {
            format!(
                "Per call: {packet_bytes} bytes × 8 bits × {rate} packets per second = {per_call_bps} bps"
            )
        } else {
            format!(
                "Per call: {packet_bytes} bytes × 8 bits × 1000 ÷ {packet_ms} per second = {per_call_bps} bps{}",
                nearest(per_call_numerator)
            )
        },
        if per_call_numerator.is_multiple_of(divisor) {
            format!("Total: {calls} × {per_call_bps} bps = {total_bps} bps")
        } else {
            format!(
                "Total: {calls} × {packet_bytes} bytes × 8 bits × 1000 ÷ {packet_ms} per second = {total_bps} bps{}",
                nearest(total_numerator)
            )
        },
    ];

    let vad = options.vad.then(|| {
        // Taken from the figures shown, not from the exact fraction behind
        // them: whoever checks the line multiplies the number in front of
        // them, and half a bit either way is nothing to an estimate.
        let kept = u64::from(100 - VAD_SAVING_PCT);
        let per_call = rounded(per_call_bps * kept, 100);
        let total = rounded(total_bps * kept, 100);
        arithmetic.push(format!(
            "With voice-activity detection, at best {VAD_SAVING_PCT} % less: {total_bps} bps × {kept} % = {total} bps{}",
            nearest_of(total_bps * kept, 100)
        ));
        let enough_calls = calls > VAD_MIN_CALLS;
        let caveat = if enough_calls {
            format!(
                "Up to {VAD_SAVING_PCT} % is an average over time across more than {VAD_MIN_CALLS} calls, never a promise for one call, \
                 and music on hold and fax defeat it. Cisco's advice is to provision the full figure and let data use what VAD frees."
            )
        } else {
            format!(
                "Up to {VAD_SAVING_PCT} % is an average across more than {VAD_MIN_CALLS} calls; with {calls} it must not be counted on. \
                 Provision the full figure."
            )
        };
        VadEstimate {
            saving_pct: VAD_SAVING_PCT,
            per_call_bps: per_call,
            total_bps: total,
            per_call: format_rate(per_call as f64),
            total: format_rate(total as f64),
            enough_calls,
            caveat,
        }
    });

    let mut notes = vec![
        "One direction of each call; the other direction costs the same again.".to_owned(),
        "Voice packets only: call signalling and RTCP are not included.".to_owned(),
    ];
    if !link.note.is_empty() {
        notes.push(link.note.clone());
    }
    if options.crtp {
        // 2 more bytes in every packet, in bits per second, to the nearest one.
        let extra = rounded(
            u64::from(CRTP_HEADER_CHECKSUM - CRTP_HEADER) * 8 * 1000,
            divisor,
        );
        notes.push(format!(
            "cRTP is counted at {CRTP_HEADER} bytes, as in Cisco's table. With UDP checksums it is {CRTP_HEADER_CHECKSUM} bytes: \
             {extra} bps more per call. Both ends of the link have to run it."
        ));
    }
    if packet_ms > 30 {
        notes.push(format!(
            "Cisco's SRND says a sampling rate above 30 ms usually results in very poor voice quality: \
             {packet_ms} ms of voice in a packet is that much delay before it is sent."
        ));
    }

    let mut sources = vec![SOURCE_PER_CALL.to_owned()];
    for citation in citations {
        if !sources.iter().any(|known| known == citation) {
            sources.push((*citation).to_owned());
        }
    }
    if packet_ms > 30 && !sources.iter().any(|known| known == SOURCE_SRND) {
        sources.push(SOURCE_SRND.to_owned());
    }

    Ok(VoiceBandwidth {
        codec: codec.row(),
        link,
        calls,
        crtp: options.crtp,
        packet_ms,
        packets_per_second: (1_000_000.0 / f64::from(packet_ms)).round() / 1000.0,
        payload_bytes,
        packet,
        packet_bytes,
        per_call_bps,
        total_bps,
        per_call: format_rate(per_call_bps as f64),
        total: format_rate(total_bps as f64),
        vad,
        arithmetic,
        notes,
        sources,
    })
}

impl Codec {
    fn row(&self) -> VoiceCodec {
        VoiceCodec {
            id: self.id.to_owned(),
            name: self.name.to_owned(),
            bit_rate_bps: self.bit_rate_bps,
            sample_bytes: self.sample_bytes,
            sample_ms: self.sample_ms,
            packet_ms: self.packet_ms,
            payload_bytes: self.packet_ms / self.sample_ms * self.sample_bytes,
            mos: self.mos,
        }
    }

    fn find(input: &str) -> Result<&'static Codec, ToolError> {
        const WHAT: &str = "codec";
        let text = input.trim();
        let known = || {
            CODECS
                .iter()
                .map(|codec| codec.id)
                .collect::<Vec<_>>()
                .join(", ")
        };
        if text.is_empty() {
            return Err(ToolError::invalid(
                WHAT,
                "",
                format!("say which one: {}", known()),
            ));
        }
        if text.chars().count() > MAX_INPUT_CHARS {
            return Err(ToolError::invalid(
                WHAT,
                text,
                "too long to be a codec name",
            ));
        }
        let wanted = squash(text);
        if let Some((_, choices)) = AMBIGUOUS_CODECS.iter().find(|(name, _)| *name == wanted) {
            return Err(ToolError::invalid(
                WHAT,
                text,
                format!("that name covers two bit rates; say which: {choices}"),
            ));
        }
        CODECS
            .iter()
            .find(|codec| squash(codec.id) == wanted || codec.aliases.contains(&wanted.as_str()))
            .ok_or_else(|| {
                ToolError::invalid(
                    WHAT,
                    text,
                    format!(
                        "not one this calculator has figures for; it has {}",
                        known()
                    ),
                )
            })
    }

    /// The voice per packet to use: the one asked for if a packet can hold
    /// exactly that, the codec's default if nothing was asked.
    fn packet_ms_or(&self, asked: Option<u32>) -> Result<u32, ToolError> {
        let Some(packet_ms) = asked else {
            return Ok(self.packet_ms);
        };
        let refuse = |reason: String| {
            ToolError::invalid("voice per packet", &format!("{packet_ms} ms"), reason)
        };
        if packet_ms == 0 {
            return Err(refuse("a packet has to hold some voice".to_owned()));
        }
        if packet_ms > MAX_PACKET_MS {
            return Err(refuse(format!(
                "more than {MAX_PACKET_MS} ms of voice in one packet is not something a phone or a gateway sends"
            )));
        }
        if !packet_ms.is_multiple_of(self.sample_ms) {
            return Err(refuse(format!(
                "{} produces {} bytes every {} ms, so a packet holds a multiple of {} ms; its default is {} ms",
                self.name, self.sample_bytes, self.sample_ms, self.sample_ms, self.packet_ms
            )));
        }
        Ok(packet_ms)
    }
}

impl Link {
    fn overhead(&self) -> u32 {
        self.parts.iter().map(|(_, bytes)| bytes).sum()
    }

    fn row(&self) -> VoiceLink {
        VoiceLink {
            id: self.id.to_owned(),
            name: self.name.to_owned(),
            overhead_bytes: self.overhead(),
            parts: if self.parts.is_empty() {
                "nothing".to_owned()
            } else {
                self.parts
                    .iter()
                    .map(|(name, bytes)| format!("{name} {bytes}"))
                    .collect::<Vec<_>>()
                    .join(" + ")
            },
            crtp: self.crtp,
            source: self.source.to_owned(),
            note: self.note.to_owned(),
        }
    }
}

/// The link a name means: its row, what it adds to a packet, and where its
/// figure is from. `l2_bytes` belongs to `custom` and to nothing else — a
/// number that silently replaced a named link's figure would be an answer to
/// a question nobody asked.
fn find_link(
    input: &str,
    l2_bytes: Option<u32>,
) -> Result<(VoiceLink, Vec<PacketPart>, &'static [&'static str]), ToolError> {
    const WHAT: &str = "link type";
    let text = input.trim();
    let known = || {
        LINKS
            .iter()
            .map(|link| link.id)
            .chain([CUSTOM_LINK])
            .collect::<Vec<_>>()
            .join(", ")
    };
    if text.is_empty() {
        return Err(ToolError::invalid(
            WHAT,
            "",
            format!("say which one: {}", known()),
        ));
    }
    if text.chars().count() > MAX_INPUT_CHARS {
        return Err(ToolError::invalid(WHAT, text, "too long to be a link type"));
    }
    let wanted = squash(text);

    if wanted == CUSTOM_LINK {
        let Some(bytes) = l2_bytes else {
            return Err(ToolError::invalid(
                WHAT,
                text,
                "a custom link needs its layer-2 bytes per packet; give them in `l2_bytes`",
            ));
        };
        if bytes > MAX_L2_BYTES {
            return Err(ToolError::invalid(
                "layer-2 overhead",
                &format!("{bytes} bytes"),
                format!(
                    "more than {MAX_L2_BYTES} bytes in front of every packet is not a link header"
                ),
            ));
        }
        let link = VoiceLink {
            id: CUSTOM_LINK.to_owned(),
            name: "Custom layer 2".to_owned(),
            overhead_bytes: bytes,
            parts: format!("Layer 2, as given {bytes}"),
            // Whoever knows the link's header size knows whether it compresses.
            crtp: true,
            source: "given with the request".to_owned(),
            note: String::new(),
        };
        return Ok((link, vec![part("Layer 2, as given", bytes)], &[]));
    }

    let link = LINKS
        .iter()
        .find(|link| squash(link.id) == wanted || link.aliases.contains(&wanted.as_str()))
        .ok_or_else(|| {
            ToolError::invalid(
                WHAT,
                text,
                format!(
                    "not one this calculator has figures for; it has {}",
                    known()
                ),
            )
        })?;
    if let Some(bytes) = l2_bytes {
        return Err(ToolError::invalid(
            "layer-2 overhead",
            &format!("{bytes} bytes"),
            format!(
                "`l2_bytes` goes with the `{CUSTOM_LINK}` link only; `{}` already counts {} bytes",
                link.id,
                link.overhead()
            ),
        ));
    }
    let parts = link
        .parts
        .iter()
        .map(|(name, bytes)| part(name, *bytes))
        .collect();
    Ok((link.row(), parts, link.citations))
}

fn part(name: &str, bytes: u32) -> PacketPart {
    PacketPart {
        name: name.to_owned(),
        bytes,
    }
}

/// A name with everything but its letters and digits removed, in lower
/// case: `G.723.1-6.3`, `g723.1 6.3` and `G7231_63` are one codec.
fn squash(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// `numerator / divisor` to the nearest whole number, halves up.
fn rounded(numerator: u64, divisor: u64) -> u64 {
    (numerator + divisor / 2) / divisor
}

/// What a line of the arithmetic says after a figure that [`rounded`] had to
/// round, and nothing after one that came out whole.
fn nearest_of(numerator: u64, divisor: u64) -> &'static str {
    if numerator.is_multiple_of(divisor) {
        ""
    } else {
        ", to the nearest bit"
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn options() -> VoiceOptions {
        VoiceOptions::default()
    }

    fn crtp() -> VoiceOptions {
        VoiceOptions {
            crtp: true,
            ..options()
        }
    }

    fn custom(l2_bytes: u32) -> VoiceOptions {
        VoiceOptions {
            l2_bytes: Some(l2_bytes),
            ..options()
        }
    }

    fn at(packet_ms: u32) -> VoiceOptions {
        VoiceOptions {
            packet_ms: Some(packet_ms),
            ..options()
        }
    }

    fn one(codec: &str, link: &str, options: &VoiceOptions) -> VoiceBandwidth {
        voice(codec, 1, link, options)
            .unwrap_or_else(|error| panic!("{codec} over {link}: {error}"))
    }

    /// Bits per second the way a Cisco table prints them: kbps, to as many
    /// decimals as the printed figure has.
    fn printed_like(bps: u64, published: &str) -> String {
        let decimals = published.split_once('.').map_or(0, |(_, frac)| frac.len());
        format!("{:.decimals$}", bps as f64 / 1000.0)
    }

    fn refusal(codec: &str, calls: u32, link: &str, options: &VoiceOptions) -> String {
        voice(codec, calls, link, options)
            .expect_err(&format!("{codec} x{calls} over {link} should be refused"))
            .to_string()
    }

    /// The table "VoIP - Per Call Bandwidth" of Cisco document 7934
    /// (`SOURCE_PER_CALL`), as fetched from www.cisco.com on 2026-11-22:
    /// codec, voice payload in bytes, voice payload in ms, packets per
    /// second, and the three bandwidth columns in kbps — MP or FRF.12,
    /// MP or FRF.12 with cRTP, Ethernet. The document leaves the payload
    /// cell of G.726 at 24 kbps empty; 60 is 20 ms of its 15-byte samples.
    #[rustfmt::skip]
    const CISCO_7934_TABLE: [(&str, u32, u32, &str, &str, &str, &str); 10] = [
        ("g711",       160, 20, "50",   "82.8",   "67.6",  "87.2"),
        ("g729",        20, 20, "50",   "26.8",   "11.6",  "31.2"),
        ("g723.1-6.3",  24, 30, "33.3", "18.9",   "8.8",   "21.9"),
        ("g723.1-5.3",  20, 30, "33.3", "17.9",   "7.7",   "20.8"),
        ("g726-32",     80, 20, "50",   "50.8",   "35.6",  "55.2"),
        ("g726-24",     60, 20, "50",   "42.8",   "27.6",  "47.2"),
        ("g728",        60, 30, "33.3", "28.5",   "18.4",  "31.5"),
        ("g722",       160, 20, "50",   "82.8",   "67.6",  "87.2"),
        ("ilbc-20",     38, 20, "50",   "34.0",   "18.8",  "38.4"),
        ("ilbc-30",     50, 30, "33.3", "25.867", "15.73", "28.8"),
    ];

    #[test]
    fn the_per_call_table_of_cisco_document_7934_is_reproduced_to_the_last_digit() {
        for (codec, payload, ms, pps, multilink, multilink_crtp, ethernet) in CISCO_7934_TABLE {
            let plain = one(codec, "mlppp", &options());
            assert_eq!(plain.payload_bytes, payload, "{codec} payload");
            assert_eq!(plain.packet_ms, ms, "{codec} ms");
            assert_eq!(
                format!("{:.1}", plain.packets_per_second).trim_end_matches(".0"),
                pps,
                "{codec} pps"
            );
            assert_eq!(
                printed_like(plain.per_call_bps, multilink),
                multilink,
                "{codec} over MP"
            );
            // "MP or FRF.12": one column for both.
            assert_eq!(
                one(codec, "frf12", &options()).per_call_bps,
                plain.per_call_bps,
                "{codec} over FRF.12"
            );
            assert_eq!(
                printed_like(one(codec, "mlppp", &crtp()).per_call_bps, multilink_crtp),
                multilink_crtp,
                "{codec} over MP with cRTP"
            );
            assert_eq!(
                printed_like(one(codec, "frf12", &crtp()).per_call_bps, multilink_crtp),
                multilink_crtp,
                "{codec} over FRF.12 with cRTP"
            );
            assert_eq!(
                printed_like(one(codec, "ethernet", &options()).per_call_bps, ethernet),
                ethernet,
                "{codec} over Ethernet"
            );
        }
    }

    #[test]
    fn the_codec_rows_are_the_ones_cisco_document_7934_prints() {
        // The "Codec Information" half of the same table: bit rate in kbps,
        // sample size in bytes, sample interval in ms, MOS (blank for G.726
        // at 24 kbps, "NA" for iLBC).
        #[rustfmt::skip]
        let published: [(&str, f64, u32, u32, Option<f64>); 10] = [
            ("g711",       64.0,  80, 10, Some(4.1)),
            ("g729",        8.0,  10, 10, Some(3.92)),
            ("g723.1-6.3",  6.3,  24, 30, Some(3.9)),
            ("g723.1-5.3",  5.3,  20, 30, Some(3.8)),
            ("g726-32",    32.0,  20,  5, Some(3.85)),
            ("g726-24",    24.0,  15,  5, None),
            ("g728",       16.0,  10,  5, Some(3.61)),
            ("g722",       64.0,  80, 10, Some(4.13)),
            ("ilbc-20",    15.2,  38, 20, None),
            ("ilbc-30",    13.33, 50, 30, None),
        ];
        let table = voice_codecs();
        assert_eq!(table.len(), published.len());
        for (row, (id, kbps, sample_bytes, sample_ms, mos)) in table.iter().zip(published) {
            assert_eq!(row.id, id);
            assert_eq!(f64::from(row.bit_rate_bps) / 1000.0, kbps, "{id}");
            assert_eq!(
                (row.sample_bytes, row.sample_ms, row.mos),
                (sample_bytes, sample_ms, mos),
                "{id}"
            );
            let (_, payload, ms, ..) = CISCO_7934_TABLE
                .iter()
                .find(|(codec, ..)| *codec == id)
                .unwrap();
            assert_eq!((row.payload_bytes, row.packet_ms), (*payload, *ms), "{id}");
        }
    }

    #[test]
    fn the_sample_calculation_of_cisco_document_7934_comes_to_11_2_kbps() {
        // "Sample Calculation" in the same document: "the required bandwidth
        // for a G.729 call (8 Kbps codec bit rate) with cRTP, MP, and the
        // default 20 bytes of voice payload is: Total packet size (bytes) =
        // (MP header of 6 bytes) + (compressed IP/UDP/RTP header of 2 bytes)
        // + (voice payload of 20 bytes) = 28 bytes ... 224 bits ... PPS = 50
        // pps ... Bandwidth per call = voice packet size (224 bits) * 50 pps
        // = 11.2 Kbps".
        //
        // The sample leaves out the one-byte end-of-frame flag that the
        // document's own header assumptions list and its table includes
        // (11.6 kbps), so it is the 6 bytes alone that reproduce it.
        let sample = one(
            "g729",
            "custom",
            &VoiceOptions {
                crtp: true,
                l2_bytes: Some(6),
                ..options()
            },
        );
        assert_eq!(sample.payload_bytes, 20);
        assert_eq!(sample.packet_bytes, 28);
        assert_eq!(sample.packet_bytes * 8, 224);
        assert_eq!(sample.packets_per_second, 50.0);
        assert_eq!(sample.per_call_bps, 11_200);
        assert_eq!(sample.per_call, "11.2 kbps");
        assert_eq!(
            sample.arithmetic,
            [
                "Voice payload: 20 ms ÷ 10 ms per sample × 10 bytes = 20 bytes",
                "Packet: 6 (Layer 2, as given) + 2 (cRTP-compressed IP/UDP/RTP header) + 20 (Voice payload) = 28 bytes",
                "Packet rate: 1000 ms ÷ 20 ms = 50 packets per second",
                "Per call: 28 bytes × 8 bits × 50 packets per second = 11200 bps",
                "Total: 1 × 11200 bps = 11200 bps",
            ]
        );
        // The flag is the whole difference to the table's figure: one byte
        // in fifty packets a second is 400 bps.
        let table = one("g729", "mlppp", &crtp());
        assert_eq!(table.packet_bytes, 29);
        assert_eq!(table.per_call_bps - sample.per_call_bps, 400);
    }

    #[test]
    fn doubling_the_payload_halves_the_packet_rate_as_in_cisco_document_7934() {
        // "Impact of a Change to Voice Payload Sizes": "G.729 call with voice
        // payload size of 20 bytes (20 ms): (40 bytes of IP/UDP/RTP headers +
        // 20 bytes voice payload) * 8 bits per byte * 50 pps = 24 Kbps" and
        // "... 40 bytes (40 ms): (40 + 40) * 8 * 25 pps = 16 Kbps", with the
        // note "L2 headers are not considered in this calculation".
        let default = one("g729", "none", &options());
        assert_eq!(
            (
                default.payload_bytes,
                default.packets_per_second,
                default.per_call_bps
            ),
            (20, 50.0, 24_000)
        );
        let doubled = one("g729", "none", &at(40));
        assert_eq!(
            (
                doubled.payload_bytes,
                doubled.packets_per_second,
                doubled.per_call_bps
            ),
            (40, 25.0, 16_000)
        );
        // The same section maps milliseconds to bytes: G.711 at 30 ms is 240
        // bytes and G.729 at 30 ms is 30.
        assert_eq!(one("g711", "none", &at(30)).payload_bytes, 240);
        assert_eq!(one("g729", "none", &at(30)).payload_bytes, 30);
    }

    #[test]
    fn the_layer_3_table_of_the_cisco_srnd_is_reproduced() {
        // Table 3-7, "Bandwidth Consumption for Voice Payload and IP Header
        // Only", of `SOURCE_SRND` (fetched 2026-11-22), the rows without
        // SRTP: codec, sampling rate in ms, voice payload in bytes, bandwidth
        // per conversation in kbps. Its G.729A is this table's `g729`, and
        // its two iLBC rows are the codec's two modes.
        for (codec, ms, payload, kbps) in [
            ("g711", 20, 160, "80.0"),
            ("g722", 20, 160, "80.0"),
            ("g711", 30, 240, "74.7"),
            ("ilbc-20", 20, 38, "31.2"),
            ("ilbc-30", 30, 50, "24.0"),
            ("g729a", 20, 20, "24.0"),
            ("g729a", 30, 30, "18.7"),
        ] {
            let read = one(codec, "none", &at(ms));
            assert_eq!(read.payload_bytes, payload, "{codec} at {ms} ms");
            assert_eq!(
                printed_like(read.per_call_bps, kbps),
                kbps,
                "{codec} at {ms} ms"
            );
        }
    }

    #[test]
    fn the_layer_2_table_of_the_cisco_srnd_is_reproduced() {
        // Table 3-8, "Bandwidth Consumption with Layer 2 Headers Included",
        // of `SOURCE_SRND`, the rows without SRTP and every column but ATM
        // (which is not offered: the table does not say what it adds to a
        // packet before padding it to cells, and its figures are not what
        // AAL5's 8-byte trailer gives). Columns, in kbps: Ethernet 14 bytes,
        // PPP 6 bytes, Frame Relay 4 bytes, MLPPP 10 bytes, MPLS 4 bytes,
        // WLAN 24 bytes. Ethernet at 14 and MLPPP at 10 are that table's own
        // counts and differ from document 7934's, which this module's named
        // links follow; they are reproduced with the custom link.
        let columns: [(&str, VoiceOptions); 6] = [
            ("custom", custom(14)),
            ("ppp", options()),
            ("frame-relay", options()),
            ("custom", custom(10)),
            ("mpls", options()),
            ("wlan", options()),
        ];
        #[rustfmt::skip]
        let rows: [(&str, u32, [&str; 6]); 6] = [
            ("g711",    20, ["85.6", "82.4", "81.6", "84.0", "81.6", "89.6"]),
            ("g711",    30, ["78.4", "76.3", "75.7", "77.3", "75.7", "81.1"]),
            ("ilbc-20", 20, ["36.8", "33.6", "32.8", "35.2", "32.8", "40.8"]),
            ("ilbc-30", 30, ["27.7", "25.6", "25.0", "26.6", "25.0", "30.4"]),
            ("g729a",   20, ["29.6", "26.4", "25.6", "28.0", "25.6", "33.6"]),
            ("g729a",   30, ["22.4", "20.3", "19.7", "21.3", "19.8", "25.1"]),
        ];
        // Four cells of the 33.3-packet rows are not the formula's value
        // rounded to one decimal, and the table shows that the fault is its
        // own: Frame Relay and MPLS have the same four bytes, yet G.729A at
        // 30 ms is printed as 19.7 under one and 19.8 under the other, and
        // the iLBC row was worked out with 33.3 packets a second where the
        // rows around it used 33.33. Those four are held to within 0.07 kbps
        // — a quarter of what one byte more or less would change (0.27) —
        // and every other cell to the digit.
        let loose = [("ilbc-30", 2), ("ilbc-30", 3), ("ilbc-30", 4), ("g729a", 4)];
        let mut exact_cells = 0;
        for (codec, ms, published) in rows {
            for (column, ((link, link_options), kbps)) in columns.iter().zip(published).enumerate()
            {
                let read = one(
                    codec,
                    link,
                    &VoiceOptions {
                        packet_ms: Some(ms),
                        ..link_options.clone()
                    },
                );
                let cell = format!(
                    "{codec} at {ms} ms over {link} ({} bytes)",
                    read.link.overhead_bytes
                );
                if ms == 30 && loose.contains(&(codec, column)) {
                    let off =
                        (read.per_call_bps as f64 / 1000.0 - kbps.parse::<f64>().unwrap()).abs();
                    assert!(
                        off < 0.07,
                        "{cell}: {} bps against {kbps} kbps",
                        read.per_call_bps
                    );
                } else {
                    assert_eq!(printed_like(read.per_call_bps, kbps), kbps, "{cell}");
                    exact_cells += 1;
                }
            }
        }
        assert_eq!(exact_cells, 32);
    }

    #[test]
    fn the_links_carry_the_overheads_their_documents_print() {
        // 7934, "protocol header assumptions": "6 bytes for Multilink
        // Point-to-Point Protocol (MP) or Frame Relay Forum (FRF).12 Layer 2
        // (L2) header", "1 byte for the end-of-frame flag on MP and Frame
        // Relay frames", "18 bytes for Ethernet L2 headers, which include 4
        // bytes of Frame Check Sequence (FCS)". SRND table 3-8, header row:
        // "PPP 6 Bytes", "Frame Relay 4 Bytes", "MPLS 4 Bytes", "WLAN 24
        // Bytes". The tag of `dot1q` is IEEE 802.1Q's four bytes on top of
        // the Ethernet figure, and is not a row of either document.
        let links: Vec<(String, u32, bool)> = voice_links()
            .into_iter()
            .map(|link| (link.id, link.overhead_bytes, link.crtp))
            .collect();
        let expected = [
            ("ethernet", 18, false),
            ("dot1q", 22, false),
            ("ppp", 6, true),
            ("mlppp", 7, true),
            ("frame-relay", 4, true),
            ("frf12", 7, true),
            ("mpls", 4, false),
            ("wlan", 24, false),
            ("none", 0, true),
        ]
        .map(|(id, bytes, crtp)| (id.to_owned(), bytes, crtp));
        assert_eq!(links, expected);

        let ethernet = &voice_links()[0];
        assert_eq!(ethernet.parts, "Ethernet header 14 + Ethernet FCS 4");
        assert_eq!(ethernet.source, "Cisco 7934");
        assert_eq!(voice_links()[8].parts, "nothing");
        // A tagged frame is the untagged one plus four bytes in each of the
        // fifty packets a second: 4 * 8 * 50 = 1600 bps.
        assert_eq!(
            one("g711", "dot1q", &options()).per_call_bps
                - one("g711", "ethernet", &options()).per_call_bps,
            1600
        );
        // Every row says where it is from, and the result repeats it in full.
        for link in voice_links() {
            assert!(link.source.contains("Cisco"), "{}", link.id);
            let read = one("g729", &link.id, &options());
            assert_eq!(read.sources[0], SOURCE_PER_CALL);
            assert_eq!(
                read.sources.iter().any(|source| source == SOURCE_SRND),
                link.source.contains("SRND"),
                "{}",
                link.id
            );
        }
        assert!(
            one("g711", "dot1q", &options())
                .sources
                .iter()
                .any(|source| source.contains("802.1Q"))
        );
    }

    #[test]
    fn the_packet_is_shown_part_by_part_and_the_parts_are_the_answer() {
        let call = voice("g711", 10, "ethernet", &options()).unwrap();
        let parts: Vec<(&str, u32)> = call
            .packet
            .iter()
            .map(|part| (part.name.as_str(), part.bytes))
            .collect();
        assert_eq!(
            parts,
            [
                ("Ethernet header", 14),
                ("Ethernet FCS", 4),
                ("IPv4 header", 20),
                ("UDP header", 8),
                ("RTP header", 12),
                ("Voice payload", 160),
            ]
        );
        assert_eq!(call.packet_bytes, 218);
        // 218 bytes * 8 * 50 = 87 200 bps, and ten of them.
        assert_eq!((call.per_call_bps, call.total_bps), (87_200, 872_000));
        assert_eq!(
            (call.per_call.as_str(), call.total.as_str()),
            ("87.2 kbps", "872 kbps")
        );
        assert_eq!(
            call.arithmetic,
            [
                "Voice payload: 20 ms ÷ 10 ms per sample × 80 bytes = 160 bytes",
                "Packet: 14 (Ethernet header) + 4 (Ethernet FCS) + 20 (IPv4 header) + 8 (UDP header) + 12 (RTP header) + 160 (Voice payload) = 218 bytes",
                "Packet rate: 1000 ms ÷ 20 ms = 50 packets per second",
                "Per call: 218 bytes × 8 bits × 50 packets per second = 87200 bps",
                "Total: 10 × 87200 bps = 872000 bps",
            ]
        );
        assert_eq!(call.vad, None);

        // For every codec on every link the parts add up to the packet and
        // the packet gives the figure — nothing is added on the side.
        for codec in voice_codecs() {
            for link in voice_links() {
                let call = one(&codec.id, &link.id, &options());
                let sum: u32 = call.packet.iter().map(|part| part.bytes).sum();
                assert_eq!(sum, call.packet_bytes);
                assert_eq!(
                    sum,
                    link.overhead_bytes + 40 + codec.payload_bytes,
                    "{} over {}",
                    codec.id,
                    link.id
                );
                let exact = f64::from(sum) * 8.0 * 1000.0 / f64::from(call.packet_ms);
                assert_eq!(call.per_call_bps, exact.round() as u64);
                assert_eq!(call.codec, codec);
                assert_eq!(call.link, link);
            }
        }
    }

    #[test]
    fn a_total_is_the_exact_figure_multiplied_not_the_rounded_one() {
        // G.723.1 over Ethernet: 82 bytes * 8 * 1000 / 30 = 21 866.67 bps,
        // printed 21 867. Three calls are 65 600 bps exactly — not 3 * 21 867.
        let three = voice("g723.1-6.3", 3, "ethernet", &options()).unwrap();
        assert_eq!(three.packet_bytes, 82);
        assert_eq!(three.packets_per_second, 33.333);
        assert_eq!((three.per_call_bps, three.total_bps), (21_867, 65_600));
        assert_eq!(
            &three.arithmetic[2..],
            [
                "Packet rate: 1000 ms ÷ 30 ms = 33.33 packets per second",
                "Per call: 82 bytes × 8 bits × 1000 ÷ 30 per second = 21867 bps, to the nearest bit",
                "Total: 3 × 82 bytes × 8 bits × 1000 ÷ 30 per second = 65600 bps",
            ]
        );
        // Two calls do not: 43 733.33 bps.
        let two = voice("g723.1-6.3", 2, "ethernet", &options()).unwrap();
        assert_eq!(
            two.arithmetic[4],
            "Total: 2 × 82 bytes × 8 bits × 1000 ÷ 30 per second = 43733 bps, to the nearest bit"
        );
        // A line never multiplies by a rounded rate: iLBC at 30 ms over
        // Ethernet is 108 bytes * 8 * 1000 / 30 = 28 800 bps exactly, which
        // 108 * 8 * 33.33 is not.
        let whole = voice("ilbc-30", 2, "ethernet", &options()).unwrap();
        assert_eq!(
            &whole.arithmetic[2..],
            [
                "Packet rate: 1000 ms ÷ 30 ms = 33.33 packets per second",
                "Per call: 108 bytes × 8 bits × 1000 ÷ 30 per second = 28800 bps",
                "Total: 2 × 28800 bps = 57600 bps",
            ]
        );
        // The largest request does not overflow: G.711 with 240 ms in a
        // packet behind the largest custom header, a million times.
        let most = voice(
            "g711",
            MAX_CALLS,
            "custom",
            &VoiceOptions {
                packet_ms: Some(MAX_PACKET_MS),
                l2_bytes: Some(MAX_L2_BYTES),
                vad: true,
                ..options()
            },
        )
        .unwrap();
        // (256 + 40 + 1920) bytes * 8 * 1000 / 240 = 73 866.67 bps a call.
        assert_eq!(most.per_call_bps, 73_867);
        assert_eq!(most.total_bps, 73_866_666_667);
        assert_eq!(most.total, "73.87 Gbps");
    }

    #[test]
    fn compressed_rtp_takes_38_bytes_out_of_every_packet_where_the_link_has_it() {
        // 7934: "cRTP reduces the IP/UDP/RTP headers to 2 or 4 bytes (cRTP is
        // not available over Ethernet)"; its table uses 2.
        let plain = one("g729", "ppp", &options());
        let compressed = one("g729", "ppp", &crtp());
        assert_eq!(plain.packet_bytes - compressed.packet_bytes, 38);
        assert!(compressed.crtp && !plain.crtp);
        let parts: Vec<(&str, u32)> = compressed
            .packet
            .iter()
            .map(|part| (part.name.as_str(), part.bytes))
            .collect();
        assert_eq!(
            parts,
            [
                ("PPP header", 6),
                ("cRTP-compressed IP/UDP/RTP header", 2),
                ("Voice payload", 20),
            ]
        );
        // The 4-byte variant is two bytes more, fifty times a second: 800 bps.
        assert!(
            compressed
                .notes
                .iter()
                .any(|note| note.contains("4 bytes") && note.contains("800 bps more per call")),
            "{:?}",
            compressed.notes
        );
        assert!(!plain.notes.iter().any(|note| note.contains("cRTP")));

        // Only Ethernet is ruled out in so many words; for wireless and
        // MPLS the documents simply have no cRTP, and the refusal claims no
        // more than that.
        for link in ["ethernet", "dot1q", "wlan", "mpls"] {
            let refused = refusal("g729", 1, link, &crtp());
            assert!(
                refused.contains("compressed RTP is not counted on")
                    && refused.contains("PPP, Multilink PPP and Frame Relay")
                    && !refused.contains("does not exist"),
                "{link}: {refused}"
            );
        }
        assert_eq!(
            refusal("g729", 1, "ethernet", &crtp()),
            "invalid link type `ethernet`: compressed RTP is not counted on Ethernet: Cisco has it hop by hop on PPP, \
             Multilink PPP and Frame Relay, and not over Ethernet. If this link does compress the header, use `custom` \
             and give its layer-2 bytes"
        );
        for link in ["ppp", "mlppp", "frame-relay", "frf12", "none"] {
            assert!(voice("g729", 1, link, &crtp()).is_ok(), "{link}");
        }
    }

    #[test]
    fn the_everyday_cases_come_to_the_figures_worked_out_by_hand() {
        // Worked out with pencil from the two documents as fetched from
        // www.cisco.com on 2026-11-22, not from a run of this code. Headers:
        // IP/UDP/RTP 40 bytes, cRTP 2, Ethernet 18 with its FCS (7934), PPP 6
        // (SRND table 3-8). Payload at the default 20 ms: G.711 160 bytes,
        // G.729 20 bytes; 1000 / 20 = 50 packets a second, so a byte in the
        // packet is 8 * 50 = 400 bps.
        //
        //   G.711, Ethernet:    (18 + 40 + 160) = 218 bytes * 400 = 87 200  (7934 prints 87.2)
        //   G.729, Ethernet:    (18 + 40 +  20) =  78 bytes * 400 = 31 200  (7934 prints 31.2)
        //   G.711, PPP:         ( 6 + 40 + 160) = 206 bytes * 400 = 82 400  (SRND prints 82.4)
        //   G.729, PPP:         ( 6 + 40 +  20) =  66 bytes * 400 = 26 400  (SRND prints 26.4)
        //   G.711, PPP, cRTP:   ( 6 +  2 + 160) = 168 bytes * 400 = 67 200
        //   G.729, PPP, cRTP:   ( 6 +  2 +  20) =  28 bytes * 400 = 11 200  (7934's sample: 11.2)
        for (codec, link, options, packet_bytes, bps) in [
            ("g711", "ethernet", options(), 218, 87_200),
            ("g729", "ethernet", options(), 78, 31_200),
            ("g711", "ppp", options(), 206, 82_400),
            ("g729", "ppp", options(), 66, 26_400),
            ("g711", "ppp", crtp(), 168, 67_200),
            ("g729", "ppp", crtp(), 28, 11_200),
        ] {
            let call = one(codec, link, &options);
            assert_eq!(
                (call.packet_bytes, call.per_call_bps),
                (packet_bytes, bps),
                "{codec} over {link}, cRTP {}",
                options.crtp
            );
            // Twelve of them are twelve times that, to the bit.
            let dozen = voice(codec, 12, link, &options).unwrap();
            assert_eq!(dozen.total_bps, 12 * bps, "{codec} over {link}");
        }
        // And there is no figure for cRTP over Ethernet to work out: 7934
        // says "cRTP is not available over Ethernet".
        assert!(voice("g711", 1, "ethernet", &crtp()).is_err());
        assert!(voice("g729", 1, "ethernet", &crtp()).is_err());
    }

    #[test]
    fn the_voice_activity_line_multiplies_out_to_the_figure_beside_it() {
        // Two G.723.1 calls over Ethernet are 2 * 82 * 8 * 1000 / 30 =
        // 43 733.33 bps, shown as 43 733. Whoever checks the last line takes
        // 65 % of the number in front of them: 28 426.45, so 28 426 — which
        // 65 % of the unrounded figure (28 426.67, so 28 427) is not.
        let vad = VoiceOptions {
            vad: true,
            ..options()
        };
        let two = voice("g723.1-6.3", 2, "ethernet", &vad).unwrap();
        assert_eq!((two.per_call_bps, two.total_bps), (21_867, 43_733));
        assert_eq!(
            two.arithmetic.last().unwrap(),
            "With voice-activity detection, at best 35 % less: 43733 bps × 65 % = 28426 bps, to the nearest bit"
        );
        let estimate = two.vad.unwrap();
        assert_eq!(estimate.total_bps, 28_426);
        // Per call the same way: 65 % of the 21 867 shown is 14 213.55.
        assert_eq!(estimate.per_call_bps, 14_214);
        // Where 65 % is a whole number of bits nothing is said about rounding.
        let whole = voice("g729", 10, "mlppp", &vad).unwrap();
        assert_eq!(
            whole.arithmetic.last().unwrap(),
            "With voice-activity detection, at best 35 % less: 268000 bps × 65 % = 174200 bps"
        );
        // For every codec, link and a spread of call counts the line and
        // the estimate are 65 % of the totals the answer itself shows.
        for codec in voice_codecs() {
            for link in voice_links() {
                for calls in [1, 2, 3, 7, 24, 25, 1000] {
                    let answer = voice(&codec.id, calls, &link.id, &vad).unwrap();
                    let estimate = answer.vad.clone().unwrap();
                    // 65 hundredths, half a bit rounding up.
                    let by_hand = |bps: u64| (bps * 65 + 50) / 100;
                    assert_eq!(
                        (estimate.per_call_bps, estimate.total_bps),
                        (by_hand(answer.per_call_bps), by_hand(answer.total_bps)),
                        "{} x{calls} over {}",
                        codec.id,
                        link.id
                    );
                }
            }
        }
    }

    #[test]
    fn voice_activity_detection_is_an_estimate_beside_the_full_figure_never_instead_of_it() {
        // 7934, "Voice Activity Detection": "Over time and as an average on a
        // volume of more than 24 calls, VAD can provide up to a 35 percent
        // bandwidth savings. ... For the purposes of network design and
        // bandwidth engineering, VAD must not be taken into account,
        // especially on links that carry fewer than 24 voice calls
        // simultaneously."
        let vad = VoiceOptions {
            vad: true,
            ..options()
        };
        let without = voice("g729", 30, "mlppp", &options()).unwrap();
        let with = voice("g729", 30, "mlppp", &vad).unwrap();
        // The figure to provision is the same with or without the option.
        assert_eq!(
            (with.per_call_bps, with.total_bps),
            (without.per_call_bps, without.total_bps)
        );
        assert_eq!((with.per_call_bps, with.total_bps), (26_800, 804_000));
        let estimate = with.vad.clone().unwrap();
        // 65 % of 26 800 is 17 420; of 804 000, 522 600.
        assert_eq!(
            (
                estimate.saving_pct,
                estimate.per_call_bps,
                estimate.total_bps
            ),
            (35, 17_420, 522_600)
        );
        assert_eq!(
            (estimate.per_call.as_str(), estimate.total.as_str()),
            ("17.42 kbps", "522.6 kbps")
        );
        assert!(estimate.enough_calls);
        assert!(
            estimate.caveat.contains("provision the full figure"),
            "{}",
            estimate.caveat
        );
        assert_eq!(
            with.arithmetic.last().unwrap(),
            "With voice-activity detection, at best 35 % less: 804000 bps × 65 % = 522600 bps"
        );
        assert_eq!(with.arithmetic.len(), without.arithmetic.len() + 1);
        assert_eq!(without.vad, None);

        // Twenty-four calls are not "more than 24": the estimate is still
        // shown, and says it must not be used.
        let few = voice("g729", 24, "mlppp", &vad).unwrap().vad.unwrap();
        assert!(!few.enough_calls);
        assert_eq!(
            few.caveat,
            "Up to 35 % is an average across more than 24 calls; with 24 it must not be counted on. Provision the full figure."
        );
        assert!(
            voice("g729", 25, "mlppp", &vad)
                .unwrap()
                .vad
                .unwrap()
                .enough_calls
        );
    }

    #[test]
    fn a_codec_is_found_by_the_names_people_and_devices_use() {
        for (spelling, id) in [
            ("g711", "g711"),
            ("G.711", "g711"),
            (" g711ulaw ", "g711"),
            ("PCMA", "g711"),
            ("G.729", "g729"),
            ("G.729A", "g729"),
            ("g729r8", "g729"),
            ("G.723.1-6.3", "g723.1-6.3"),
            ("g723.1 6.3", "g723.1-6.3"),
            ("g723r53", "g723.1-5.3"),
            ("G.726-32", "g726-32"),
            ("g726r24", "g726-24"),
            ("G.728", "g728"),
            ("G722_64k", "g722"),
            ("ilbc_mode_20", "ilbc-20"),
            ("iLBC-30", "ilbc-30"),
        ] {
            assert_eq!(one(spelling, "none", &options()).codec.id, id, "{spelling}");
        }
        for (spelling, id) in [
            ("Ethernet", "ethernet"),
            ("802.1Q", "dot1q"),
            ("vlan", "dot1q"),
            ("PPP", "ppp"),
            ("MLP", "mlppp"),
            ("frame relay", "frame-relay"),
            ("FRF.12", "frf12"),
            ("MPLS", "mpls"),
            ("wifi", "wlan"),
            ("ip", "none"),
        ] {
            assert_eq!(one("g729", spelling, &options()).link.id, id, "{spelling}");
        }
    }

    #[test]
    fn a_name_that_covers_two_bit_rates_is_refused_with_the_two() {
        assert_eq!(
            refusal("G.723", 1, "ethernet", &options()),
            "invalid codec `G.723`: that name covers two bit rates; say which: `g723.1-6.3` or `g723.1-5.3`"
        );
        assert!(refusal("g723.1", 1, "ethernet", &options()).contains("`g723.1-5.3`"));
        assert!(refusal("g726", 1, "ethernet", &options()).contains("`g726-32` or `g726-24`"));
        assert!(refusal("iLBC", 1, "ethernet", &options()).contains("`ilbc-20` or `ilbc-30`"));
    }

    #[test]
    fn a_codec_or_link_without_published_figures_is_refused_with_the_ones_there_are() {
        // Opus is variable-rate and in neither document: no row, no guess.
        assert_eq!(
            refusal("opus", 1, "ethernet", &options()),
            "invalid codec `opus`: not one this calculator has figures for; it has g711, g729, g723.1-6.3, \
             g723.1-5.3, g726-32, g726-24, g728, g722, ilbc-20, ilbc-30"
        );
        assert_eq!(
            refusal("g729", 1, "atm", &options()),
            "invalid link type `atm`: not one this calculator has figures for; it has ethernet, dot1q, ppp, \
             mlppp, frame-relay, frf12, mpls, wlan, none, custom"
        );
        assert!(refusal("", 1, "ethernet", &options()).contains("say which one: g711"));
        assert!(refusal("g729", 1, " ", &options()).contains("say which one: ethernet"));
        assert!(refusal(&"g".repeat(65), 1, "ethernet", &options()).contains("too long"));
        assert!(refusal("g729", 1, &"e".repeat(65), &options()).contains("too long"));
    }

    #[test]
    fn a_number_of_calls_that_is_nothing_or_absurd_is_refused() {
        assert_eq!(
            refusal("g729", 0, "ethernet", &options()),
            "invalid number of calls `0`: there has to be at least one call to work out"
        );
        assert_eq!(
            voice("g729", MAX_CALLS + 1, "ethernet", &options()),
            Err(ToolError::TooLarge(
                "1000001 simultaneous calls is more than this calculator works out; the most is 1000000"
                    .into()
            ))
        );
        assert!(voice("g729", MAX_CALLS, "ethernet", &options()).is_ok());
        assert!(voice("g729", u32::MAX, "ethernet", &options()).is_err());
    }

    #[test]
    fn a_packet_holds_whole_samples_and_not_more_than_a_phone_would_send() {
        // 7934: "The voice payload size must be a multiple of the codec
        // sample size. For example, G.729 packets can use 10, 20, 30, 40,
        // 50, or 60 bytes of voice payload size."
        for (ms, bytes) in [(10, 10), (20, 20), (30, 30), (40, 40), (50, 50), (60, 60)] {
            assert_eq!(one("g729", "none", &at(ms)).payload_bytes, bytes);
        }
        assert_eq!(
            refusal("g729", 1, "ethernet", &at(25)),
            "invalid voice per packet `25 ms`: G.729 produces 10 bytes every 10 ms, so a packet holds a multiple of 10 ms; its default is 20 ms"
        );
        // G.723.1 frames are 30 ms: 20 ms of it cannot be sent.
        assert!(refusal("g723.1-6.3", 1, "ethernet", &at(20)).contains("multiple of 30 ms"));
        assert_eq!(one("g723.1-6.3", "none", &at(60)).payload_bytes, 48);
        assert_eq!(one("g726-32", "none", &at(5)).payload_bytes, 20);
        assert!(refusal("g729", 1, "ethernet", &at(0)).contains("has to hold some voice"));
        assert!(refusal("g729", 1, "ethernet", &at(250)).contains("more than 240 ms"));
        assert!(refusal("g729", 1, "ethernet", &at(u32::MAX)).contains("more than 240 ms"));
        assert!(voice("g729", 1, "ethernet", &at(MAX_PACKET_MS)).is_ok());

        // Above 30 ms the answer comes with the SRND's warning and names it.
        let slow = one("g729", "ethernet", &at(40));
        assert!(
            slow.notes.iter().any(|note| note.contains("above 30 ms")),
            "{:?}",
            slow.notes
        );
        assert!(slow.sources.iter().any(|source| source == SOURCE_SRND));
        let usual = one("g729", "ethernet", &at(30));
        assert!(!usual.notes.iter().any(|note| note.contains("above 30 ms")));
        assert_eq!(usual.sources, [SOURCE_PER_CALL]);
    }

    #[test]
    fn a_layer_2_size_goes_with_the_custom_link_and_with_nothing_else() {
        let given = one("g711", "custom", &custom(26));
        assert_eq!(given.link.id, "custom");
        assert_eq!(given.link.overhead_bytes, 26);
        assert_eq!(given.link.source, "given with the request");
        assert_eq!(given.packet[0], part("Layer 2, as given", 26));
        // (26 + 40 + 160) * 8 * 50.
        assert_eq!(given.per_call_bps, 90_400);
        assert_eq!(one("g711", "custom", &custom(0)).per_call_bps, 80_000);

        assert_eq!(
            refusal("g711", 1, "custom", &options()),
            "invalid link type `custom`: a custom link needs its layer-2 bytes per packet; give them in `l2_bytes`"
        );
        // A number beside a named link would silently replace a published
        // figure, so it is refused rather than obeyed or ignored.
        assert_eq!(
            refusal("g711", 1, "ethernet", &custom(14)),
            "invalid layer-2 overhead `14 bytes`: `l2_bytes` goes with the `custom` link only; `ethernet` already counts 18 bytes"
        );
        assert!(refusal("g711", 1, "custom", &custom(257)).contains("more than 256 bytes"));
        assert!(voice("g711", 1, "custom", &custom(MAX_L2_BYTES)).is_ok());
    }

    #[test]
    fn the_answer_says_what_it_does_not_count() {
        let call = one("g711", "mpls", &options());
        assert_eq!(
            call.notes,
            [
                "One direction of each call; the other direction costs the same again.",
                "Voice packets only: call signalling and RTCP are not included.",
                "One label and nothing else, as Cisco's table counts it: the frame that carries the labelled packet is not included.",
            ]
        );
        assert_eq!(one("g711", "ppp", &options()).notes.len(), 2);
    }
}
