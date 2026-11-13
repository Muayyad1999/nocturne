//! Virtual labs on the network simulator: runtime, consoles, inventory sync, packet tracing.
//!
//! Specification: `docs/dev/features/lab.md` (conventions: `docs/dev/features/README.md`).
//!
//! A lab is a [`nocturne_netsim::LabSpec`] stored in the `labs` collection.
//! *Running* means there is a [`nocturne_netsim::Lab`] in memory and a ticker
//! task advancing it in real time. Every node that has a command line is also
//! an inventory device (`transport: sim`, a `lab` reference, site `Lab: <name>`),
//! so the terminal, the collectors, monitoring and the change pipeline work on
//! simulated devices with no special cases.

#![forbid(unsafe_code)]

mod api;
mod console;
mod design;
mod devices;
mod expose;
mod model;
mod runtime;
mod twin;

pub use model::{LabRecord, LabSummary, LabView, Settings};

/// The device profile this crate ships for the simulator's end hosts.
pub fn host_profile_yaml() -> &'static str {
    devices::SIM_HOST_PROFILE
}

/// Register this feature's settings, state, RPC methods and background tasks.
pub fn init(builder: &mut nocturne_app::AppBuilder) {
    nocturne_app::settings::declare(builder, "labs", &Settings::default());
    nocturne_app::profiles::register(builder, devices::SIM_HOST_PROFILE);
    builder.state(runtime::LabsState::default());
    builder.state(expose::ExposeState::default());
    api::init(builder);
    design::init(builder);
    builder.on_start(|app| {
        let opener = console::install(app);
        expose::install(app, opener);
    });
}
