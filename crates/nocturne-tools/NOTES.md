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

- `error.rs` — `ToolError` (`Invalid { what, input, reason }`, `TooLarge`, `DoesNotFit`), input echo truncated.
- `ip.rs` — hand-written strict IPv4/IPv6 parsers (fuzz-compared with `std::net`), RFC 5952 formatting,
  `Net`/`Interface`, range merge + range→CIDR.
- `subnet.rs` — info / split / summarize / supernet / contains / overlaps / range_to_cidrs / next_free / free_space.
- `vlsm.rs` — plan (largest first; /32 for 1 host, /30 or /31 for 2).
- `wildcard.rs` — convert / matches / explain.
- `mac.rs` + `data/oui.tsv.gz` (59,032 prefixes: MA-L + MA-M + MA-S + IAB, downloaded 2026-11-13,
  built by `data/build_oui.py`; raw CSVs are in the scratch dir, not in the repo).

## In progress

- `secrets/` (cisco7, junos9, identify, password)

## Next

- `convert/`, `reference/`, `text.rs`
- Python `ipaddress` cross-check (generator under `tests/`, vendored JSON), robustness tests, interop tests
  (passlib for type 7, Crypt::Juniper for `$9$`, coreutils for hashes/base64)
- clippy / fmt / report
