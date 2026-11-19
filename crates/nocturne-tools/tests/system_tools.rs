//! Interoperability with tools found on an ordinary Unix system: the
//! coreutils digest and Base64 commands and `openssl passwd`. Each test
//! skips, with a note, when its tool is not installed.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use nocturne_tools::{secrets, text};

/// A scratch directory that is removed again, also when the test fails.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("nocturne-tools-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run a command with `input` on its standard input; `None` if the program
/// is not installed (or does not understand the arguments).
fn run(program: &str, args: &[&str], input: &[u8]) -> Option<Vec<u8>> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // The writer runs on its own thread so that a full pipe cannot deadlock.
    let mut stdin = child.stdin.take()?;
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let output = child.wait_with_output().ok()?;
    let _ = writer.join();
    output.status.success().then_some(output.stdout)
}

/// Deterministic test data of every byte value.
fn sample(len: usize) -> Vec<u8> {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

#[test]
fn digests_match_coreutils() {
    let scratch = Scratch::new("digests");
    let samples = [
        Vec::new(),
        b"abc".to_vec(),
        b"hostname core-sw-01\n".to_vec(),
        sample(1),
        sample(55),
        sample(56),
        sample(64),
        sample(1 << 20),
    ];
    let tools: Vec<&str> = ["md5sum", "sha1sum", "sha256sum", "sha512sum"]
        .into_iter()
        .filter(|tool| {
            let installed = run(tool, &["--version"], b"").is_some();
            if !installed {
                eprintln!("SKIPPED: {tool} is not installed");
            }
            installed
        })
        .collect();
    let mut checked = 0;
    for (index, data) in samples.iter().enumerate() {
        let file = scratch.0.join(format!("sample-{index}.bin"));
        std::fs::write(&file, data).unwrap();
        let ours = text::hashes(data);
        for tool in &tools {
            let expected = match *tool {
                "md5sum" => &ours.md5,
                "sha1sum" => &ours.sha1,
                "sha256sum" => &ours.sha256,
                _ => &ours.sha512,
            };
            let output = Command::new(tool)
                .arg(&file)
                .output()
                .expect("the tool was there a moment ago");
            assert!(output.status.success(), "{tool} failed");
            let stdout = String::from_utf8_lossy(&output.stdout);
            // GNU's tools mark a line whose *filename* needed escaping by
            // putting a backslash in front of the digest, and on Windows
            // every path has backslashes in it. The digest is what is being
            // compared here, so the marker comes off.
            let digest = stdout
                .split_whitespace()
                .next()
                .map(|field| field.trim_start_matches('\\'));
            assert_eq!(
                digest,
                Some(expected.as_str()),
                "{tool} of sample {index}: {stdout:?}"
            );
            checked += 1;
        }
    }
    eprintln!("compared {checked} digests with coreutils");
}

#[test]
fn base64_matches_coreutils() {
    if run("base64", &["--version"], b"").is_none() {
        eprintln!("SKIPPED: base64 (coreutils) is not installed");
        return;
    }
    for len in [0, 1, 2, 3, 4, 57, 58, 76, 1000, 100_000] {
        let data = sample(len);
        // Unwrapped output is exactly ours ...
        let unwrapped = run("base64", &["-w", "0"], &data).expect("base64 -w 0");
        assert_eq!(
            String::from_utf8(unwrapped).unwrap(),
            text::base64_encode(&data),
            "{len} bytes"
        );
        // ... the default 76-column wrapping is something we must read back ...
        let wrapped = run("base64", &[], &data).expect("base64");
        assert_eq!(
            text::base64_decode(std::str::from_utf8(&wrapped).unwrap()).unwrap(),
            data,
            "{len} bytes, wrapped"
        );
        // ... and what we write, base64 -d reads.
        let decoded =
            run("base64", &["-d"], text::base64_encode(&data).as_bytes()).expect("base64 -d");
        assert_eq!(decoded, data, "{len} bytes, decoded by coreutils");
    }
}

#[test]
fn openssl_password_hashes_are_identified() {
    let mut checked = 0;
    for (flag, kind, prefix) in [
        ("-1", "md5crypt", "$1$"),
        ("-5", "sha256crypt", "$5$"),
        ("-6", "sha512crypt", "$6$"),
    ] {
        let Some(output) = run(
            "openssl",
            &["passwd", flag, "-salt", "nocturne", "-stdin"],
            b"correct horse\n",
        ) else {
            eprintln!("SKIPPED: openssl passwd {flag} is not available");
            continue;
        };
        let hash = String::from_utf8(output).unwrap();
        let hash = hash.trim();
        assert!(hash.starts_with(prefix), "{hash}");
        let found = secrets::identify(hash);
        assert_eq!(
            (found.kind.as_str(), found.reversible),
            (kind, false),
            "{hash}"
        );
        // The same hash inside a Junos and an EOS configuration line.
        assert_eq!(
            secrets::identify(&format!("encrypted-password \"{hash}\"; ## SECRET-DATA")).kind,
            kind
        );
        assert_eq!(
            secrets::identify(&format!("username admin privilege 15 secret {hash}")).kind,
            kind
        );
        // A hash is not something the decoders will pretend to decode.
        assert!(secrets::cisco7_decode(hash).is_err() && secrets::junos9_decode(hash).is_err());
        checked += 1;
    }
    // Cisco type 5 is MD5-crypt behind a type digit.
    if let Some(output) = run(
        "openssl",
        &["passwd", "-1", "-salt", "mERr", "-stdin"],
        b"cisco\n",
    ) {
        let hash = String::from_utf8(output).unwrap();
        assert_eq!(
            secrets::identify(&format!("enable secret 5 {}", hash.trim())).kind,
            "cisco_type5"
        );
        checked += 1;
    }
    eprintln!("identified {checked} hashes made by openssl");
}
