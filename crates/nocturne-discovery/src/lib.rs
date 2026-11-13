//! Finding out what is on a network, and proposing what to manage.
//!
//! Specification: `docs/dev/features/discovery.md` (conventions:
//! `docs/dev/features/README.md`).
//!
//! Two ways in. **Sweeping** ([`scan`]) probes a range of addresses and
//! identifies what answers. **Crawling** ([`crawl`]) asks the devices
//! Nocturne already manages what *they* know — neighbour tables, ARP caches,
//! routing tables — which maps a real network far better than a sweep and
//! disturbs nobody. Either way the result is a list of [`model::Found`]
//! records, each carrying the evidence for every field it claims, and
//! nothing is added to the inventory until a person asks for it
//! (`discovery.preview`, then `discovery.add`).
//!
//! # Rules of engagement
//!
//! A sweep sends packets to machines nobody asked Nocturne to talk to. On
//! someone else's network that is rude; on a production network it can be
//! worse. So:
//!
//! 1. **Nothing scans unless a person asks.** There is no background sweep,
//!    no sweep at startup, and no "while we are here" probing from any other
//!    code path in this crate.
//! 2. **The settings are the leash, not a suggestion.** `max_hosts`, `rate`,
//!    `concurrency`, `timeout_ms` and the `never_scan` list bound every scan
//!    ([`settings`]). A request may ask for *less* than the settings allow
//!    and never for more, and a block in `never_scan` is dropped from the
//!    plan whatever the request says.
//! 3. **Targets are vetted before a packet is sent** ([`targets`]):
//!    multicast, broadcast and unspecified addresses are refused outright;
//!    anything that is not private, link-local, unique-local or loopback is
//!    refused unless the request asks for it *and* the installation allows
//!    it — and then the job log says so in as many words.
//! 4. **A scan is a job**, so it is visible, has a log that says what it did,
//!    and stops when the user cancels it — checked between addresses and
//!    inside every probe.
//! 5. **Discovery never tries credentials against a device it merely found.**
//!    There is no login anywhere in this crate. SNMP is attempted only with
//!    the credentials the request names, because a scan without an SNMP
//!    community can identify almost nothing; the SSH banner is read from
//!    what the server volunteers before any authentication; and the only
//!    devices whose command line is used are the ones already in the
//!    inventory, through the core's read-only gate (`collect::run`), during
//!    a crawl the user asked for.
//! 6. **A find is a proposal.** `discovery.add` is a separate, explicit act,
//!    it creates devices through `inventory::create` like any other import,
//!    and it still logs in nowhere.
//!
//! # What identification refuses to guess
//!
//! A device type is data: it comes from the profile registry's `detect:`
//! rules (`detect::from_snmp`, then `detect::from_banner`), never from a
//! vendor string written into this crate. When no profile matches, the find
//! has no profile — open ports, an OUI vendor or a DNS name are never
//! promoted into a platform, a role or a name the device did not give
//! itself. See [`identify`] for the list and the reasons.

pub mod add;
pub mod crawl;
pub mod db;
pub mod identify;
pub mod model;
pub mod probe;
pub mod scan;
pub mod settings;
pub mod snmp;
pub mod suggest;
pub mod targets;

#[cfg(test)]
mod tests;

/// Register this feature's settings, state, RPC methods and background tasks.
///
/// There is no background task: discovery only ever runs because somebody
/// asked it to.
pub fn init(builder: &mut nocturne_app::AppBuilder) {
    nocturne_app::settings::declare(
        builder,
        settings::SECTION,
        &settings::DiscoverySettings::default(),
    );
    builder.state(db::DiscoveryState::default());
    // A scan that was running when the process ended did not finish; without
    // this it would sit in the list as `running` for ever.
    db::close_interrupted(builder.store());

    suggest::register(&mut builder.router);
    scan::register(&mut builder.router);
    crawl::register(&mut builder.router);
    add::register(&mut builder.router);
}
