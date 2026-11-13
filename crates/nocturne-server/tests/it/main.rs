//! Integration tests: a real server on a free port, an in-memory application,
//! and real HTTP, WebSocket and TLS clients.
//!
//! Nothing here depends on `ui/dist`: tests that need UI files serve a
//! temporary directory, and the one test of the built-in UI accepts either
//! the real UI or the fallback page.

mod auth;
// mod binary;   // not written yet (the engineer was interrupted)
mod events;
// mod lifecycle;   // not written yet (the engineer was interrupted)
mod origin;
mod rpc;
mod support;
// mod tls;   // not written yet (the engineer was interrupted)
// mod ui;   // not written yet (the engineer was interrupted)
