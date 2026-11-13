//! The offline half: calculators, decoders, converters and reference tables.
//!
//! Every method here is a thin, validated wrapper over `nocturne-tools` (or,
//! for two of them, over `nocturne-drivers` and `nocturne-snmp`): it reads the
//! parameters, calls the engine, and returns the engine's own result type so
//! the shapes the UI is written against are the shapes the engine documents.
//! No I/O, no device, no clock — these are cheap enough to call on every
//! keystroke, which is why none of them is a job.
//!
//! The one rule worth repeating: a refusal is the engine's message
//! ([`crate::errors::tool`]), never a default answer. `tools.subnet` of
//! `10.0.300.1/24` says which octet is wrong; it does not quietly return
//! something about `10.0.0.1`.

use nocturne_app::Router;

pub mod addressing;
pub mod identity;
pub mod reference;
pub mod secrets;
pub mod text;
pub mod units;

pub fn register(router: &mut Router) {
    addressing::register(router);
    identity::register(router);
    secrets::register(router);
    units::register(router);
    reference::register(router);
    text::register(router);
}
