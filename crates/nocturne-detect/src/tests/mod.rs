//! Behaviour tests: every detection through `App::rpc` and the events it is
//! decided from, with fakes standing in for the features this crate does not
//! link to (`support`).
//!
//! The events are published in the shape the real features publish them;
//! each helper that builds one says which source file the shape was read
//! from.

mod audit_trail;
mod catalogue_rpc;
mod config_change;
mod host_key;
mod protected;
mod reload;
mod support;
mod wire;
