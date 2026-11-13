//! Junos `$9$` secrets against `Crypt::Juniper`, the Perl module that
//! published the algorithm.
//!
//! `tests/data/junos9_vectors.json` holds secrets encrypted by the module
//! (see `tests/generate_junos9_vectors.pl`). When Perl and the module are
//! installed, fresh vectors are checked too, and the module is made to
//! decrypt what this crate encodes.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use nocturne_tools::secrets::{junos9_decode, junos9_encode};
use serde_json::Value;

const VENDORED: &str = include_str!("data/junos9_vectors.json");

/// Decode every vector of a generator output; returns how many there were.
fn check(vectors: &Value) -> usize {
    let vectors = vectors["vectors"].as_array().expect("a list of vectors");
    for vector in vectors {
        let (plain, encoded) = (
            vector["plain"].as_str().unwrap(),
            vector["encoded"].as_str().unwrap(),
        );
        assert_eq!(
            junos9_decode(encoded).as_deref(),
            Ok(plain),
            "junos9_decode({encoded:?})"
        );
    }
    vectors.len()
}

fn crypt_juniper_is_installed() -> bool {
    Command::new("perl")
        .args(["-MCrypt::Juniper", "-MJSON::PP", "-e", "1"])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
fn decodes_what_crypt_juniper_encrypted() {
    let vectors: Value = serde_json::from_str(VENDORED).unwrap();
    assert_eq!(
        vectors["meta"]["module"].as_str(),
        Some("Crypt::Juniper 0.02")
    );
    assert!(check(&vectors) >= 600);
}

#[test]
fn decodes_fresh_vectors_from_the_installed_crypt_juniper() {
    if !crypt_juniper_is_installed() {
        eprintln!(
            "SKIPPED: Perl with Crypt::Juniper is not installed; only the vendored vectors were checked"
        );
        return;
    }
    let generator =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/generate_junos9_vectors.pl");
    let output = Command::new("perl")
        .arg(&generator)
        .args(["777", "400"])
        .output()
        .expect("perl runs");
    assert!(
        output.status.success(),
        "the generator failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let count = check(&serde_json::from_slice(&output.stdout).unwrap());
    assert!(count >= 400);
    eprintln!("checked {count} fresh vectors");
}

#[test]
fn crypt_juniper_decrypts_what_this_crate_encodes() {
    if !crypt_juniper_is_installed() {
        eprintln!("SKIPPED: Perl with Crypt::Juniper is not installed");
        return;
    }
    // Secrets of every length up to 80, over all printable ASCII, with seeds
    // that reach all four salt families.
    let plains: Vec<String> = (1..=400usize)
        .map(|number| {
            (0..1 + number % 80)
                .map(|index| char::from(b' ' + ((number * 31 + index * 7) % 95) as u8))
                .collect()
        })
        .collect();
    let encoded: Vec<String> = plains
        .iter()
        .enumerate()
        .map(|(number, plain)| {
            let mut seed = [0u8; 32];
            seed[..8].copy_from_slice(&(number as u64).to_be_bytes());
            junos9_encode(plain, &seed).unwrap()
        })
        .collect();

    let script = "local $/; my $in = decode_json(<STDIN>); print encode_json([map { juniper_decrypt($_) } @$in]);";
    let mut perl = Command::new("perl")
        .args(["-MCrypt::Juniper", "-MJSON::PP", "-e", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("perl runs");
    perl.stdin
        .take()
        .expect("a pipe to perl")
        .write_all(serde_json::to_string(&encoded).unwrap().as_bytes())
        .unwrap();
    let output = perl.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Crypt::Juniper rejected our output:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let decrypted: Vec<String> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(decrypted, plains);
    eprintln!(
        "Crypt::Juniper decrypted {} secrets encoded by this crate",
        decrypted.len()
    );
}

/// `NOCTURNE_TOOLS_JUNOS9_VECTORS=/path/to/vectors.json cargo test` checks
/// any other file in the format of `generate_junos9_vectors.pl`.
#[test]
fn decodes_vectors_named_in_the_environment() {
    let Some(path) = std::env::var_os("NOCTURNE_TOOLS_JUNOS9_VECTORS") else {
        eprintln!("SKIPPED: NOCTURNE_TOOLS_JUNOS9_VECTORS is not set");
        return;
    };
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.to_string_lossy()));
    eprintln!(
        "checked {} vectors from {}",
        check(&serde_json::from_slice(&bytes).unwrap()),
        path.to_string_lossy()
    );
}
