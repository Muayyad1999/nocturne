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

## Validation

* Unit tests with hand-verified values for every function, including the classic edge cases: /0, /31, /32,
  /127, /128, network and broadcast addresses as input, non-contiguous masks, IPv4-mapped IPv6, zone ids
  rejected, `summarize` of adjacent/nested/unsorted/duplicate inputs, VLSM plans that exactly fill and that
  overflow the base, MACs in every notation, type-7 strings from real configs (e.g. `0822455D0A16` →
  `cisco`), `$9$` examples, DSCP/ToS round trips.
* Cross-check `subnet` and `vlsm` against Python's `ipaddress` module on a few thousand random inputs
  (commit the generator script under `tests/`; the Rust test reads a vendored JSON of expected values so
  CI does not need Python).
* A robustness test: random strings (ASCII junk, Unicode, very long input) through every public function —
  no panics, no unbounded allocation (`split` and friends must honour their limits; sizes are computed with
  `u128`/checked arithmetic).
* `cargo clippy -p nocturne-tools --all-targets -- -D warnings` clean; no `unsafe`.
