# Spec: `nocturne-tools` — calculators, decoders and reference tables

The pure half of Nocturne's NOC toolbox: everything a network engineer keeps a
browser tab or a scrap of paper for. **No I/O, no async, no clock** — every
function is a deterministic function of its arguments, cheap enough to call on
every keystroke from the UI and usable by the AI agent as tools.

All result types derive `Serialize, Deserialize, Clone, Debug, PartialEq` (they
are sent to the UI as JSON, `snake_case` enums). Errors are one
`thiserror` enum, `ToolError`, whose messages are written for end users
("`300` is not a valid octet in `10.0.300.1`"). Nothing may panic on any
input: every public function gets a robustness test fed with junk.

## Public API (contract)

```rust
// ------------------------------------------------------------------ subnet
pub mod subnet {
    /// Accepts "192.168.1.10/24", "192.168.1.10 255.255.255.0", "192.168.1.10/255.255.255.0",
    /// "192.168.1.10 0.0.0.255" (wildcard), a bare address (/32 or /128), and IPv6 "2001:db8::1/64".
    pub fn info(input: &str) -> Result<SubnetInfo, ToolError>;
    pub struct SubnetInfo {
        pub version: u8,                 // 4 | 6
        pub address: String,             // the host address given
        pub network: String,             // network address
        pub prefix: u8,
        pub cidr: String,                // "192.168.1.0/24"
        pub netmask: String,             // v4 only, "" for v6
        pub wildcard: String,            // v4 only
        pub broadcast: String,           // v4 only ("" for /31 and /32 and v6)
        pub first_host: String,
        pub last_host: String,
        pub usable_hosts: String,        // decimal string (can exceed u64 for v6); /31 → 2, /32 → 1
        pub total_addresses: String,
        pub class: String,               // v4 classful letter A–E, "" for v6
        pub scope: String,               // "private (RFC 1918)", "public", "loopback", "link-local", "CGNAT (RFC 6598)",
                                         // "documentation", "multicast", "reserved", v6: "global unicast", "unique local", …
        pub binary_address: String,      // v4: dotted binary octets
        pub binary_netmask: String,
        pub hex: String,                 // v4 "0xC0A8010A"; v6 full hex
        pub integer: String,             // decimal
        pub reverse_dns: String,         // "10.1.168.192.in-addr.arpa" / ip6.arpa nibble form
        pub expanded: String,            // v6 fully expanded; v4 same as address
        pub compressed: String,          // v6 RFC 5952 form
        pub is_network_address: bool, pub is_broadcast_address: bool,
    }
    /// Split a network into subnets of `new_prefix`; at most `limit` are returned (plus the true total).
    pub fn split(network: &str, new_prefix: u8, limit: usize) -> Result<SplitResult, ToolError>;
    pub struct SplitResult { pub total: String, pub subnets: Vec<SubnetInfo>, pub truncated: bool }
    /// Smallest set of CIDR blocks covering exactly the given networks/addresses (merges adjacent and nested).
    pub fn summarize(networks: &[String]) -> Result<Vec<String>, ToolError>;
    /// The single smallest supernet containing all of them.
    pub fn supernet(networks: &[String]) -> Result<String, ToolError>;
    pub fn contains(network: &str, address: &str) -> Result<bool, ToolError>;
    pub fn overlaps(a: &str, b: &str) -> Result<bool, ToolError>;
    /// "10.0.0.5" – "10.0.1.20" → minimal CIDR list.
    pub fn range_to_cidrs(first: &str, last: &str) -> Result<Vec<String>, ToolError>;
    /// Next free subnet of `prefix` inside `parent` that overlaps none of `used`.
    pub fn next_free(parent: &str, prefix: u8, used: &[String]) -> Result<Option<String>, ToolError>;
    /// Unused address ranges inside `parent` given the `used` networks, as CIDR blocks.
    pub fn free_space(parent: &str, used: &[String]) -> Result<Vec<String>, ToolError>;
}

// -------------------------------------------------------------------- vlsm
pub mod vlsm {
    pub struct Request { pub name: String, pub hosts: u64 }
    pub struct Allocation {
        pub name: String, pub hosts_requested: u64, pub cidr: String, pub netmask: String,
        pub first_host: String, pub last_host: String, pub broadcast: String,
        pub usable_hosts: u64, pub spare: u64,
    }
    pub struct Plan { pub allocations: Vec<Allocation>, pub unallocated: Vec<String>, pub used_addresses: u64, pub total_addresses: u64 }
    /// Largest-first allocation with correct alignment. Fails with a clear message naming the request that did not fit.
    /// A request for N hosts gets the smallest subnet with ≥ N usable addresses (point-to-point 2 → /30; `p2p31: true` → /31).
    pub fn plan(base: &str, requests: &[Request], p2p31: bool) -> Result<Plan, ToolError>;
}

// ---------------------------------------------------------------- wildcard
pub mod wildcard {
    pub struct MaskInfo { pub prefix: Option<u8>, pub netmask: String, pub wildcard: String, pub contiguous: bool, pub addresses: String }
    /// Accepts a prefix length ("24", "/24"), a netmask or a wildcard mask.
    pub fn convert(input: &str) -> Result<MaskInfo, ToolError>;
    /// Does `candidate` match the ACL pair `address wildcard`? Works for non-contiguous wildcards.
    pub fn matches(address: &str, wildcard: &str, candidate: &str) -> Result<bool, ToolError>;
    /// Plain-English description of what an ACL address/wildcard pair matches, with first/last address and count.
    pub fn explain(address: &str, wildcard: &str) -> Result<String, ToolError>;
}

// --------------------------------------------------------------------- mac
pub mod mac {
    pub struct MacInfo {
        pub colon: String,      // aa:bb:cc:dd:ee:ff
        pub hyphen: String,     // AA-BB-CC-DD-EE-FF
        pub dotted: String,     // aabb.ccdd.eeff
        pub bare: String,       // aabbccddeeff
        pub oui: String,        // AA:BB:CC
        pub vendor: Option<String>,
        pub is_multicast: bool, pub is_broadcast: bool,
        pub is_locally_administered: bool,
        pub eui64: String,                 // modified EUI-64 interface id "a8bb:ccff:fedd:eeff"
        pub link_local_ipv6: String,       // fe80::…
    }
    /// Any common notation, any case, with or without separators.
    pub fn info(input: &str) -> Result<MacInfo, ToolError>;
    /// Vendor for a MAC or OUI prefix from the bundled IEEE registry (MA-L; MA-M/MA-S if cheap).
    pub fn vendor(input: &str) -> Option<String>;
    /// How many assignments the bundled registry holds, and its date.
    pub fn registry_info() -> (usize, &'static str);
}

// ----------------------------------------------------------------- secrets
pub mod secrets {
    /// Cisco "type 7" is reversible obfuscation.
    pub fn cisco7_decode(encoded: &str) -> Result<String, ToolError>;
    pub fn cisco7_encode(plain: &str, salt: u8) -> Result<String, ToolError>;       // salt 0–15
    /// Junos "$9$" is reversible obfuscation.
    pub fn junos9_decode(encoded: &str) -> Result<String, ToolError>;
    pub struct SecretKind { pub kind: String, pub name: String, pub reversible: bool, pub strength: String, pub advice: String }
    /// Identify a secret from a config line or the bare value: Cisco types 0/4/5/6/7/8/9, md5crypt, sha256crypt,
    /// sha512crypt, bcrypt, Junos $9$/$1$/$5$/$6$, Huawei cipher blobs, FortiOS ENC, RouterOS plain, SNMP communities.
    pub fn identify(input: &str) -> SecretKind;
    pub struct PasswordPolicy { pub length: usize, pub upper: bool, pub lower: bool, pub digits: bool, pub symbols: bool, pub avoid_ambiguous: bool }
    /// Deterministic for a given seed (the caller supplies entropy), using a CSPRNG-quality expansion
    /// (e.g. SHA-256 in counter mode over the seed); rejection sampling, at least one of each enabled class.
    pub fn generate_password(policy: &PasswordPolicy, seed: &[u8; 32]) -> Result<String, ToolError>;
}

// ----------------------------------------------------------------- convert
pub mod convert {
    pub struct NumberForms { pub decimal: String, pub hex: String, pub binary: String, pub octal: String, pub ipv4: Option<String> }
    /// "0xC0A80101", "3232235777", "0b1010", "192.168.1.1" → every form.
    pub fn number(input: &str) -> Result<NumberForms, ToolError>;
    pub struct TransferEstimate { pub seconds: f64, pub human: String, pub effective_bps: f64 }
    /// File size ("700 MB", "4.7GiB", bytes) over a link ("100 Mbps", "1.5G", bps) with protocol overhead %.
    pub fn transfer_time(size: &str, rate: &str, overhead_pct: f64) -> Result<TransferEstimate, ToolError>;
    pub fn parse_size_bytes(input: &str) -> Result<u64, ToolError>;      // KB/MB/GB (10^3) and KiB/MiB/GiB (2^10)
    pub fn parse_rate_bps(input: &str) -> Result<u64, ToolError>;        // "1G", "10 Gbps", "100Mb/s", "9600"
    pub fn format_rate(bps: f64) -> String;                              // "1.25 Gbps"
    pub fn format_bytes(bytes: f64) -> String;                           // "4.38 GiB"
    /// "2w3d", "1 day, 4 hours, 5 minutes", "3d04h", "12:34:56", timeticks → seconds.
    pub fn parse_duration_s(input: &str) -> Option<u64>;
    pub fn format_duration(seconds: u64) -> String;                      // "2w 3d 4h 5m"
    pub struct MtuResult { pub payload_mtu: u32, pub tcp_mss_v4: u32, pub tcp_mss_v6: u32, pub overhead: u32, pub breakdown: Vec<(String, u32)> }
    /// Stack encapsulations on a link MTU: "dot1q", "qinq", "mpls" (per label), "gre", "gre-key", "ipsec-esp-tunnel"
    /// (AES-GCM / AES-CBC-SHA estimates), "vxlan", "geneve", "pppoe", "l2tp", "wireguard", "ipip", "6in4".
    pub fn mtu(link_mtu: u32, encapsulations: &[String]) -> Result<MtuResult, ToolError>;
}

// --------------------------------------------------------------------- qos
pub mod qos {
    /// What the value handed to `marking` is. `FromStr`: "dscp" | "tos" | "precedence" | "name".
    pub enum MarkingForm { Dscp, Tos, Precedence, Name }
    pub struct Marking {
        pub dscp: u8,                    // 0–63
        pub name: Option<String>,        // "EF", "AF41", "CS3", "DF"; None for a value the registry has not named
        pub dscp_binary: String,         // "101110"
        pub dscp_hex: String,            // "0x2E"
        pub tos: u8,                     // the whole ToS / Traffic Class byte; its ECN bits are the ones given
                                         // with a ToS byte, and clear for every other form
        pub tos_hex: String,             // "0xB8"
        pub tos_binary: String,          // "10111000"
        pub precedence: u8,              // 0–7, the top three bits
        pub precedence_name: String,     // RFC 791: "Routine" … "CRITIC/ECP" … "Network Control"
        pub ecn: u8,                     // 0–3, the bottom two bits
        pub ecn_binary: String,          // "10"
        pub ecn_name: String,            // RFC 3168: "Not-ECT" | "ECT(1)" | "ECT(0)" | "CE"
        pub ecn_meaning: String,
        pub phb: String,                 // per-hop behaviour and its RFC; for an unnamed value, its registry pool
        pub typical_use: Option<String>, // None where there is no standard use
        pub notes: Vec<String>,          // what the answer assumes that the input did not say
    }
    /// One marking in every form. The caller says what the value is, because the same number is a different
    /// marking as a DSCP, a ToS byte and a precedence; a number outside the form's range is refused, never
    /// re-read as another form. `Dscp` takes 0–63 (decimal, 0x.., 0b..) or a name as a device takes it after
    /// `dscp`; `Name` takes a code point name only; `Tos` takes 0–255; `Precedence` takes 0–7 or
    /// routine … network, and answers with the class selector plus a note that the low bits were not given.
    /// Names, behaviours and uses are `reference::dscp_table`'s: one table, two readers.
    pub fn marking(value: &str, form: MarkingForm) -> Result<Marking, ToolError>;

    pub struct VoiceCodec {
        pub id: String,                  // "g711" | "g729" | "g723.1-6.3" | "g723.1-5.3" | "g726-32" | "g726-24"
                                         // | "g728" | "g722" | "ilbc-20" | "ilbc-30"
        pub name: String, pub bit_rate_bps: u32,
        pub sample_bytes: u32, pub sample_ms: u32,   // what the codec produces, and how often
        pub packet_ms: u32, pub payload_bytes: u32,  // the default packet: voice per packet and its bytes
        pub mos: Option<f64>,            // None where the source gives none
    }
    pub struct VoiceLink {
        pub id: String,                  // "ethernet" | "dot1q" | "ppp" | "mlppp" | "frame-relay" | "frf12"
                                         // | "mpls" | "wlan" | "none"   ("custom" is accepted by `voice`, not listed)
        pub name: String,
        pub overhead_bytes: u32,         // layer-2 bytes per packet
        pub parts: String,               // "Ethernet header 14 + Ethernet FCS 4"
        pub crtp: bool,                  // whether compressed RTP is counted on this kind of link
        pub source: String,              // which document the figure is from
        pub note: String,                // "" when there is nothing to add
    }
    pub fn voice_codecs() -> Vec<VoiceCodec>;
    pub fn voice_links() -> Vec<VoiceLink>;

    pub struct VoiceOptions {            // Default: none of them; every field is optional in JSON
        pub crtp: bool,                  // 2 bytes of IP/UDP/RTP header instead of 40
        pub vad: bool,                   // also return the voice-activity-detection estimate
        pub packet_ms: Option<u32>,      // voice per packet; a multiple of the codec's sample_ms, ≤ MAX_PACKET_MS
        pub l2_bytes: Option<u32>,       // layer-2 bytes for the `custom` link, and only for it; ≤ MAX_L2_BYTES
    }
    pub struct PacketPart { pub name: String, pub bytes: u32 }
    pub struct VadEstimate {
        pub saving_pct: u32,             // 35: the most the source says to expect
        pub per_call_bps: u64, pub total_bps: u64, pub per_call: String, pub total: String,
                                         // 65 % of the result's per_call_bps and total_bps as shown, to the nearest
                                         // bit — not of the exact fraction, so the arithmetic line multiplies out
        pub enough_calls: bool,          // more than 24 calls, below which the source says not to count on it
        pub caveat: String,
    }
    pub struct VoiceBandwidth {
        pub codec: VoiceCodec, pub link: VoiceLink, pub calls: u32, pub crtp: bool,
        pub packet_ms: u32, pub packets_per_second: f64,     // 50.0, 33.333 (three decimals)
        pub payload_bytes: u32,
        pub packet: Vec<PacketPart>,     // layer 2, then IP/UDP/RTP (or the cRTP header), then the voice payload
        pub packet_bytes: u32,           // their sum
        pub per_call_bps: u64,           // one call, one direction, to the nearest bit per second
        pub total_bps: u64,              // from the exact per-call figure, then rounded — not calls × the rounded one
        pub per_call: String, pub total: String,             // "26.8 kbps"
        pub vad: Option<VadEstimate>,    // only when asked for, and never instead of the full figure
        pub arithmetic: Vec<String>,     // the calculation, one line per step
        pub notes: Vec<String>,          // what the figure covers and leaves out
        pub sources: Vec<String>,        // the documents the figures were taken from
    }
    /// packet = layer 2 + IP/UDP/RTP (40, or 2 with cRTP) + payload; rate = 1000 / packet_ms; bps = packet × 8 × rate.
    /// Refuses: an unknown codec or link (with the list), a name that covers two bit rates (`g723`, `g726`,
    /// `ilbc`), 0 calls or more than MAX_CALLS, cRTP on a link the sources do not have it on (Ethernet, 802.1Q,
    /// WLAN, MPLS — only Ethernet is ruled out in so many words, and the refusal says "not counted", not "impossible"),
    /// a packet that does not hold whole samples, `custom` without `l2_bytes`, `l2_bytes` with any other link.
    pub fn voice(codec: &str, calls: u32, link: &str, options: &VoiceOptions) -> Result<VoiceBandwidth, ToolError>;
    pub const MAX_CALLS: u32 = 1_000_000;  pub const MAX_PACKET_MS: u32 = 240;  pub const MAX_L2_BYTES: u32 = 256;
    pub const VAD_SAVING_PCT: u32 = 35;
    pub const SOURCE_PER_CALL: &str;  pub const SOURCE_SRND: &str;        // the two citations, below
}

// --------------------------------------------------------------- reference
pub mod reference {
    pub struct PortEntry { pub port: u16, pub protocol: String, pub name: String, pub description: String }
    /// ~250 ports a network engineer actually meets (not all of IANA).
    pub fn ports(query: &str) -> Vec<PortEntry>;          // by number, name or text
    pub struct DscpEntry { pub name: String, pub dscp: u8, pub binary: String, pub tos: u8, pub precedence: u8, pub phb: String, pub typical_use: String }
    pub fn dscp_table() -> Vec<DscpEntry>;
    pub fn dscp(query: &str) -> Option<DscpEntry>;        // "ef", "46", "af41", "0xb8" (ToS)
    pub struct NamedNumber { pub number: u32, pub name: String, pub description: String }
    pub fn ip_protocols() -> Vec<NamedNumber>;
    pub fn ethertypes() -> Vec<NamedNumber>;
    pub fn icmp_types() -> Vec<NamedNumber>;
    pub struct AdEntry { pub source: String, pub cisco: Option<u32>, pub juniper: Option<u32>, pub mikrotik: Option<u32>, pub huawei: Option<u32>, pub arista: Option<u32> }
    /// Administrative distance / route preference per vendor.
    pub fn administrative_distances() -> Vec<AdEntry>;
    pub struct CableEntry { pub standard: String, pub medium: String, pub speed: String, pub max_distance: String, pub notes: String }
    pub fn ethernet_standards() -> Vec<CableEntry>;       // 10BASE-T … 400GBASE-*, incl. common optics
    pub struct SyslogLevel { pub level: u8, pub name: String, pub cisco_keyword: String, pub meaning: String }
    pub fn syslog_levels() -> Vec<SyslogLevel>;
    /// Interface-name helpers shared with the rest of the app.
    pub fn interface_long_name(short: &str) -> String;    // "Gi1/0/1" → "GigabitEthernet1/0/1", "Te1/1/1", "Po1", "Vl10", "Lo0", "Fa0/1", "Eth1/1", "Hu1/0/1"…
    pub fn interface_short_name(long: &str) -> String;
    /// Natural sort key so Gi1/0/2 sorts before Gi1/0/10.
    pub fn interface_sort_key(name: &str) -> Vec<(String, u64)>;
}

// -------------------------------------------------------------------- text
pub mod text {
    pub struct RegexMatch { pub start: usize, pub end: usize, pub text: String, pub groups: Vec<Option<String>>, pub named: Vec<(String, Option<String>)> }
    pub struct RegexResult { pub matches: Vec<RegexMatch>, pub error: Option<String> }
    pub fn regex_test(pattern: &str, haystack: &str, multiline: bool, case_insensitive: bool) -> RegexResult;
    pub struct Hashes { pub md5: String, pub sha1: String, pub sha256: String, pub sha512: String }
    pub fn hashes(data: &[u8]) -> Hashes;
    pub fn base64_encode(data: &[u8]) -> String;
    pub fn base64_decode(text: &str) -> Result<Vec<u8>, ToolError>;      // tolerant of whitespace and missing padding
}

#[derive(thiserror::Error, Debug, Clone, PartialEq)]
pub enum ToolError { /* Invalid { what, input, reason }, TooLarge(String), DoesNotFit(String) … */ }
```

