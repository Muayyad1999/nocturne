//! A secret must not leave this crate unless someone asked for it in so many
//! words — and when they do, it is written down who did.
//!
//! The tests below push one configuration containing every shape of secret
//! the redactor knows through every method that returns configuration text,
//! and through the events, job logs and exported files on the way.

mod support;

use nocturne_app::App;
use serde_json::{Value, json};
use support::{SECRETS, assert_clean, assert_no_secrets, audits, backup_device, lab, load};

/// The whole zoo, including the forms a fake IOS switch would never print.
const ZOO: &str = "\
hostname R1
enable secret 5 $1$mERr$Th1sIsTheEnableSecretHash
enable secret 8 $8$salt$pbkdf2HashValue000000000000
enable secret 9 $9$NiceTryMate$scryptHashGoesHere0123
enable password 7 0822455D0A1655
username admin privilege 15 secret 5 $1$abcd$MD5HashOfTheUserPassword
username backup password 7 104D000A0618
snmp-server community S3cr3tRead RO
snmp-server community PublicWrite42 RW
snmp-server host 10.0.0.9 version 2c TrapCommunity99
snmp-server user NMS GROUP v3 auth sha AuthPass123 priv aes 128 PrivPass456
tacacs-server host 10.1.1.1 key 7 094F471A1A0A464058
tacacs server CORP
 key 7 13061E010803557878
radius-server host 10.2.2.2 key RadiusSharedKey
aaa group server radius RAD
 key 0 AnotherRadiusKey
interface Dialer1
 ppp chap password 7 02050D480809
 ppp chap hostname branch
crypto isakmp key MySharedVpnKey address 198.51.100.7
crypto ikev2 keyring KR
 peer HQ
  pre-shared-key local PreSharedLocal99
  pre-shared-key remote PreSharedRemote88
key chain OSPF-KEYS
 key 1
  key-string 7 00071A150754
dot11 ssid CORP
 wpa-psk ascii 7 112A4F1B0A0B5C
ntp authentication-key 1 md5 11223344556677 7
line vty 0 4
 password 7 030752180500
 transport input ssh
crypto pki certificate chain TP
 certificate self-signed 01
-----BEGIN RSA PRIVATE KEY-----
MIIEowIBAAKCAQEAx7Qk9mfakekeymaterialthatmustneverbeshown0000000
d2hhdGV2ZXIgeW91IGRvLCBkbyBub3QgcHJpbnQgdGhpcyBsaW5lIGFueXdoZXJl
-----END RSA PRIVATE KEY-----
 quit
";

/// Secrets of [`ZOO`] that are not in the shared list.
const EXTRA: &[&str] = &[
    "$8$salt$pbkdf2HashValue000000000000",
    "$1$abcd$MD5HashOfTheUserPassword",
    "AuthPass123",
    "PrivPass456",
    "RadiusSharedKey",
    "AnotherRadiusKey",
    "PreSharedLocal99",
    "PreSharedRemote88",
    "MIIEowIBAAKCAQEAx7Qk9mfakekeymaterial",
    "d2hhdGV2ZXIgeW91IGRvLCBkbyBub3QgcHJpbnQ",
];

#[track_caller]
fn assert_zoo_hidden(what: &str, text: &str) {
    assert_no_secrets(what, text);
    for secret in EXTRA {
        assert!(
            !text.contains(secret),
            "{what} leaked the secret `{secret}`:\n{text}"
        );
    }
}

/// Store a configuration as if a backup had fetched it.
fn store(app: &App, device_id: &str, text: &str) -> i64 {
    let (version, _) = app
        .store()
        .config_insert(device_id, "running", text, "manual", "")
        .expect("the configuration is stored");
    version.id
}

