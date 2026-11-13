//! Active network probes and small servers: ping, traceroute, scans, DNS, TLS, NTP, TFTP, syslog, NetFlow.
//!
//! Every probe is an `async fn` on tokio that takes its timeouts explicitly and is cancel-safe:
//! dropping the future stops the work and releases its sockets. The servers ([`tftp`],
//! [`syslog`], [`flow`]) run until they are stopped or dropped. Results are plain serde types.

pub mod dns;
pub mod ping;

mod error;
mod process;
mod sys;
mod util;

pub use error::ProbeError;
