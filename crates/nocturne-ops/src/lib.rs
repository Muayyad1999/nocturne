//! The operations record: overview, incidents, shift handover, reports, notes
//! and a global search.
//!
//! Specification: `docs/dev/features/ops.md` (conventions:
//! `docs/dev/features/README.md`).
//!
//! Nothing here changes a device. Every number is a view over a record that
//! already exists — alerts, changes, jobs, the audit trail, configuration
//! snapshots, reachability samples — read either from the core store or from
//! another feature by RPC ([`sources`]). A source that is missing or broken
//! produces a `null` section and a reason in the result's `unavailable` list,
//! never a zero.

pub mod audit;
pub mod configs;
pub mod handover;
pub mod incidents;
pub mod model;
pub mod notes;
pub mod overview;
pub mod reach;
pub mod records;
pub mod refs;
pub mod render;
pub mod reports;
pub mod search;
pub mod settings;
pub mod sources;
pub mod text;
pub mod timefmt;
pub mod timeline;

use std::sync::OnceLock;

use parking_lot::Mutex;
use serde_json::Value;

use nocturne_app::AppBuilder;

/// This feature's runtime state.
#[derive(Default)]
pub struct OpsState {
    /// The RPC surface of this build, read once (see [`sources::has`]).
    pub features: OnceLock<sources::Features>,
    /// The last overview and when it was made, so that a wall display asking
    /// every few seconds does not re-read the whole estate each time.
    pub overview: Mutex<Option<(i64, Value)>>,
}

/// Register this feature's settings, state, RPC methods and background tasks.
pub fn init(builder: &mut AppBuilder) {
    settings::declare(builder);
    builder.state(OpsState::default());
    // The feature cannot work without its tables; failing at start is better
    // than failing on the first incident.
    records::ensure_schema(builder.store())
        .expect("the operations record's own tables can be created");
    overview::register(builder);
    incidents::register(builder);
    handover::register(builder);
    reports::register(builder);
    notes::register(builder);
    search::register(builder);
    audit::register(builder);
}
