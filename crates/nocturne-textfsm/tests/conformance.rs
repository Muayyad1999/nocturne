//! Conformance against the upstream ntc-templates test corpus.
//!
//! Point `NTC_TEMPLATES_DIR` at a checkout of
//! <https://github.com/networktocode/ntc-templates> at the bundled revision
//! (`ntc::upstream_revision()`):
//!
//! ```text
//! NTC_TEMPLATES_DIR=/path/to/ntc-templates \
//!     cargo test -p nocturne-textfsm --test conformance -- --nocapture
//! ```
//!
//! Without the variable the test is skipped.

mod common;

use std::path::PathBuf;

/// Cases that are known not to pass, with the reason (path relative to `tests/`).
const SKIPPED: &[(&str, &str)] = &[];

/// The number of passing cases reached with the bundled revision; fewer is a regression.
const PASSING_CASES: usize = 1897;

#[test]
fn upstream_corpus() {
    let Some(directory) = std::env::var_os("NTC_TEMPLATES_DIR") else {
        eprintln!("NTC_TEMPLATES_DIR is not set; skipping the upstream conformance run");
        return;
    };
    let root = PathBuf::from(directory).join("tests");
    let skip: Vec<&str> = SKIPPED.iter().map(|(case, _)| *case).collect();
    let report = common::run_cases(&root, &skip);

    println!("{}", report.summary());
    println!("skipped: {} ({SKIPPED:?})", SKIPPED.len());
    println!("resolved by directory name instead of the index: {:?}", report.fallbacks);
    println!(
        "{} of {} cases pass ({:.2} %)",
        report.passed(),
        report.total(),
        100.0 * report.passed() as f64 / report.total().max(1) as f64
    );
    assert!(
        report.passed() >= PASSING_CASES,
        "{} cases pass, expected at least {PASSING_CASES}",
        report.passed()
    );
}
