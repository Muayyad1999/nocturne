# Spec: `nocturne-server` — headless server mode

Nocturne is a desktop app first, but the same backend can run on a jump host
or a NOC server and be used from a browser. `nocturne-server` is that mode: a
thin HTTP + WebSocket adapter over the application's single RPC surface, with
the web UI embedded in the binary. It is also the vehicle for end-to-end tests
(Playwright drives the real UI against the real backend through it), so it has
to be boringly correct.

It holds network credentials. Treat every default as a security decision.

## What it adapts

```rust
// crates/nocturne-suite (already exists)
pub use nocturne_app::{App, AppBuilder, AppConfig, AppError, AppResult, Event};
pub fn start(config: AppConfig) -> AppResult<App>;

// nocturne_app
impl AppConfig { pub fn in_memory() -> AppConfig; pub fn at(dir: impl Into<PathBuf>) -> AppConfig; }
impl App {                                   // Clone, cheap
    pub async fn rpc(&self, method: &str, params: serde_json::Value) -> AppResult<serde_json::Value>;
    pub fn methods(&self) -> Vec<&'static str>;
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Event>;
    pub fn shutdown(&self);
}
pub struct Event { pub topic: String, pub payload: serde_json::Value, pub at: i64 }   // Serialize; `at` = ms since epoch
pub struct AppError { pub code: &'static str, pub message: String, pub details: Option<serde_json::Value> }  // Serialize
```

Read `crates/nocturne-app/src/{app.rs,error.rs}` for the exact shapes. The
browser side of this contract is `docs/dev/spec-ui.md` → "API client"; the UI
client is being written against it right now, so the wire format below is
fixed.

## Wire contract (fixed)

| Route | Behaviour |
|---|---|
| `POST /api/rpc` | Body `{"method": "...", "params": <any JSON, optional>}`. Auth required. Always `200` with `{"ok": true, "result": …}` or `{"ok": false, "error": {"code", "message", "details"?}}` when the request reached the app. Transport-level failures use HTTP status **and** the same error envelope: `400` malformed JSON / missing method (`code: "invalid"`), `401` missing or wrong token (`"unauthorized"`), `403` origin/host check failed (`"forbidden"`), `413` body too large (`"invalid"`), `404` unknown path. |
| `GET /api/events` | WebSocket. Auth by `?token=…` query (browsers cannot set headers on WebSocket) **or** `Authorization: Bearer`. Optional `?topics=a.*,b.c` filter (comma list; a trailing `*` matches a prefix; absent = everything). Each text frame is one event `{"topic","payload","at"}`. If the client falls behind and events are dropped, send `{"topic":"events.lagged","payload":{"missed":N},"at":…}` so the UI can refetch. Server pings every 20 s; a client silent for 60 s is dropped. Close cleanly on shutdown. |
| `GET /api/health` | No auth. `{"status":"ok","version":"…"}` only — nothing about the data. |
| `GET /api/info` | Auth required. `{"version","methods": […]}`. |
| `GET /*` | The embedded UI (`ui/dist`). `index.html` is served for `/` with `Cache-Control: no-cache`; hashed files under `/assets/` get `public, max-age=31536000, immutable`; correct `Content-Type` for every file; an unknown top-level path returns `index.html` (the UI uses a hash router, but a typed link must not 404), a deeper one redirects to `/` because the page's asset URLs are relative, and a path that names a file (an extension, or under `/assets/`) is a plain `404` — a browser that asked for a script cannot use HTML. Never serve files from disk in release builds. |

Auth: a bearer token, compared in constant time. Missing/wrong → `401`
with `WWW-Authenticate: Bearer`. The token is: `--token`, else
`NOCTURNE_TOKEN`, else the content of `<data-dir>/server.token`, which is
created on first start with 32 random bytes (hex) and mode `0600` on Unix.

## Security requirements

* **Default bind is `127.0.0.1`.** Binding to any non-loopback address
  without TLS is refused with a clear message unless `--allow-insecure-http`
  is given (a NOC LAN is not a trusted network: the token and every device
  password typed into the UI would cross it in clear text).
* **TLS**: `--tls-cert <pem> --tls-key <pem>`, or `--tls-self-signed`, which
  creates (and reuses) a self-signed certificate + key in the data directory
  (`server-cert.pem`, `server-key.pem`, key mode `0600`) valid for the bind
  address, `localhost` and the machine hostname, and prints its SHA-256
  fingerprint at start. rustls with the **ring** provider only — no
  `aws-lc-rs`, no OpenSSL, nothing that needs a C toolchain beyond what ring
  needs (check feature flags of whatever you add; `cargo tree -p
  nocturne-server -i aws-lc-rs` must find nothing).
* **DNS-rebinding and cross-site protection**: when bound to loopback, reject
  requests whose `Host` is not a loopback name/address (`localhost`,
  `127.0.0.1`, `[::1]`, with the right port) → `403`. On `/api/*`, when an
  `Origin` header is present it must match the request's own scheme+host+port
  → otherwise `403`. No CORS headers at all (the UI is same-origin).
