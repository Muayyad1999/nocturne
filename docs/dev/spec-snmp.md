# Spec: `nocturne-snmp` — SNMP v1 / v2c / v3

Nocturne polls devices over SNMP for real-time monitoring (interface counters,
CPU, memory, sensors) and receives traps. This crate is a small, dependency-light,
pure-Rust SNMP implementation built on tokio.

## Public API (contract)

```rust
/// Object identifier. Displays and parses dotted form ("1.3.6.1.2.1.1.1.0", leading dot accepted).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Oid(/* Vec<u32> */);
impl Oid {
    pub fn new(parts: impl Into<Vec<u32>>) -> Oid;
    pub fn parts(&self) -> &[u32];
    pub fn starts_with(&self, prefix: &Oid) -> bool;
    pub fn child(&self, sub: u32) -> Oid;
    /// The part of `self` after `prefix` (table index), if `self` is under it.
    pub fn suffix(&self, prefix: &Oid) -> Option<&[u32]>;
}
impl std::str::FromStr for Oid { /* … */ }
impl std::fmt::Display for Oid { /* … */ }
// Serialize/Deserialize as the dotted string.

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    Integer(i64),
    OctetString(Vec<u8>),
    Null,
    Oid(Oid),
    IpAddress([u8; 4]),
    Counter32(u32),
    Gauge32(u32),
    TimeTicks(u32),
    Opaque(Vec<u8>),
    Counter64(u64),
    NoSuchObject,
    NoSuchInstance,
    EndOfMibView,
}
impl Value {
    pub fn as_u64(&self) -> Option<u64>;      // Integer (≥0), Counter32/64, Gauge32, TimeTicks
    pub fn as_i64(&self) -> Option<i64>;
    pub fn as_text(&self) -> Option<String>;  // OctetString as UTF-8 (lossy) if printable, else None
    /// Human rendering: text if printable, MAC `aa:bb:…` for 6 bytes, hex otherwise; ticks as duration.
    pub fn display(&self) -> String;
    pub fn is_exception(&self) -> bool;       // NoSuch*/EndOfMibView
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VarBind { pub oid: Oid, pub value: Value }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "version", rename_all = "snake_case")]
pub enum Auth {
    V1 { community: String },
    V2c { community: String },
    V3 {
        user: String,
        #[serde(default)] auth: Option<(AuthProtocol, String)>,   // protocol + passphrase
        #[serde(default)] privacy: Option<(PrivProtocol, String)>,
        #[serde(default)] context_name: String,
    },
}
pub enum AuthProtocol { Md5, Sha1, Sha224, Sha256, Sha384, Sha512 }   // serde snake_case
pub enum PrivProtocol { Des, Aes128, Aes192, Aes256 }                 // AES-192/256 = the common Cisco/"Blumenthal" key-extension variant

#[derive(Clone, Debug)]
pub struct ClientConfig {
    pub target: std::net::SocketAddr,
    pub auth: Auth,
    pub timeout: std::time::Duration,   // per attempt, default 2 s
    pub retries: u32,                   // default 1
    pub max_repetitions: u32,           // GETBULK, default 25
}

pub struct Client { /* owns one UDP socket; usable from one task at a time; cheap to create */ }
impl Client {
    pub async fn connect(config: ClientConfig) -> Result<Client, SnmpError>;
    pub async fn get(&mut self, oids: &[Oid]) -> Result<Vec<VarBind>, SnmpError>;
    pub async fn get_next(&mut self, oids: &[Oid]) -> Result<Vec<VarBind>, SnmpError>;
    pub async fn get_bulk(&mut self, oids: &[Oid], non_repeaters: u32, max_repetitions: u32) -> Result<Vec<VarBind>, SnmpError>;
    /// Walk a subtree (GETBULK on v2c/v3, GETNEXT on v1). Stops at the subtree
    /// end, on a non-increasing OID (loop guard), or after `limit` varbinds.
    pub async fn walk(&mut self, root: &Oid, limit: usize) -> Result<Vec<VarBind>, SnmpError>;
    /// Walk several columns of one table and pivot by index:
    /// index suffix → column oid → value.
    pub async fn table(&mut self, columns: &[Oid], limit: usize)
        -> Result<indexmap::IndexMap<Vec<u32>, indexmap::IndexMap<Oid, Value>>, SnmpError>;
    pub async fn set(&mut self, binds: &[VarBind]) -> Result<Vec<VarBind>, SnmpError>;
}

#[derive(thiserror::Error, Debug)]
pub enum SnmpError {
    Timeout, Io(std::io::Error), Decode(String),
    /// error-status from the agent (tooBig, noSuchName, genErr, authorizationError, …) with error-index.
    Agent { status: u32, name: &'static str, index: u32 },
    /// SNMPv3 Report PDUs that are terminal: unknown user, wrong digest, unsupported sec level, decryption error…
    Auth(String),
    Unsupported(String),
}

// ---- traps / informs -------------------------------------------------------
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Trap {
    pub version: u8,                       // 1 or 2 (v2c). v3 traps: decode noAuthNoPriv only, else Unsupported.
    pub community: String,
    pub source: std::net::SocketAddr,
    pub uptime_ticks: Option<u32>,
    pub trap_oid: Option<Oid>,             // snmpTrapOID.0 (v2c) or enterprise + generic/specific mapped to the v2 form (RFC 3584)
    pub enterprise: Option<Oid>,           // v1
    pub agent_addr: Option<[u8; 4]>,       // v1
    pub generic: Option<u32>, pub specific: Option<u32>,   // v1
    pub varbinds: Vec<VarBind>,
    pub is_inform: bool,
}
pub fn decode_trap(datagram: &[u8], source: std::net::SocketAddr) -> Result<Trap, SnmpError>;
/// Build the acknowledgement datagram for an InformRequest.
pub fn inform_response(datagram: &[u8]) -> Result<Vec<u8>, SnmpError>;

// ---- names -----------------------------------------------------------------
/// A built-in dictionary of the MIB objects network engineers meet daily; no MIB compiler.
pub mod names {
    /// "1.3.6.1.2.1.2.2.1.10.3" → Some("ifInOctets.3")
    pub fn describe(oid: &super::Oid) -> Option<String>;
    /// "ifInOctets" / "IF-MIB::ifInOctets" / "sysName.0" → Oid
    pub fn resolve(name: &str) -> Option<super::Oid>;
    /// Well-known trap OIDs → short human name ("linkDown", "coldStart", "bgpBackwardTransition"…)
    pub fn trap_name(oid: &super::Oid) -> Option<&'static str>;
}
/// Constants for the OIDs Nocturne polls (system group, IF-MIB ifTable/ifXTable, HOST-RESOURCES,
/// ENTITY-MIB/ENTITY-SENSOR, BRIDGE/Q-BRIDGE, LLDP-MIB, IP-MIB arp, and the vendor CPU/memory/temperature
/// objects for Cisco, MikroTik, Juniper, Arista, Huawei, HPE/Aruba, Fortinet, Palo Alto, Ubiquiti, net-snmp/UCD).
pub mod oids { /* pub const SYS_DESCR: &str = "1.3.6.1.2.1.1.1.0"; … */ }

// ---- a small agent (responder) ---------------------------------------------
/// Serves v1/v2c GET/GETNEXT/GETBULK from an ordered map. Used by Nocturne to expose simulated lab
/// devices to SNMP, and by this crate's own tests so they run without an external daemon.
pub struct Agent { /* … */ }
impl Agent {
    pub async fn bind(addr: std::net::SocketAddr, community: &str) -> Result<Agent, SnmpError>;
    pub fn local_addr(&self) -> std::net::SocketAddr;
    /// Replace the whole MIB view atomically.
    pub fn set_view(&self, view: std::collections::BTreeMap<Oid, Value>);
    /// Serve until dropped/aborted.
    pub async fn run(self) -> Result<(), SnmpError>;
}
```

