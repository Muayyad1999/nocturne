//! The pure half of Nocturne's NOC toolbox: calculators, decoders and
//! reference tables.
//!
//! Everything here is a deterministic function of its arguments — no I/O, no
//! async, no clock — cheap enough to run on every keystroke and safe to hand
//! to the AI agent as tools. Nothing panics, whatever the input; failures are
//! a [`ToolError`] whose message is written for the person who typed it.
//! Results are plain data that serialises to JSON for the UI.
//!
//! | Module | What it does |
//! |---|---|
//! | [`subnet`] | Subnet details, splitting, summarising, free-space search (IPv4 and IPv6) |
//! | [`vlsm`] | Variable-length subnet planning |
//! | [`wildcard`] | Netmask / wildcard / prefix conversion and ACL wildcard matching |
//! | [`mac`] | MAC address notations, EUI-64 and vendor lookup in the bundled IEEE registry |
//! | [`secrets`] | Cisco type 7 and Junos `$9$` decoding, secret identification, password generation |
//! | [`convert`] | Number bases, sizes and rates, transfer time, durations, MTU and MSS |
//! | [`qos`] | A DSCP, ToS byte or precedence in every form; bandwidth of voice calls over a link |
//! | [`mod@reference`] | Ports, DSCP, protocol numbers, administrative distances, Ethernet standards, interface names |
//! | [`text`] | Regex tester, digests, Base64 |
//!
//! ```
//! use nocturne_tools::{convert, mac, secrets, subnet};
//!
//! let info = subnet::info("192.168.1.10/24")?;
//! assert_eq!(info.broadcast, "192.168.1.255");
//! assert_eq!(info.scope, "private (RFC 1918)");
//!
//! assert_eq!(subnet::summarize(&["10.0.0.0/24".into(), "10.0.1.0/24".into()])?, ["10.0.0.0/23"]);
//! assert_eq!(mac::info("0000.0c12.3456")?.vendor.as_deref(), Some("Cisco Systems"));
//! assert_eq!(secrets::cisco7_decode("0822455D0A16")?, "cisco");
//! assert_eq!(convert::mtu(1500, &["vxlan".into()])?.payload_mtu, 1450);
//! # Ok::<(), nocturne_tools::ToolError>(())
//! ```
//!
//! # Conventions
//!
//! * Input is read leniently (surrounding whitespace, either letter case,
//!   the notations of every major vendor) and rejected precisely: an error
//!   says which part of the input is wrong and, where possible, what to
//!   write instead.
//! * Counts that can exceed 64 bits (IPv6) are decimal strings.
//! * Anything that could grow without bound has a documented cap
//!   ([`subnet::MAX_SPLIT_SUBNETS`], [`text::MAX_REGEX_MATCHES`],
//!   [`secrets::MAX_PASSWORD_LENGTH`]), and over-long input is refused
//!   before any work is spent on it.
//! * The reference tables and the vendor registry are compiled in; nothing
//!   is read from disk or the network at run time.

#![forbid(unsafe_code)]

mod error;
mod ip;

pub mod convert;
pub mod mac;
pub mod qos;
pub mod reference;
pub mod secrets;
pub mod subnet;
pub mod text;
pub mod vlsm;
pub mod wildcard;

pub use error::ToolError;
