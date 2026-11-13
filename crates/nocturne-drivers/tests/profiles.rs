//! Data-driven tests for the built-in profile pack.
//!
//! Every profile under `profiles/` has a directory under `tests/profiles/`
//! holding real prompts, error messages, command classifications, detection
//! evidence, recorded command output with the model data it must produce,
//! running configurations and scripted sessions. Adding a profile's tests
//! needs no Rust: see "Testing a profile" in `docs/profiles.md` for the file
//! formats.
//!
//! To work on one profile, run
//! `NOCTURNE_PROFILE=<id> cargo test -p nocturne-drivers --test profiles -- --nocapture`.

mod harness;

use std::path::Path;

use nocturne_drivers::profile::Registry;

// The harness below is finished; the profile pack it tests is not written yet
// (`profiles/` is empty), so there is nothing for it to check. Remove this
// attribute together with the first profile.
#[ignore = "the built-in profile pack has yet to be written"]
#[tokio::test(start_paused = true)]
async fn built_in_profiles_pass_their_recorded_tests() {
    let (registry, errors) = Registry::builtin();
    assert!(
        errors.is_empty(),
        "built-in profiles that do not load:\n{}",
        errors
            .iter()
            .map(|err| format!("  {err}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/profiles");
    let only = std::env::var("NOCTURNE_PROFILE").ok();
    let report = harness::run(&registry, &root, only.as_deref()).await;
    println!("{}", report.summary());
    assert!(
        report.failures.is_empty(),
        "\n{} problem(s) in the profile tests:\n\n{}\n",
        report.failures.len(),
        report.failures.join("\n")
    );
    assert!(
        report.profiles > 0,
        "no profile was tested (NOCTURNE_PROFILE={only:?})"
    );
}
