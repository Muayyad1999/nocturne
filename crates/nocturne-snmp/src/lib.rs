//! Pure-Rust SNMP v1/v2c/v3 client, trap decoder and minimal agent.
//!
//! * [`Client`] — GET, GETNEXT, GETBULK, SET, subtree [walks](Client::walk) and pivoted
//!   [tables](Client::table) over SNMPv1, SNMPv2c and SNMPv3 (USM with MD5/SHA-1/SHA-2
//!   authentication and DES/AES-128/192/256 privacy), on a tokio UDP socket.
//! * [`decode_trap`] / [`inform_response`] — SNMPv1 traps, SNMPv2c traps and informs.
//! * [`names`] and [`oids`] — a built-in dictionary of the objects network engineers meet
//!   daily, so that results can be shown and requested by name without a MIB compiler.
//! * [`Agent`] — a small v1/v2c responder serving a map of values.
//!
//! # Polling interface counters
//!
//! ```no_run
//! use nocturne_snmp::{Auth, Client, ClientConfig, Oid, names, oids};
//!
//! # async fn poll() -> Result<(), nocturne_snmp::SnmpError> {
//! let config = ClientConfig::new(
//!     "192.0.2.1:161".parse().unwrap(),
//!     Auth::V2c { community: "public".into() },
//! );
//! let mut client = Client::connect(config).await?;
//!
//! // One request returns whole rows: ifName, ifHCInOctets and ifHCOutOctets of each interface.
//! let columns: Vec<Oid> = [oids::IF_NAME, oids::IF_HC_IN_OCTETS, oids::IF_HC_OUT_OCTETS]
//!     .iter()
//!     .map(|column| column.parse())
//!     .collect::<Result<_, _>>()?;
//! for (index, row) in client.table(&columns, 10_000).await? {
//!     let name = row.get(&columns[0]).and_then(|v| v.as_text()).unwrap_or_default();
//!     let octets_in = row.get(&columns[1]).and_then(|v| v.as_u64());
//!     println!("ifIndex {index:?} {name}: {octets_in:?} octets in");
//! }
//!
//! // Scalars by name.
//! let uptime = client.get(&[names::resolve("sysUpTime.0").unwrap()]).await?;
//! println!("up for {}", uptime[0].value.display());
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

mod agent;
mod ber;
mod client;
mod error;
pub mod names;
mod oid;
pub mod oids;
mod pdu;
#[cfg(test)]
mod robustness;
mod trap;
mod v3;
mod value;

pub use agent::Agent;
pub use client::{Auth, AuthParts, AuthProblem, Client, ClientConfig};
pub use error::{SnmpError, error_status_name};
pub use oid::Oid;
pub use trap::{Trap, decode_trap, inform_response};
pub use v3::usm::{AuthProtocol, PrivProtocol};
pub use value::{Value, VarBind};
