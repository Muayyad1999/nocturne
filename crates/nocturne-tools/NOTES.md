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

- `secrets/` — cisco7 (decode/encode), junos9 (decode + seeded encode helper), identify, password generator.
  One test (`password::tests::pinned_outputs`) still has placeholder values: fill them from the Python
  reference in `tests/generate_vectors.py` once that script exists.
- `convert/` — number, units (sizes/rates/transfer time), duration, mtu (+ `encapsulations()` helper).
- `text.rs` — regex_test, hashes, base64.

- `reference/` — ports (225 entries, names checked against the IANA registry), dscp (IANA registry incl.
  LE and NQB), ip protocols / ethertypes / icmp(v6), administrative distances (checked against vendor
  docs), ethernet standards, syslog levels, interface names.
- `tests/generate_vectors.py` + `tests/data/vectors.json.gz` + `tests/cross_check.rs`: subnet/vlsm vs Python
  `ipaddress`, type 7 vs passlib, password generator vs a hashlib reference. Passes on the vendored set, on a
  fresh live set, and on 3 x 147k cases (`NOCTURNE_TOOLS_VECTORS=file cargo test --test cross_check`).
- clippy clean as of the reference module.

- `tests/generate_junos9_vectors.pl` + `tests/data/junos9_vectors.json` + `tests/junos9_interop.rs`:
  decode vs Crypt::Juniper (vendored + live), and Crypt::Juniper decrypts what `junos9_encode` writes.
  (Crypt/Juniper.pm was copied to /usr/local/share/perl/5.38.2 on this machine for the live tests.)
- `tests/robustness.rs`: junk / mutated / Unicode / 1 MiB inputs through every public function, limits.
  Input-length guards were added where junk was needlessly expensive (addresses, secrets, units, regex
  pattern, durations, port queries).
- `tests/system_tools.rs`: digests vs coreutils, base64 vs coreutils, identify vs `openssl passwd`.
- All suites green, clippy clean at this point.

- `tests/api_contract.rs`: every signature, field name and derive from the spec, plus its examples.
- `lib.rs` crate docs; rustdoc builds with `-D warnings`; `cargo fmt -p nocturne-tools` applied
  (hand-written reference tables are one row per line under `#[rustfmt::skip]`).

## In progress

- Final read-through of every module, then the report.

## Next

- (nothing planned)
