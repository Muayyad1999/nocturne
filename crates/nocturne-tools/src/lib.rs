//! The pure half of Nocturne's NOC toolbox: calculators, decoders and
//! reference tables.
//!
//! Everything here is a deterministic function of its arguments — no I/O, no
//! async, no clock — cheap enough to run on every keystroke and safe to hand
//! to the AI agent as tools. Nothing panics, whatever the input; failures are
//! a [`ToolError`] whose message is written for the person who typed it.
//!
//! | Module | What it does |
//! |---|---|
//! | [`subnet`] | Subnet details, splitting, summarising, free-space search (IPv4 and IPv6) |
//!
//! ```
//! use nocturne_tools::subnet;
//!
//! let info = subnet::info("192.168.1.10/24")?;
//! assert_eq!(info.broadcast, "192.168.1.255");
//! assert_eq!(info.scope, "private (RFC 1918)");
//! # Ok::<(), nocturne_tools::ToolError>(())
//! ```

#![forbid(unsafe_code)]

mod error;
mod ip;

pub mod mac;
pub mod secrets;
pub mod subnet;
pub mod vlsm;
pub mod wildcard;

pub use error::ToolError;
