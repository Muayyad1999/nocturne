//! Conformance against the upstream ntc-templates test corpus.
//!
//! Point `NTC_TEMPLATES_DIR` at a checkout of
//! <https://github.com/networktocode/ntc-templates> at the bundled revision
//! (`ntc::upstream_revision()`):
//!
//! ```text
//! NTC_TEMPLATES_DIR=/path/to/ntc-templates \
//!     cargo test --release -p nocturne-textfsm --test conformance -- --nocapture
//! ```
//!
//! Without the variable the test is skipped. Every case of the corpus is
//! parsed and compared with the rows upstream expects; the test prints a table
//! per platform and the first difference of every failing case.

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};

use nocturne_textfsm::ntc;

/// Cases that are known not to pass, with the reason (path relative to
/// `tests/`). The Python reference passes every case of the bundled revision,
/// and so does this crate, so there is nothing to skip.
const SKIPPED: &[(&str, &str)] = &[];

/// The share of cases that pass at the bundled revision: 1897 of 1897.
/// Anything less is a regression.
const REQUIRED_PASS_RATE: f64 = 1.0;

/// The platforms Nocturne supports first; none of their cases may ever fail.
const REQUIRED_PLATFORMS: &[&str] = &[
    "arista_eos",
    "aruba_aoscx",
    "cisco_asa",
    "cisco_ios",
    "cisco_nxos",
    "cisco_xr",
    "fortinet",
    "hp_comware",
    "hp_procurve",
    "huawei_vrp",
    "juniper_junos",
    "linux",
    "mikrotik_routeros",
    "paloalto_panos",
];

#[test]
fn upstream_corpus() {
    let Some(directory) = std::env::var_os("NTC_TEMPLATES_DIR") else {
        // Written to the terminal directly so that the note is not swallowed
        // by the test harness.
        let note = "NTC_TEMPLATES_DIR is not set; skipping the upstream conformance run";
        let _ = writeln!(std::io::stderr(), "{note}");
        return;
    };
    let checkout = PathBuf::from(directory);
    let stale = templates_differing_from_bundle(&checkout);
    if stale > 0 {
        println!(
            "note: {stale} templates of the checkout differ from the bundle (revision {}); \
             expect failures that are not regressions",
            ntc::upstream_revision()
        );
    }

    let skip: Vec<&str> = SKIPPED.iter().map(|(case, _)| *case).collect();
    let report = common::run_cases(&checkout.join("tests"), &skip);
    let (passed, total) = (report.passed(), report.total());
    println!("{}", report.summary());
    println!("skipped: {SKIPPED:?}");
    println!(
        "resolved by directory name instead of the index: {:?}",
        report.fallbacks
    );
    println!(
        "{passed} of {total} cases pass ({:.2} %)",
        100.0 * passed as f64 / total.max(1) as f64
    );

    assert!(total > 0, "no test cases below {}", checkout.display());
    assert!(
        passed as f64 >= REQUIRED_PASS_RATE * total as f64,
        "{passed} of {total} cases pass, required: {:.2} %",
        100.0 * REQUIRED_PASS_RATE
    );
    for platform in REQUIRED_PLATFORMS {
        let tally = report.platforms.get(*platform);
        let tally = tally.unwrap_or_else(|| panic!("no cases for {platform}"));
        assert_eq!(tally.failed, 0, "cases of {platform} fail");
    }
}

/// How many templates of the checkout are missing from the bundle or differ from it.
fn templates_differing_from_bundle(checkout: &Path) -> usize {
    let directory = checkout.join("ntc_templates/templates");
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return 0;
    };
    entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "textfsm")
        })
        .filter(|path| {
            let name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default();
            ntc::source(name) != std::fs::read_to_string(path).ok().as_deref()
        })
        .count()
}
