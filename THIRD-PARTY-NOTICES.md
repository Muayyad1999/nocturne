# Third-party notices

Nocturne bundles or derives data from the following projects. Rust crate
dependencies are listed in `Cargo.lock`; their licences can be listed with
`cargo license` or `cargo deny check licenses`.

| Component | Where | Licence |
|---|---|---|
| ntc-templates (TextFSM templates and index, by Network to Code and contributors) | `crates/nocturne-textfsm/templates/` | Apache-2.0 (copy in that directory) |
| Recorded device output used as test fixtures (from the ntc-templates test corpus) | `crates/nocturne-textfsm/tests/fixtures/`, `crates/nocturne-drivers/tests/profiles/` | Apache-2.0 |
| Inter and JetBrains Mono fonts | bundled by the UI build (`@fontsource-variable/*`) | SIL Open Font License 1.1 |
| Lucide icons | UI | ISC |

The TextFSM engine in `crates/nocturne-textfsm` is an independent Rust
implementation of the TextFSM template language (originally created at
Google, Apache-2.0); no upstream source code is included.
