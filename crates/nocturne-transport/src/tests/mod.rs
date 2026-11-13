//! In-process tests: this crate's clients against this crate's servers.
//!
//! (Interoperability with real daemons is tested from `tests/`.)

mod local;
mod serial;
mod ssh;
mod support;
mod telnet;
