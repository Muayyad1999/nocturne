//! The rules of the profile test harness about where a capture comes from.
//!
//! `tests/profiles.rs` runs the harness over the built-in pack, where every
//! rule is expected to hold — which says nothing about whether a rule would
//! notice if it did not. Here the harness is pointed at small, made-up
//! profiles whose test data breaks one rule at a time.

mod harness;

use std::path::{Path, PathBuf};

use nocturne_drivers::profile::{Origin, Registry};

/// A profile that collects `facts` (always from a recording here) and `qos`.
/// `{claims}` is where the test says what the profile claims about itself.
const PROFILE: &str = r#"
id: rule_os
name: Rule OS
{claims}
cli:
  prompt: '^(?P<host>[\w.\-]+)#\s?$'
policy:
  read_only: ['^show\s+\S']
collect:
  facts:
    - command: show version
      parser: {regex: 'Version (?P<version>\S+)'}
      map: {os_version: version}
  qos:
    - command: show policy-map interface
      parser: {regex: 'Class-map: (?P<class>\S+)'}
      map: {class: class, direction: "'out'"}
"#;

const POLICY_OUTPUT: &str = "    Class-map: VOICE (match-all)\n";
const QOS_ROWS: &str = "kinds:\n  qos:\n    count: 1\n    rows:\n      - class: VOICE\n";

/// The test data every variant shares: prompts, the command policy, and a
/// recording that covers `facts`.
fn base() -> Vec<(&'static str, String)> {
    vec![
        (
            "prompts.yml",
            "exec: ['sw1#']\nnot_prompts: ['Building configuration...']\n".into(),
        ),
        (
            "policy.yml",
            "read_only: ['show version']\nchange: ['configure terminal']\n".into(),
        ),
        (
            "captures/recorded-facts/show_version.txt",
            "Version 15.2\n".into(),
        ),
        (
            "captures/recorded-facts/expect.yml",
            "kinds:\n  facts:\n    rows:\n      - os_version: '15.2'\n".into(),
        ),
        (
            "captures/recorded-facts/SOURCE.md",
            "Recorded from a device.\n".into(),
        ),
    ]
}

/// One capture of `show policy-map interface`: its output, what it must
/// produce, and where it says it came from.
fn qos_case(name: &str, origin: &str, source: &str) -> Vec<(String, String)> {
    vec![
        (
            format!("captures/{name}/show_policy-map_interface.txt"),
            POLICY_OUTPUT.into(),
        ),
        (
            format!("captures/{name}/expect.yml"),
            format!("{origin}{QOS_ROWS}"),
        ),
        (format!("captures/{name}/SOURCE.md"), source.into()),
    ]
}

/// A directory of its own for one test, emptied first.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nocturne-harness-rules-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Run the harness over `PROFILE` with the given claims and capture files,
/// and return what it found wrong.
async fn failures(name: &str, claims: &str, extra: Vec<(String, String)>) -> Vec<String> {
    let mut registry = Registry::empty();
    registry
        // As a built-in: only those may say `verified`.
        .add(&PROFILE.replace("{claims}", claims), Origin::Builtin)
        .expect("the test profile loads");
    let root = scratch(name);
    let files = base()
        .into_iter()
        .map(|(path, text)| (path.to_string(), text))
        .chain(extra);
    for (path, text) in files {
        let path = root.join("rule_os").join(path);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
        std::fs::write(path, text).expect("a file");
    }
    let report = harness::run(&registry, Path::new(&root), None).await;
    assert_eq!(report.profiles, 1, "{}", report.summary());
    let _ = std::fs::remove_dir_all(&root);
    report.failures
}

fn mentions(failures: &[String], text: &str) -> bool {
    failures.iter().any(|failure| failure.contains(text))
}

const VERIFIED: &str = "maturity: verified";
const VERIFIED_BUT_QOS: &str = "maturity: verified\nunverified: [qos]";
const A_PAGE: &str = "Printed on https://vendor.example/reference/show-policy-map.\n";

#[tokio::test(start_paused = true)]
async fn a_verified_profile_needs_a_recording_of_every_kind_it_does_not_list_as_unverified() {
    // Without the list, a synthetic capture is not enough for `verified`…
    let synthetic = qos_case("lab", "origin: synthetic\n", "Written by hand.\n");
    let found = failures("verified-synthetic", VERIFIED, synthetic.clone()).await;
    assert!(
        mentions(
            &found,
            "qos is not covered by a capture recorded from a real device"
        ),
        "{found:#?}"
    );
    // …with it, the same capture is, and nothing else is wrong.
    let found = failures("listed-synthetic", VERIFIED_BUT_QOS, synthetic.clone()).await;
    assert_eq!(found, Vec::<String>::new());
    // The older spelling of a synthetic capture means the same.
    let old_spelling = qos_case("lab", "synthetic: true\n", "Written by hand.\n");
    let found = failures("listed-old-spelling", VERIFIED_BUT_QOS, old_spelling).await;
    assert_eq!(found, Vec::<String>::new());
    // A recording of `qos` makes a verified profile whole without the list.
    let recorded = qos_case("router", "", "Recorded from a device.\n");
    let found = failures("verified-recorded", VERIFIED, recorded).await;
    assert_eq!(found, Vec::<String>::new());
}

