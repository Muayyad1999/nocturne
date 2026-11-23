//! Issue identification from collected state, configurations, logs and metrics.
//!
//! Specification: `docs/dev/features/insights.md` (conventions: `docs/dev/features/README.md`).
//!
//! Nocturne has already read the network: interface state and counters,
//! neighbours, VLANs, routes, configurations, logs, metrics. This crate is
//! what notices things in it — a duplex mismatch, a port that flaps, a trunk
//! whose two ends disagree, a switch that was never saved, two devices with
//! one address — and says so with the evidence it decided from.
//!
//! It touches no device. Every check is a pure function of a [`world::World`]
//! loaded from what is already stored, which is what makes each one testable
//! case by case, including the cases where it must stay quiet.
//!
//! The thing that decides whether a feature like this is kept or switched off
//! is false positives, so three rules run through the whole crate:
//!
//! 1. **A check that cannot decide says so.** Every check records, per
//!    device, either a verdict or the reason there is none, and
//!    `insights.coverage` reports both — so "nothing found" is never confused
//!    with "not checked".
//! 2. **Every look-alike is written down.** Next to each check is the
//!    reasoning for the cases it deliberately stays quiet about, and a test
//!    that proves it does.
//! 3. **Nothing here changes a device.** A suggested fix is configuration
//!    lines handed to `changes.create` through `insights.fix`, where a human
//!    sees them in their final form.

#![forbid(unsafe_code)]

pub mod catalogue;
pub mod checks;
mod db;
pub mod links;
pub mod logs;
pub mod metrics;
pub mod model;
pub mod net;
pub mod norm;
pub mod report;
pub mod rpc;
pub mod scan;
pub mod settings;
#[cfg(test)]
pub(crate) mod testdata;
pub mod world;

/// Register this feature's settings, state, RPC methods and background tasks.
pub fn init(builder: &mut nocturne_app::AppBuilder) {
    nocturne_app::settings::declare(builder, settings::SECTION, &settings::Settings::default());
    nocturne_app::settings::check_writes(builder, settings::SECTION, settings::check_write);
    // Without its table the feature cannot remember a finding from one scan to
    // the next: failing here means a broken database, not a condition to carry
    // on with.
    db::init(builder.store()).expect("the insights feature's table can be created");
    builder.state(world::ConfigCache::default());
    builder.state(scan::ScanState::default());
    rpc::register(&mut builder.router);
    builder.on_start(scan::spawn_watcher);
}