## Data

* **OUI registry.** Download the IEEE MA-L registry (`https://standards-oui.ieee.org/oui/oui.csv`; MA-M
  `oui28/mam.csv` and MA-S `oui36/oui36.csv` too if the size stays reasonable) at authoring time and vendor a
  compact, sorted, deduplicated `data/oui.tsv.gz` (`PREFIXHEX<TAB>Organisation`, organisation names trimmed of
  trailing legal noise only if unambiguous) inside the crate, embedded with `include_bytes!` and inflated once
  on first use (`flate2`, already a dependency). Record the download date. If the download is impossible, say
  so in the report and embed a hand-written table of the ~300 vendors that matter in networking instead.
* Reference tables are written by hand from the relevant RFCs / IEEE standards; be accurate — a wrong
  administrative distance or DSCP value in a reference tool is worse than a missing row.
* **Voice codecs and link overheads** (`qos`) are not written from memory: every row is copied from one of two
  published Cisco documents, fetched from www.cisco.com on 2026-11-22, and the tests reproduce the tables those
  documents print.
  * *Modify Bandwidth Consumption Calculation for Voice Calls*, Document ID 7934, revision 3.0 of 11 December 2023
    (`https://www.cisco.com/c/en/us/support/docs/voice/voice-quality/7934-bwidth-consume.html`; that is the title
    the page carries at this revision, and its table is headed "VoIP - Per Call Bandwidth"): the ten codecs
    (sample size and interval, default payload, MOS), IP/UDP/RTP at 40 bytes and cRTP at 2 (4 with UDP
    checksums), Ethernet at 18 bytes including the FCS, Multilink PPP and FRF.12 at 6 bytes plus a 1-byte
    end-of-frame flag, "cRTP is not available over Ethernet", and voice-activity detection at "up to 35 percent"
    as an average over more than 24 calls that "must not be taken into account" when provisioning.
  * *Cisco Collaboration System 12.x SRND*, chapter 3 "Network Infrastructure", revised 1 March 2018, tables 3-7
    and 3-8 (`https://www.cisco.com/c/en/us/td/docs/voice_ip_comm/cucm/srnd/collab12/collab12/netstruc.pdf`):
    PPP at 6 bytes, Frame Relay at 4, an MPLS label at 4, an 802.11 header at 24.
  * The 802.1Q row is the first document's Ethernet figure plus the 4-byte tag of IEEE 802.1Q; neither document
    prints it, and its `source` says so.
  * Compressed RTP is counted on PPP, Multilink PPP, Frame Relay, FRF.12, `none` and `custom`. The first document
    has it for "MP or FRF.12" and says it is "not available over Ethernet"; the SRND (table 3-5) lists it for
    leased lines and Frame Relay and says it "operates on a per-hop basis". Neither says anything about cRTP on a
    wireless LAN or under an MPLS label, so those two are refused as *not counted*, with `custom` as the way out —
    the refusal does not claim it cannot be done.
  * Where the two documents disagree the named link follows the first and the row's `note` gives the other
    figure: Ethernet is 18 bytes with the FCS (the SRND counts the 14-byte header alone), Multilink PPP is 6 + 1
    (the SRND counts 10). The `custom` link takes any byte count, which is also how the SRND's two columns and
    the first document's sample calculation (a 6-byte MP header without the flag: 11.2 kbps) are reproduced.
  * Left out on purpose: ATM (the SRND's ATM column says only "53-byte cells with a 48-byte payload"; its
    figures fit 3 or 4 bytes of overhead before padding to whole cells, which the document does not state and
    which is not AAL5's 8-byte trailer — a number fitted to a table is not a number taken from one), SRTP, IPv6,
    and every codec neither document tabulates (Opus, G.722.1, AMR): a row is left out rather than guessed.

## Validation

* Unit tests with hand-verified values for every function, including the classic edge cases: /0, /31, /32,
  /127, /128, network and broadcast addresses as input, non-contiguous masks, IPv4-mapped IPv6, zone ids
  rejected, `summarize` of adjacent/nested/unsorted/duplicate inputs, VLSM plans that exactly fill and that
  overflow the base, MACs in every notation, type-7 strings from real configs (e.g. `0822455D0A16` →
  `cisco`), `$9$` examples, DSCP/ToS round trips.
* Cross-check `subnet` and `vlsm` against Python's `ipaddress` module on a few thousand random inputs
  (commit the generator script under `tests/`; the Rust test reads a vendored JSON of expected values so
  CI does not need Python).
* `qos::voice` against the published tables, with the document named in each test: all thirty bandwidth cells
  of document 7934's per-call table to the printed digit, its sample calculation (28 bytes, 224 bits, 50 pps,
  11.2 kbps) and its payload-size example (24 kbps and 16 kbps); the SRND's layer-3 table and its layer-2 table
  for every column but ATM. Four cells of the SRND's 33.3-packet rows are not its own formula rounded to one
  decimal (it prints 19.7 and 19.8 for the same four-byte header); the test names them and holds them to
  0.07 kbps, a quarter of what one byte would change. Six everyday cases (G.711 and G.729 over Ethernet and
  over PPP, and over PPP with cRTP) are worked out by hand in a test's comment and asserted to the bit.
  `qos::marking`: every byte 0–255 taken apart and put back, every table name, the refusals, and every row of the
  IANA registry "Differentiated Services Field Codepoints" (pools 1 and 3, fetched from www.iana.org on
  2026-11-22) read in every direction — name, DSCP in decimal, hexadecimal and binary, ToS byte with each of the
  four ECN values, precedence — against hexadecimal, ToS and precedence figures worked out by hand.
* A robustness test: random strings (ASCII junk, Unicode, very long input) through every public function —
  no panics, no unbounded allocation (`split` and friends must honour their limits; sizes are computed with
  `u128`/checked arithmetic).
* `cargo clippy -p nocturne-tools --all-targets -- -D warnings` clean; no `unsafe`.
