# nocturne-server: running log

Spec: `docs/dev/spec-server.md` (wire contract + library API are fixed).

## Done

- Read the spec, the UI API client (`ui/src/api`), `nocturne-app` (`app.rs`, `error.rs`) and `nocturne-suite`.

## In progress

- Crate scaffold: `Cargo.toml`, `build.rs`, fallback page, module skeleton.

## Next

- Library: options, token, guard (Host/Origin), auth, rpc, events, ui, tls, server (accept loop, shutdown).
- Binary: serve / call / methods / token.
- Tests: unit (in-crate), integration (`tests/it`), binary, TLS, Windows `cargo check`.

## Decisions

(see the bottom of this file; filled in as they are made)
