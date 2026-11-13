//! Syslog, SNMP trap and flow receivers; log search, live tail and pattern rules.
//!
//! Specification: `docs/dev/features/logs.md` (conventions: `docs/dev/features/README.md`).

/// Register this feature's settings, state, RPC methods and background tasks.
pub fn init(_builder: &mut nocturne_app::AppBuilder) {}
