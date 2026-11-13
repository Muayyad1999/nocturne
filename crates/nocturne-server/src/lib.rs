//! Nocturne's headless server mode: the application's RPC surface and event
//! bus over HTTP and WebSocket, with the web UI inside the binary.
//!
//! ```no_run
//! # async fn example() -> anyhow::Result<()> {
//! use nocturne_server::{ServerOptions, serve};
//! use nocturne_suite::AppConfig;
//!
//! let app = nocturne_suite::start(AppConfig::in_memory())?;
//! let server = serve(app.clone(), ServerOptions::default()).await?;
//! println!("open {}", server.url());
//! // …
//! app.shutdown();
//! server.shutdown().await;
//! # Ok(())
//! # }
//! ```
//!
//! | Route | |
//! |---|---|
//! | `POST /api/rpc` | `{"method", "params"?}` → `{"ok": true, "result"}` or `{"ok": false, "error": {"code", "message", "details"?}}` |
//! | `GET /api/events` | WebSocket; one `{"topic", "payload", "at"}` per text frame; `?topics=a.*,b.c` filters |
//! | `GET /api/health` | `{"status": "ok", "version"}`, no token needed |
//! | `GET /api/info` | `{"version", "methods"}` |
//! | everything else | the web UI |
//!
//! The server holds network credentials, so its defaults are closed: it
//! listens on loopback, refuses plain HTTP anywhere else, requires a bearer
//! token for the API and rejects requests that a foreign web page could have
//! caused. [`token`], [`tls`], [`instance`] and [`data_dir`] are the pieces the
//! `nocturne-server` binary adds around [`serve`]; other front ends can use
//! them too.

#![forbid(unsafe_code)]

mod auth;
pub mod data_dir;
mod envelope;
mod events;
mod guard;
mod headers;
pub mod instance;
mod logging;
mod options;
mod private_file;
mod router;
mod rpc;
mod server;
mod state;
mod text;
pub mod tls;
pub mod token;
mod ui;

pub use options::{DEFAULT_PORT, ServerOptions, Timing, Tls};
pub use server::{ServerHandle, serve, serve_with_timing};
pub use ui::ui_is_embedded;

/// The version reported by `/api/health` and `/api/info`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
