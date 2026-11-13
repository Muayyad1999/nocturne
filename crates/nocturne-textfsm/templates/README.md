# Bundled ntc-templates

This directory is a verbatim copy of `ntc_templates/templates/` (every
`*.textfsm` template and the `index` file) from
[networktocode/ntc-templates](https://github.com/networktocode/ntc-templates).

* Upstream commit: `84ae82fd022807810d73a19655cca113b734de58` (2026-11-11)
* Licence: Apache License 2.0, see [`LICENSE`](LICENSE) (copied from the upstream repository root)
* Contents: 979 templates and the `index` file

The files are not modified. `build.rs` packs them into one compressed blob that
is embedded in the crate, so nothing here is read at run time.

## Updating

1. Replace the `*.textfsm` files and `index` with the ones from the new upstream
   commit (delete templates that upstream removed).
2. Update the commit hash above and `UPSTREAM_REVISION` in `src/ntc/bundle.rs`
   (a unit test checks that the two agree).
3. Run the conformance suite against the matching upstream checkout:
   `NTC_TEMPLATES_DIR=/path/to/ntc-templates cargo test -p nocturne-textfsm --test conformance -- --nocapture`
   and refresh `tests/fixtures/` if the expected output of a fixture changed.
