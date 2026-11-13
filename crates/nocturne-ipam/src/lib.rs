//! Prefixes, addresses and VLANs, checked against what devices report.
//!
//! Specification: `docs/dev/features/ipam.md` (conventions:
//! `docs/dev/features/README.md`).
//!
//! The point of this feature is not that it stores prefixes — a spreadsheet
//! does that. It is that Nocturne already knows what is really configured, so
//! the record can be *checked* instead of trusted. Everything here therefore
//! keeps two things apart, everywhere, without exception:
//!
//! | | written by | lives in | changed by |
//! |---|---|---|---|
//! | **record** | a person | [`model::Prefix`], [`model::AddressRecord`], [`model::VlanRecord`] | `ipam.*.save`, `ipam.import`, `ipam.learn` |
//! | **observation** | a device | [`model::Observation`] | `ipam.sync`, which only ever rewrites evidence |
//!
//! An address nobody has seen is not "free", it is "not seen". A prefix
//! nobody documented is not "unused", it is "not documented" — and it says so
//! in its own `source`. Where the two disagree, `ipam.reconcile` reports it
//! and refuses to decide which side is wrong: that decision is a change to a
//! device or to a plan, and both belong to a person.
//!
//! No device is touched from here. The evidence is the core's collected
//! interface tables and ARP tables; the subnet arithmetic is
//! [`nocturne_tools::subnet`] and [`nocturne_tools::vlsm`].

pub mod addresses;
pub mod api;
pub mod collected;
pub mod conflicts;
pub mod db;
pub mod model;
pub mod net;
pub mod portability;
pub mod prefixes;
pub mod sync;
pub mod vlans;

/// Register this feature's settings, tables, RPC methods and watcher.
pub fn init(builder: &mut nocturne_app::AppBuilder) {
    api::init(builder);
}
