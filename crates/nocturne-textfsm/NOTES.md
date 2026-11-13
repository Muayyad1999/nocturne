# nocturne-textfsm — working notes (resume file)

Spec: `docs/dev/spec-textfsm.md`. Upstream: ntc-templates at commit
`84ae82fd022807810d73a19655cca113b734de58` (979 templates + `index`), reference engine:
`pip install textfsm` (2.1.1, run on CPython 3.13).

Rules of the build: `CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/textfsm`, every cargo
command with `-p nocturne-textfsm` (also `cargo fmt -p nocturne-textfsm`, never a bare `cargo fmt`),
touch only this crate, no git commands, do not edit the workspace `Cargo.toml`.

## Status: done

Nothing is in progress. This file can be deleted once the crate is committed; everything a
maintainer needs is in the code documentation (`src/lib.rs`, `src/pyregex/mod.rs`,
`src/pyregex/loops.rs`, `templates/README.md`, the doc comments of `tests/*.rs` and `tools/*.py`).

## Layout

- `templates/` — verbatim upstream bundle (+ `LICENSE`, `README.md`); `build.rs` packs it into one
  zlib blob (270,130 bytes; 1,197,038 raw) embedded by `src/ntc/bundle.rs`.
- `src/template.rs` (template language), `src/fsm.rs` (run engine), `src/text.rs` (Python string
  semantics), `src/value.rs`, `src/error.rs`.
- `src/pyregex/` — Python `re` on the Rust engines: `parser.rs` (port of CPython's parser),
  `emit.rs` (translation, two flavours), `loops.rs` (final empty iteration), `language.rs`,
  `mod.rs` (`Regex`: ASCII flavour for ASCII text, lazily compiled Unicode-aware flavour otherwise).
- `src/ntc/` — `bundle.rs`, `index.rs` (clitable index), `merge.rs` (multi-template rows),
  `mod.rs` (public API).
- `tests/semantics/*.yml` + `tests/semantics.rs` — 151 reference-verified cases.
- `tests/fixtures/` + `tests/fixtures.rs` — 118 upstream cases (always on).
- `tests/conformance.rs` — the whole upstream corpus (`NTC_TEMPLATES_DIR`).
- `tools/semantics.py`, `tools/regex_oracle.py`, `tools/fuzz_regex.py` — reference data generators.
- `examples/timing.rs` — timing measurement.

## How to re-verify everything

```sh
export CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/textfsm
cargo fmt -p nocturne-textfsm -- --check
cargo clippy -p nocturne-textfsm --all-targets -- -D warnings
cargo test -p nocturne-textfsm                      # always-on tests

# reference data (python3 -m venv venv && venv/bin/pip install textfsm pyyaml)
python tools/semantics.py check tests/semantics/*.yml
python tools/regex_oracle.py /home/user/.cache/vendor/ntc-templates /tmp/oracle.jsonl
python tools/fuzz_regex.py 150000 7 /tmp/fuzz_regex.json
python tools/semantics.py fuzz 100000 99 /tmp/fuzz_cases.json

NTC_TEMPLATES_DIR=/home/user/.cache/vendor/ntc-templates PYREGEX_ORACLE=/tmp/oracle.jsonl \
PYREGEX_FUZZ=/tmp/fuzz_regex.json TEXTFSM_CASES=/tmp/fuzz_cases.json \
  cargo test --release -p nocturne-textfsm -- --nocapture
cargo run --release -p nocturne-textfsm --example timing
```

## Results at hand-over

- Corpus: 1897 / 1897 cases pass, no skips, no fallbacks (the Python reference also passes all).
- Regex oracle: 543,213 evaluations equal to Python; 539,216 repeated with the Unicode flavour.
- Random patterns: 150,002 cases, 0 unexplained differences (67 in the documented classes).
- Random templates: about 520,000 cases over six seeds (300,000 of them with damaged templates),
  all equal to Python.
- Timing (release, loaded 4-core box): compile `cisco_ios_show_interfaces` 2.9 ms, parse 5,003
  lines 12 ms (medians of 25 runs). Memory: all 979 templates compiled and used: 329 MiB.

## Decisions worth knowing

- `\d`, `\w`, `\s`, `\b` are Unicode aware exactly as in Python (the spec suggested ASCII classes;
  on ASCII text there is no difference, and ASCII text is served by a small ASCII flavour).
- A `List` item for a group that took no part in a match is the string `"None"` (corpus expects it).
- Six index rows list several templates; `ntc::parse` merges their columns like clitable,
  `ntc::find` returns the first, `ntc::find_all` all of them.
- Known differences from Python: loops over empty matches other than the emulated shape
  (`src/pyregex/loops.rs`), `\B` in an empty text (Python 3.14 behaviour), `(?a)` and `\N{...}`
  unsupported, nested named groups in `List` values give plain text. Where the reference crashes
  with a `TypeError` (a `List` value with nested groups whose group took no part in the match),
  this crate records a `"None"` item (pinned by a unit test in `src/fsm.rs`).