#[tokio::test]
async fn every_shape_of_secret_is_hidden_by_config_get() {
    let lab = lab(&["SW1"]).await;
    let version = store(&lab.app, &lab.id("SW1"), ZOO);

    let shown = lab
        .app
        .rpc("config.get", json!({"version_id": version}))
        .await
        .expect("the configuration");
    assert_eq!(shown["redacted"], true);
    let text = shown["text"].as_str().unwrap();
    assert_zoo_hidden("config.get", text);

    // What is left still reads like a configuration.
    assert!(text.contains("hostname R1"));
    assert!(text.contains("snmp-server community <secret:"), "{text}");
    assert!(text.contains("transport input ssh"));
    assert!(text.contains(" ppp chap hostname branch"));
    // A private key is one placeholder, and the lines after it keep their numbers.
    assert_eq!(text.lines().count(), ZOO.lines().count());
    assert_eq!(
        text.lines().last(),
        ZOO.lines().last(),
        "the line after the key is where it was"
    );
    assert!(!shown["version"].to_string().contains("secret"));
}

#[tokio::test]
async fn unredacted_text_must_be_asked_for_and_is_audited() {
    let lab = lab(&["SW1"]).await;
    let version = store(&lab.app, &lab.id("SW1"), ZOO);
    assert!(audits(&lab.app, "config.view_unredacted").is_empty());

    let shown = lab
        .app
        .rpc(
            "config.get",
            json!({"version_id": version, "redact": false}),
        )
        .await
        .expect("the configuration");
    assert_eq!(shown["redacted"], false);
    assert!(
        shown["text"]
            .as_str()
            .unwrap()
            .contains("$1$mERr$Th1sIsTheEnableSecretHash"),
        "the real text was asked for"
    );

    let audit = audits(&lab.app, "config.view_unredacted");
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].message, "Viewed unredacted configuration of `SW1`");
    assert_eq!(audit[0].device_id.as_deref(), Some(lab.id("SW1").as_str()));
    assert_eq!(audit[0].body.as_ref().unwrap()["version_id"], version);
    // The audit entry itself does not quote the configuration.
    assert_clean("the audit entry", &audit[0]);
}