## Protocol requirements

* BER: definite lengths (short + long form), correct minimal INTEGER encoding
  incl. negatives, unsigned application types (a 32-bit counter with the top
  bit set needs the leading zero octet), OID sub-identifier base-128 with the
  40×X+Y first octet, Counter64. Decoder must be strict about lengths but
  tolerant of the quirks seen in the field (non-minimal integer encodings,
  zero-length integers). Never panic on malformed input — there is a test that
  throws random and truncated datagrams at every decoder.
* Request-id matching; ignore stray/late datagrams; retries with the same
  request id; v1 has no GETBULK (walk falls back) and no Counter64.
* `tooBig` on GETBULK ⇒ halve `max_repetitions` and retry.
* **SNMPv3 / USM (RFC 3412/3414/3826, RFC 7860 for SHA-2):** engine-ID discovery
  via the Report PDU, time synchronisation (engineBoots/engineTime), automatic
  one-shot resync on `usmStatsNotInTimeWindows`, password-to-key and key
  localisation, HMAC truncation lengths (MD5/SHA-1: 12, SHA-224: 16, SHA-256:
  24, SHA-384: 32, SHA-512: 48), DES-CBC and AES-CFB-128 privacy with correct
  IV/salt construction, key extension for AES-192/256 (and when a short digest
  must feed a longer cipher key). Map the usmStats report OIDs to clear
  `SnmpError::Auth` messages ("unknown user name", "wrong digest
  (authentication passphrase or protocol mismatch)", "decryption error
  (privacy passphrase or protocol mismatch)", "unsupported security level").
  Cache localised keys per engine ID.
* Use the RustCrypto crates already in `Cargo.toml` (`sha1`, `sha2`, `md-5`,
  `hmac`, `aes`, `cfb-mode`, `des`, `cbc`) and `getrandom` for salts/ids.

## Validation you must do

1. Unit tests: BER vectors (hand-checked hex for each type, boundary lengths),
   the RFC 3414 Appendix A key-localisation test vectors (MD5 and SHA-1), a
   known-good captured v2c GET/response pair, trap decoding for a v1 linkDown
   and a v2c linkUp/coldStart, `names` round-trips.
2. Loopback tests against the in-crate `Agent` (get, get_next, bulk walk across
   several hundred OIDs, `table` pivot, tooBig handling, timeout + retry).
3. **Interoperability with net-snmp** (installed in this environment:
   `/usr/sbin/snmpd`, `snmpget`, `snmpwalk`, `snmptrap`). Write
   `tests/netsnmp_interop.rs` that, when `snmpd` is found on `PATH`/`/usr/sbin`,
   launches a private instance on a high UDP port with a temp config
   (`rocommunity`, `createUser` entries for every auth/priv combination,
   `rouser … priv`, and `-C -c <conf>` so the system config is not read) and
   checks: v1 + v2c get/walk of the system and interfaces groups; v3
   noAuthNoPriv, authNoPriv and authPriv for **every** supported auth × priv
   pair net-snmp accepts; wrong passphrase ⇒ `SnmpError::Auth` naming the
   cause; unknown user; a full `walk` of `1.3.6.1.2.1` compared against
   `snmpwalk` output count. Also send traps/informs with the `snmptrap` CLI
   (v1, v2c trap, v2c inform) to a UDP socket in the test and assert
   `decode_trap`/`inform_response`. Skip (do not fail) when net-snmp is
   absent. Report exactly which combinations passed.
4. `cargo clippy -p nocturne-snmp -- -D warnings` clean; no `unsafe`.

## Constraints

* tokio UDP only; no extra network crates. `indexmap` may be added to the
  crate's `Cargo.toml` (`indexmap.workspace = true`).
* Keep the public surface as specified; add helpers freely.
