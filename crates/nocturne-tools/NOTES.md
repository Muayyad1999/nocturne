# nocturne-tools — working notes

Running log for whoever picks this crate up after a restart. Spec: `docs/dev/spec-tools.md`.

Build / test (always with the per-crate target dir):

```bash
export CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/tools
cargo test -p nocturne-tools
cargo clippy -p nocturne-tools --all-targets -- -D warnings
cargo fmt -p nocturne-tools
```

## Done

- (nothing yet)

## In progress

- Foundation: `error.rs`, `ip.rs` (strict IPv4/IPv6 parsing, RFC 5952 formatting, prefix maths).

## Next

- `subnet`, `vlsm`, `wildcard`
- `mac` + OUI registry (`data/oui.tsv.gz`, IEEE CSVs downloaded 2026-11-13 into the scratch dir)
- `secrets`, `convert`, `reference`, `text`
- Python `ipaddress` cross-check (generator under `tests/`, vendored JSON), robustness tests, interop tests
- clippy / fmt / report
