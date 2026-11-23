//! The change pipeline: plan, risk, approval, apply, verify, rollback.
//!
//! The one path by which Nocturne configures a device: **plan** (the exact
//! commands, the predicted configuration, the rollback, the risk and the
//! protected resources), **approve** (bound to that exact plan), **apply**
//! (drift check, backup, dead-man switch, send, verify) and **roll back**
//! when anything is not as predicted.
//!
//! Specification: `docs/dev/features/changes.md` (conventions: `docs/dev/features/README.md`).

pub mod abbrev;
pub mod apply;
pub mod autonomy;
pub mod compare;
pub mod db;
pub mod model;
pub mod plan;
pub mod protect;
pub mod redact;
pub mod risk;
pub mod rollback;
pub mod rpc;
pub mod settings;
pub mod verify;

pub use model::{Change, ChangeState, Intent, Options, Plan, Risk, RiskLevel};

/// Register this feature's settings, state, RPC methods and background tasks.
pub fn init(builder: &mut nocturne_app::AppBuilder) {
    settings::declare(builder);
    builder.state(db::ChangesState::default());
    rpc::register(builder);
    builder.on_start(|app| {
        // A change that was being applied when the process stopped is nobody's
        // any more: say so, and name the devices that need looking at.
        match db::recover_interrupted(app) {
            Ok(0) => {}
            Ok(count) => tracing::warn!(count, "changes were interrupted by a restart"),
            Err(err) => tracing::error!(%err, "interrupted changes could not be recovered"),
        }
    });
}
