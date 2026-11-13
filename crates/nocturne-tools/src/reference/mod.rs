//! Reference tables: the numbers and names a network engineer looks up.
//!
//! Everything here is written out by hand from the relevant RFCs, IEEE
//! standards, IANA registries and vendor documentation. The tables favour
//! being right over being complete: a row is left out rather than guessed.

mod dscp;
mod ethernet;
mod interfaces;
mod numbers;
mod ports;
mod routing;
mod syslog;

pub use dscp::{DscpEntry, dscp, dscp_table};
pub use ethernet::{CableEntry, ethernet_standards};
pub use interfaces::{interface_long_name, interface_short_name, interface_sort_key};
pub use numbers::{NamedNumber, ethertypes, icmp_types, icmpv6_types, ip_protocols};
pub use ports::{PortEntry, ports};
pub use routing::{AdEntry, administrative_distances};
pub use syslog::{SyslogLevel, syslog_levels};
