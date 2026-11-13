//! The probes: everything in this crate that puts a packet on the wire.
//!
//! All of them share three habits, which are the whole point of running them
//! from Nocturne rather than from a website:
//!
//! * **they say where they ran from** (`from`), because "can the NOC server
//!   reach it?" and "can my laptop reach it?" are different questions;
//! * **they know the inventory** — a probe can be aimed at `SW1`, and an
//!   address that comes back can be labelled as SW1's ([`known`]);
//! * **they are remembered** ([`history`]), with the reason when there is no
//!   result.
//!
//! The ones that can take a while (ping, traceroute, port check, sweep) are
//! jobs and stream their findings as events; the quick ones answer in the
//! reply and leave a record all the same.

use nocturne_app::Router;

pub mod device;
pub mod history;
pub mod known;
pub mod local;
pub mod ping;
pub mod ports;
pub mod quick;
pub mod sweep;
pub mod trace;

pub fn register(router: &mut Router) {
    ping::register(router);
    trace::register(router);
    ports::register(router);
    sweep::register(router);
    quick::register(router);
    local::register(router);
    device::register(router);
    history::register(router);
}
