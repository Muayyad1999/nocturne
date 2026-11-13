//! Configuration backups, versions, diff, search, compliance and drift.
//!
//! Specification: `docs/dev/features/config.md` (conventions: `docs/dev/features/README.md`).
//!
//! The crate is read-only towards devices. It opens CLI sessions to *read*
//! configurations and nothing else; everything that would change a device —
//! a compliance fix, drift, a restore — is handed to the change pipeline
//! through `changes.create` (see `handoff`).

pub mod backup;
pub mod compliance;
pub mod coverage;
mod db;
pub mod diff;
pub mod export;
mod handoff;
pub mod intended;
pub mod outline;
pub mod redact;
mod scope;
pub mod search;
pub mod settings;
pub mod versions;

/// Register this feature's settings, state, RPC methods and background tasks.
pub fn init(builder: &mut nocturne_app::AppBuilder) {
    nocturne_app::settings::declare(
        builder,
        settings::SECTION,
        &settings::ConfigSettings::default(),
    );
    // Without its tables the feature cannot store anything; failing here means
    // a broken database, not a condition to carry on with.
    db::init(builder.store()).expect("the configuration feature's tables can be created");

    backup::register(&mut builder.router);
    versions::register(&mut builder.router);
    coverage::register(&mut builder.router);
    diff::register(&mut builder.router);
    search::register(&mut builder.router);
    export::register(&mut builder.router);
    intended::register(&mut builder.router);
    compliance::register(&mut builder.router);

    builder.on_start(backup::spawn_watcher);
}