#[tokio::test]
async fn a_backup_leaks_nothing_through_its_events_its_log_or_its_result() {
    let (lab, _changes) = support::lab_with_changes(&["SW1"]).await;
    load(&lab, "SW1", SECRETS);
    let mut events = support::Events::new(&lab.app);

    let job = support::backup(&lab, json!({}))
        .await
        .expect("the backup runs");
    assert_clean("the job", &job);
    for event in events.drain() {
        assert_clean(&format!("the `{}` event", event.topic), &event.payload);
    }

    // The configuration really was read: it is there, with its secrets hidden.
    let latest = lab
        .app
        .rpc("config.latest", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("the stored configuration");
    let text = latest["text"].as_str().unwrap();
    assert_no_secrets("config.latest", text);
    assert!(text.contains("snmp-server community <secret:"), "{text}");
    assert!(text.contains("crypto isakmp key <secret:"), "{text}");

    // A second backup that changes a secret still says nothing about it.
    support::configure(
        &lab,
        "SW1",
        &["snmp-server community AnotherSecret42 RO"],
        false,
    )
    .await;
    let mut events = support::Events::new(&lab.app);
    let job = support::backup(&lab, json!({}))
        .await
        .expect("the backup runs");
    assert_eq!(job.result.clone().unwrap()["changed"], 1);
    assert_clean("the second job", &job);
    let payloads: Vec<Value> = events
        .drain()
        .into_iter()
        .map(|event| event.payload)
        .collect();
    let all = serde_json::to_string(&payloads).unwrap();
    assert!(!all.contains("AnotherSecret42"), "{all}");
    assert_no_secrets("the events of the second backup", &all);
}

#[tokio::test]
async fn a_diff_hides_both_sides_and_gives_one_secret_one_placeholder() {
    let lab = lab(&["SW1"]).await;
    let id = lab.id("SW1");
    let before = store(&lab.app, &id, ZOO);
    let after = store(
        &lab.app,
        &id,
        &ZOO.replace("S3cr3tRead", "S3cr3tRead2")
            .replace("hostname R1", "hostname R1\nip name-server 192.0.2.53"),
    );

    let diff = lab
        .app
        .rpc("config.diff", json!({"a": before, "b": after}))
        .await
        .expect("the diff");
    assert_eq!(diff["redacted"], true);
    assert_clean("config.diff", &diff);
    let text = serde_json::to_string(&diff).unwrap();
    assert!(!text.contains("S3cr3tRead2"), "{text}");

    // An unchanged secret keeps one placeholder on both sides; the changed
    // one gets another, so a reader can see that it changed without seeing it.
    let lines: Vec<&str> = diff["diff"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line["text"].as_str().unwrap())
        .collect();
    let community: Vec<&&str> = lines
        .iter()
        .filter(|line| line.contains("snmp-server community"))
        .collect();
    assert!(
        community.len() >= 2,
        "the community line is on both sides: {community:?}"
    );
    assert_ne!(
        community[0], community[1],
        "a changed secret is a different placeholder"
    );

    let unredacted = lab
        .app
        .rpc(
            "config.diff",
            json!({"a": before, "b": after, "redact": false}),
        )
        .await
        .expect("the diff");
    assert_eq!(unredacted["redacted"], false);
    // Two versions of one device are one device's secrets: one audit entry.
    assert_eq!(audits(&lab.app, "config.view_unredacted").len(), 1);
}

#[tokio::test]
async fn comparing_two_devices_gives_a_shared_secret_the_same_placeholder() {
    let lab = lab(&["SW1", "SW2"]).await;
    store(&lab.app, &lab.id("SW1"), ZOO);
    store(
        &lab.app,
        &lab.id("SW2"),
        &ZOO.replace("hostname R1", "hostname R2"),
    );

    let compare = lab
        .app
        .rpc(
            "config.compare",
            json!({"a": {"device_id": lab.id("SW1")}, "b": {"device_id": lab.id("SW2")}}),
        )
        .await
        .expect("the comparison");
    assert_clean("config.compare", &compare);
    // The two devices share every credential, so only the hostname differs.
    assert_eq!(compare["diff"]["added"], 1);
    assert_eq!(compare["diff"]["removed"], 1);
    let changed: Vec<&str> = compare["diff"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|line| line["tag"] != " ")
        .map(|line| line["text"].as_str().unwrap())
        .collect();
    assert_eq!(changed, vec!["hostname R1", "hostname R2"]);

    // Two devices with different credentials differ in the placeholder only.
    store(
        &lab.app,
        &lab.id("SW2"),
        &ZOO.replace("hostname R1", "hostname R2")
            .replace("MySharedVpnKey", "ADifferentVpnKey"),
    );
    let compare = lab
        .app
        .rpc(
            "config.compare",
            json!({"a": {"device_id": lab.id("SW1")}, "b": {"device_id": lab.id("SW2")}}),
        )
        .await
        .expect("the comparison");
    assert_clean("config.compare", &compare);
    assert!(
        !serde_json::to_string(&compare)
            .unwrap()
            .contains("ADifferentVpnKey")
    );
    assert_eq!(compare["diff"]["added"], 2, "hostname and the VPN key");
}

#[tokio::test]
async fn search_never_shows_a_secret_and_never_finds_one() {
    let lab = lab(&["SW1", "SW2"]).await;
    store(&lab.app, &lab.id("SW1"), ZOO);
    store(&lab.app, &lab.id("SW2"), ZOO);

    let found = lab
        .app
        .rpc(
            "config.search",
            json!({"query": "snmp-server community", "context": 2}),
        )
        .await
        .expect("the search");
    assert_eq!(found["devices_matched"], 2);
    assert!(found["matches"].as_array().unwrap().len() >= 4);
    assert_clean("config.search", &found);
    assert!(
        found["matches"][0]["line"]
            .as_str()
            .unwrap()
            .contains("<secret:"),
        "{found}"
    );

    // Searching for the value itself finds nothing.
    let found = lab
        .app
        .rpc("config.search", json!({"query": "S3cr3tRead"}))
        .await
        .expect("the search");
    assert_eq!(found["matches"], json!([]));

    // Searching for a whole line that carries a secret explains why not.
    let found = lab
        .app
        .rpc(
            "config.search",
            json!({"query": "snmp-server community S3cr3tRead RO"}),
        )
        .await
        .expect("the search");
    assert_eq!(found["matches"], json!([]));
    assert!(
        found["note"]
            .as_str()
            .unwrap_or_default()
            .contains("secrets hidden"),
        "{found}"
    );

    // Even as a regular expression, and even with the surrounding line.
    for query in [
        json!({"query": "community (S3cr3tRead|PublicWrite42)", "regex": true}),
        json!({"query": "MySharedVpnKey", "regex": true}),
        json!({"query": "crypto isakmp key MySharedVpnKey"}),
    ] {
        let found = lab
            .app
            .rpc("config.search", query.clone())
            .await
            .expect("the search");
        assert_eq!(found["matches"], json!([]), "{query} found a secret");
        assert_clean("config.search", &found);
    }

    // `config.missing` answers from the redacted text too: a device "has" the
    // line only when it is still there once the secret is hidden.
    let missing = lab
        .app
        .rpc(
            "config.missing",
            json!({"query": "snmp-server community S3cr3tRead RO"}),
        )
        .await
        .expect("the report");
    assert_eq!(missing["devices"].as_array().unwrap().len(), 2);
    assert!(missing["note"].as_str().is_some(), "{missing}");
}

#[tokio::test]
async fn compliance_evidence_is_redacted() {
    let lab = lab(&["SW1"]).await;
    store(&lab.app, &lab.id("SW1"), ZOO);

    // A rule that is about a secret-bearing line: its evidence quotes the line.
    let tested = lab
        .app
        .rpc(
            "compliance.test",
            json!({
                "rule": {
                    "id": "local.no-weak-community",
                    "title": "No read-write community",
                    "severity": "high",
                    "check": {"type": "not_matches", "pattern": "^snmp-server community \\S+ RW"},
                },
                "device_id": lab.id("SW1"),
            }),
        )
        .await
        .expect("the rule runs");
    assert_eq!(tested["finding"]["status"], "fail");
    let evidence = tested["finding"]["evidence"][0]["line"].as_str().unwrap();
    assert!(
        evidence.starts_with("snmp-server community <secret:"),
        "{evidence}"
    );
    assert!(evidence.ends_with(" RW"), "{evidence}");
    assert_clean("compliance.test", &tested);

    // And through a whole run, including what the event carries.
    let mut events = support::Events::new(&lab.app);
    let run = lab
        .app
        .rpc("compliance.run", json!({}))
        .await
        .expect("the run");
    let results = lab
        .app
        .rpc(
            "compliance.results",
            json!({"run_id": run["run_id"], "limit": 500}),
        )
        .await
        .expect("the findings");
    assert!(results["findings"].as_array().unwrap().len() > 5);
    assert_clean("compliance.results", &results);
    for event in events.drain() {
        assert_clean(&format!("the `{}` event", event.topic), &event.payload);
    }
    assert_clean(
        "the compliance audit trail",
        &audits(&lab.app, "compliance"),
    );
}

#[tokio::test]
async fn an_export_is_unredacted_only_when_asked_and_always_audited() {
    let lab = lab(&["SW1"]).await;
    load(&lab, "SW1", SECRETS);
    backup_device(&lab, "SW1").await;

    let dir = tempfile::tempdir().expect("a temporary folder");
    let hidden = dir.path().join("redacted");
    lab.app
        .rpc(
            "config.export",
            json!({"dir": hidden.to_str().unwrap(), "redact": true}),
        )
        .await
        .expect("the export runs");
    let text = std::fs::read_to_string(hidden.join("SW1.running.cfg")).expect("the file");
    assert_no_secrets("the exported file", &text);
    assert!(text.contains("<secret:"), "{text}");

    let plain = dir.path().join("plain");
    lab.app
        .rpc("config.export", json!({"dir": plain.to_str().unwrap()}))
        .await
        .expect("the export runs");
    let text = std::fs::read_to_string(plain.join("SW1.running.cfg")).expect("the file");
    assert!(
        text.contains("MySharedVpnKey"),
        "an export is plain by default"
    );

    let audit = audits(&lab.app, "config.export");
    assert_eq!(audit.len(), 2);
    assert!(audit[0].message.contains("redacted configurations"));
    assert!(audit[1].message.contains("unredacted configurations"));
}

#[tokio::test]
async fn the_intended_configuration_and_its_drift_are_redacted_too() {
    let lab = lab(&["SW1"]).await;
    load(&lab, "SW1", SECRETS);
    backup_device(&lab, "SW1").await;

    lab.app
        .rpc(
            "config.intended.set",
            json!({
                "device_id": lab.id("SW1"),
                "text": "snmp-server community AnIntendedSecret RO\nntp server 192.0.2.1\n",
            }),
        )
        .await
        .expect("the intended configuration is stored");

    let shown = lab
        .app
        .rpc("config.intended.get", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("the intended configuration");
    assert_eq!(shown["redacted"], true);
    assert!(
        !shown["text"].as_str().unwrap().contains("AnIntendedSecret"),
        "{shown}"
    );

    let drift = lab
        .app
        .rpc("config.drift", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("the drift");
    assert_eq!(drift["redacted"], true);
    assert_clean("config.drift", &drift);
    let commands = serde_json::to_string(&drift["commands"]).unwrap();
    assert!(!commands.contains("AnIntendedSecret"), "{commands}");
    assert!(commands.contains("ntp server 192.0.2.1"), "{commands}");

    // Text copied from a redacted view cannot be stored as the intended one.
    let err = lab
        .app
        .rpc(
            "config.intended.set",
            json!({"device_id": lab.id("SW1"), "text": "snmp-server community <secret:1> RO\n"}),
        )
        .await
        .expect_err("placeholders are refused");
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("redaction placeholders"), "{err}");
}

/// A RouterOS export keeps its secrets in `name=value` pairs; the search and
/// the stored text must hide those too.
#[tokio::test]
async fn routeros_key_value_secrets_are_hidden() {
    let lab = lab(&["SW1"]).await;
    let export = "\
# nov/13/2026 12:00:00 by RouterOS 7.14
/interface wireguard peers
add public-key=\"abc\" preshared-key=\"WireGuardPSKvalue123\"
/ip service
set telnet disabled=yes
/ppp secret
add name=vpnuser password=TheDialUpPassword service=l2tp
/interface wireless security-profiles
set [ find default=yes ] wpa2-pre-shared-key=\"TheWifiPassword99\"
/snmp community
set [ find default=yes ] name=public
/user
add name=backup password=\"AnotherOne!\" group=read
";
    let version = store(&lab.app, &lab.id("SW1"), export);
    let shown = lab
        .app
        .rpc("config.get", json!({"version_id": version}))
        .await
        .expect("the configuration");
    let text = shown["text"].as_str().unwrap();
    for secret in [
        "WireGuardPSKvalue123",
        "TheDialUpPassword",
        "TheWifiPassword99",
        "AnotherOne!",
    ] {
        assert!(!text.contains(secret), "`{secret}` is visible in:\n{text}");
    }
    // What is not a secret stays readable, including the community name that a
    // compliance rule must still be able to judge once it is marked weak.
    assert!(text.contains("set telnet disabled=yes"), "{text}");
    assert!(
        text.contains("add name=vpnuser password=<secret:"),
        "{text}"
    );
    assert_eq!(text.lines().count(), export.lines().count());
}
