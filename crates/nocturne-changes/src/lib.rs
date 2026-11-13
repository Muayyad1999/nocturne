//! The change pipeline: plan, risk, approval, apply, verify, rollback.
//!
//! Specification: `docs/dev/features/changes.md` (conventions: `docs/dev/features/README.md`).

/// Register this feature's settings, state, RPC methods and background tasks.
pub fn init(_builder: &mut nocturne_app::AppBuilder) {}
