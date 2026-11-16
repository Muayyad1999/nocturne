//! Monitoring: is everything up, how loaded is it, and what needs a human
//! right now.
//!
//! Specification: `docs/dev/features/monitor.md` (conventions:
//! `docs/dev/features/README.md`).
//!
//! The crate polls every monitored device on its own schedule (ICMP, SNMP,
//! and the command line where there is nothing else), keeps the numbers as
//! time series, evaluates alert rules after each poll and notifies.
//!
//! Three things it will not do, because a NOC screen that lies is worse than
//! no NOC screen:
//!
//! * **It never invents a number.** A read that failed leaves a gap in the
//!   series. The only samples a failed poll writes are `up = 0` and the loss
//!   that echoes actually measured.
//! * **It never turns a reboot into a spike.** Rates come from counter deltas
//!   over the device's *own* uptime, so a counter that restarted is told
//!   apart from one that wrapped ([`rates`]).
//! * **It raises one alert per cause.** A device that is down suppresses the
//!   interface and threshold alerts that are down because of it.
//!
//! Beside the one verdict per device, each device can carry named
//! [`checks`] — ping, DNS, HTTP, HTTPS, TLS, NTP, a TCP port, SNMP — each on
//! its own schedule with its own state, history and alert, so that "the web
//! server on 10.0.0.5 is down but the box answers ping" is a sentence this
//! crate can say.

pub mod alerts;
pub mod checks;
pub mod clock;
mod db;
pub mod interfaces;
pub mod maintenance;
pub mod metrics;
pub mod model;
pub mod notify;
pub mod poll;
pub mod probe;
pub mod rates;
pub mod read;
pub mod rules;
pub mod scheduler;
pub mod scope;
pub mod settings;
pub mod smtp;
pub mod snmp;
pub mod state;
pub mod status;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;

use std::sync::{Arc, OnceLock};

/// The process-wide rustls provider, installed on first use.
///
/// The workspace builds rustls without a bundled provider so that nothing
/// needs a C toolchain; every crate that opens a TLS connection installs
/// `ring` itself, and installing it twice is harmless.
pub(crate) fn crypto_provider() -> Arc<rustls::crypto::CryptoProvider> {
    static PROVIDER: OnceLock<Arc<rustls::crypto::CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
            Arc::new(rustls::crypto::ring::default_provider())
        })
        .clone()
}

/// Register this feature's settings, state, RPC methods and background tasks.
pub fn init(builder: &mut nocturne_app::AppBuilder) {
    nocturne_app::settings::declare(
        builder,
        settings::SECTION,
        &settings::MonitorSettings::default(),
    );
    // Without its tables the feature cannot record anything; failing here
    // means a broken database, not a condition to carry on with.
    db::init(builder.store()).expect("the monitoring feature's tables can be created");
    builder.state(state::MonitorState::default());

    status::register(&mut builder.router);
    metrics::register(&mut builder.router);
    checks::register(&mut builder.router);
    alerts::register(&mut builder.router);
    rules::register(&mut builder.router);
    maintenance::register(&mut builder.router);
    notify::register(&mut builder.router);

    builder.on_start(|app| {
        if let Err(err) = rules::ensure_builtins(app) {
            tracing::error!(%err, "the built-in alert rules could not be written");
        }
        if app.state::<state::MonitorState>().autostart {
            scheduler::spawn(app);
        }
    });
}
