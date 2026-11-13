//! The always-on subset of the upstream conformance corpus.
//!
//! `tests/fixtures/<platform>/<command>/` holds one case per command, copied
//! unchanged from the `tests/` directory of ntc-templates at the bundled
//! revision: the commands Nocturne relies on most, for the platforms it
//! supports first. The complete corpus is exercised by `tests/conformance.rs`.

mod common;

use std::path::Path;

/// Platforms that must be represented.
const PLATFORMS: &[&str] = &[
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
fn fixtures_parse_as_upstream_expects() {
    let report = common::run_cases(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
        &[],
    );
    assert!(report.failures.is_empty(), "\n{}", report.summary());
    assert!(
        report.fallbacks.is_empty(),
        "not resolved through the index: {:?}",
        report.fallbacks
    );
    for platform in PLATFORMS {
        assert!(
            report.platforms.contains_key(*platform),
            "no fixtures for {platform}"
        );
    }
    // Guards against the fixture directory being emptied or not checked out.
    assert!(
        report.total() >= 100,
        "only {} fixtures found",
        report.total()
    );
}