* Request body limit 16 MiB; RPC call timeout none (long calls are legitimate)
  but the connection must not be held by a slow-loris: header read timeout 15 s.
* Security headers on every response: `X-Content-Type-Options: nosniff`,
  `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`,
  `Cross-Origin-Opener-Policy: same-origin`, and on HTML a CSP:
  `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self' ws: wss:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'`.
* The token never appears in logs. Log lines for requests show method, path
  (query stripped), status, duration, and for RPC the RPC method name — never
  params or results.
* A panic in a handler must not take the server down (catch-panic layer →
  `500` with `code: "internal"`).

## Embedded UI

`rust-embed` over `../../ui/dist`. The UI may not have been built when the
crate is compiled (CI builds it first; a developer may not). A `build.rs`
makes that work: if `ui/dist/index.html` is missing, it embeds a small
built-in fallback page instead (from a directory inside this crate) that says,
in one sentence, that the UI was not built into this binary and how to build
it (`pnpm --dir ui build`). It must never write into `ui/` and must never
overwrite a real build. `cargo:rerun-if-changed` on `ui/dist`. A
`--ui-dir <path>` flag (debug and release) serves a directory from disk
instead, for UI development against a real backend.

## Library API

```rust
pub struct ServerOptions {
    pub bind: std::net::SocketAddr,           // default 127.0.0.1:7878
    pub token: String,
    pub tls: Option<Tls>,
    pub allow_insecure_http: bool,
    pub ui_dir: Option<std::path::PathBuf>,
}
pub enum Tls { Files { cert: PathBuf, key: PathBuf }, Pem { cert: String, key: String } }
pub struct ServerHandle { /* local_addr(), url() (with #token=…), shutdown().await */ }
pub async fn serve(app: nocturne_suite::App, options: ServerOptions) -> anyhow::Result<ServerHandle>;
```

`serve` returns once the listener is bound (port 0 supported — tests rely on
it). `shutdown` stops accepting, closes WebSockets, and resolves when in-flight
requests have finished or after 5 s.

## Binary `nocturne-server`

```
nocturne-server [serve] [--data-dir DIR] [--bind ADDR] [--port N] [--token T]
                [--tls-cert F --tls-key F | --tls-self-signed] [--allow-insecure-http]
                [--ui-dir DIR] [--open] [--log-level L]
nocturne-server call <method> [params-json] [--url URL] [--token T] [--data-dir DIR] [--insecure]
nocturne-server methods [--url …]
nocturne-server token [--data-dir DIR]          # print the token (creates it if needed)
```

* Default data directory: the platform data dir + `Nocturne` (`dirs`
  crate; the workspace declares `dirs = "7"`), overridable by `--data-dir`
  or `NOCTURNE_DATA_DIR`. `serve` writes `<data-dir>/server.json`
  (`{"url","pid"}`) while running and removes it on exit, so `call` finds a
  running instance without arguments.
* On start it prints exactly one line to stdout with the URL to open, e.g.
  `Nocturne is listening on http://127.0.0.1:7878/#token=…` (stdout only;
  logs go to stderr), so scripts can capture it. `--open` opens the browser.
* `call` is a small scripting client: prints the result as pretty JSON on
  stdout and exits 0, or prints `error [code]: message` on stderr and exits 1
  when the method ran and failed, or 2 when it never ran at all (the server
  is unreachable, the address is wrong, the token is refused) — a script can
  then retry the second and not the first. Params may be given as a JSON argument or `-`
  for stdin. This is the automation hook for people who live in shells.
* Ctrl-C / SIGTERM → graceful shutdown (`app.shutdown()`, then the server).

## Validation you must do

1. Integration tests in `tests/` using the library API on port 0 with an
   in-memory app (`AppConfig::in_memory()`), a real HTTP client and a real
   WebSocket client (add `reqwest` — workspace version, rustls/ring — and
   `tokio-tungstenite` as dev-dependencies; mind the ring-only rule):
   RPC success and app error envelopes (`devices.create` then `devices.list`;
   `devices.get` of a missing id → `not_found`); malformed body; unknown
   method; auth matrix (none / wrong / right; header and query for WS);
   host and origin checks; body limit; events arrive over WS after a
   mutating RPC (`devices.changed`), topic filters, the lagged notice (use a
   small channel or a burst), ping/idle handling, clean shutdown with an open
   socket; static file serving, caching headers, SPA fallback, security
   headers and CSP; the fallback page when no UI is embedded; `--ui-dir`.
2. TLS: start with a self-signed certificate and call it with a client that
   trusts exactly that certificate; check the non-loopback-without-TLS refusal.
3. The binary: spawn it with a temp data dir on port 0 (`--port 0` must print
   the real port), parse the printed URL, call `devices.list` through the
   `call` subcommand, check the token file permissions, send SIGTERM and
   expect exit code 0 and `server.json` removed.
4. `cargo clippy -p nocturne-server --all-targets -- -D warnings` clean,
   `cargo fmt -p nocturne-server`, no `unsafe`.
5. `cargo check -p nocturne-server --target x86_64-pc-windows-gnu` (the
   target and mingw-w64 may already be installed; install them if not) — the
   signal handling and file-permission code must have Windows branches.