#[tokio::test(start_paused = true)]
async fn a_sample_from_the_vendors_documentation_is_not_a_recording() {
    let documented = qos_case("manual", "origin: documented\n", A_PAGE);
    // It does not earn `verified` for the kind…
    let found = failures("verified-documented", VERIFIED, documented.clone()).await;
    assert!(
        mentions(
            &found,
            "qos is not covered by a capture recorded from a real device"
        ),
        "{found:#?}"
    );
    // …and it does not make the harness ask for `unverified` to be lifted:
    // it is exactly what a listed kind is expected to be tested against.
    let found = failures("listed-documented", VERIFIED_BUT_QOS, documented).await;
    assert_eq!(found, Vec::<String>::new());

    // A sample is only as good as the page it can be checked against.
    let no_address = qos_case(
        "manual",
        "origin: documented\n",
        "From the command reference.\n",
    );
    let found = failures("documented-without-address", VERIFIED_BUT_QOS, no_address).await;
    assert!(
        mentions(&found, "captures/manual/SOURCE.md") && mentions(&found, "give the address"),
        "{found:#?}"
    );
}

#[tokio::test(start_paused = true)]
async fn the_unverified_flag_comes_off_the_day_a_recording_arrives() {
    let mut files = qos_case("lab", "origin: synthetic\n", "Written by hand.\n");
    files.extend(qos_case("router", "", "Recorded from a device.\n"));
    let found = failures("listed-and-recorded", VERIFIED_BUT_QOS, files).await;
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].contains("still lists qos under `unverified`")
            && found[0].contains("captures/router")
            && found[0].contains("take qos off the list"),
        "{found:#?}"
    );

    // It is the claim "recorded" that counts, not whether the case passes: a
    // recording the parser gets wrong has met the parser all the same.
    let mut wrong = qos_case("router", "", "Recorded from a device.\n");
    wrong[1].1 = wrong[1].1.replace("VOICE", "VIDEO");
    let found = failures("listed-and-recorded-wrong", VERIFIED_BUT_QOS, wrong).await;
    assert!(
        mentions(&found, "still lists qos under `unverified`"),
        "{found:#?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_recording_of_a_device_that_refuses_the_command_has_shown_the_parser_nothing() {
    // A recorded case that lists the kind but holds no output for it: the
    // device did not answer the command. That is worth recording, and it is
    // no reason to call the parser checked.
    let mut files = qos_case("lab", "origin: synthetic\n", "Written by hand.\n");
    files.extend([
        (
            "captures/switch/show_version.txt".to_string(),
            "Version 12.2\n".to_string(),
        ),
        (
            "captures/switch/expect.yml".to_string(),
            "kinds:\n  facts:\n    rows:\n      - os_version: '12.2'\n  qos: {}\nproblems:\n  - {kind: qos, code: failed, contains: no output was recorded}\n"
                .to_string(),
        ),
        (
            "captures/switch/SOURCE.md".to_string(),
            "Recorded from a device.\n".to_string(),
        ),
    ]);
    let found = failures("listed-and-refused", VERIFIED_BUT_QOS, files).await;
    assert_eq!(found, Vec::<String>::new());
}

#[tokio::test(start_paused = true)]
async fn an_unverified_kind_is_excused_from_a_recording_not_from_being_tested() {
    // No capture of the kind at all.
    let found = failures("listed-untested", VERIFIED_BUT_QOS, Vec::new()).await;
    assert!(
        mentions(&found, "excuses it from a recording, not from being tested"),
        "{found:#?}"
    );

    // A capture that only shows the read failing does not show it working.
    let failing = vec![
        (
            "captures/refused/show_version.txt".to_string(),
            "Version 15.2\n".to_string(),
        ),
        (
            "captures/refused/expect.yml".to_string(),
            "origin: synthetic\nkinds:\n  qos: {}\nproblems:\n  - {kind: qos, code: failed, contains: no output was recorded}\n"
                .to_string(),
        ),
        (
            "captures/refused/SOURCE.md".to_string(),
            "Written by hand.\n".to_string(),
        ),
    ];
    let found = failures("listed-only-failing", VERIFIED_BUT_QOS, failing).await;
    assert!(
        mentions(&found, "excuses it from a recording, not from being tested"),
        "{found:#?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_capture_says_where_it_comes_from_once() {
    let both = qos_case(
        "lab",
        "origin: documented\nsynthetic: true\n",
        "Written by hand.\n",
    );
    let found = failures("both-spellings", VERIFIED_BUT_QOS, both).await;
    assert!(
        mentions(&found, "says both `origin` and `synthetic`"),
        "{found:#?}"
    );
    let unknown = qos_case("lab", "origin: borrowed\n", "Written by hand.\n");
    let found = failures("unknown-origin", VERIFIED_BUT_QOS, unknown).await;
    assert!(mentions(&found, "borrowed"), "{found:#?}");
}

#[tokio::test(start_paused = true)]
async fn a_read_that_finds_nothing_is_expected_as_zero_rows() {
    let empty = |count: &str| {
        vec![
            (
                "captures/bare/show_policy-map_interface.txt".to_string(),
                String::new(),
            ),
            (
                "captures/bare/expect.yml".to_string(),
                format!("origin: synthetic\nkinds:\n  qos:\n    count: {count}\n"),
            ),
            (
                "captures/bare/SOURCE.md".to_string(),
                "Written by hand.\n".to_string(),
            ),
        ]
    };
    // Zero rows, read without a problem: that is the whole expectation, and
    // it covers the kind.
    let found = failures("empty-read", VERIFIED_BUT_QOS, empty("0")).await;
    assert_eq!(found, Vec::<String>::new());
    // The count is still exact.
    let found = failures("empty-read-miscounted", VERIFIED_BUT_QOS, empty("1")).await;
    assert!(
        mentions(&found, "0 rows were collected, expected exactly 1"),
        "{found:#?}"
    );
}
