//! The program itself: `serve`, `call`, `methods`, `token`.
//!
//! The library is tested everywhere else; these tests run the real binary, so
//! that what a person types in a terminal is covered too — the line it prints,
//! the files it leaves behind, the exit statuses, and what happens on Ctrl-C.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const BINARY: &str = env!("CARGO_BIN_EXE_nocturne-server");

/// A `nocturne-server serve` that is killed when the test ends.
struct Running {
    child: Child,
    url: String,
    token: String,
}

impl Running {
    /// Start on a free port in `data_dir` and wait for the line it prints.
    fn start(data_dir: &Path) -> Running {
        let mut child = Command::new(BINARY)
            .args(["--data-dir", &data_dir.display().to_string(), "--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the server binary runs");

        // The first line of standard output is the address, and nothing else
        // goes there, so a script can read it.
        let mut stdout = child.stdout.take().expect("stdout");
        let line = read_line(&mut stdout, Duration::from_secs(30))
            .unwrap_or_else(|| panic!("the server printed no address"));
        let url = line
            .split_whitespace()
            .find(|word| word.starts_with("http"))
            .unwrap_or_else(|| panic!("no address in `{line}`"))
            .to_string();
        let (url, token) = url
            .split_once("#token=")
            .expect("the address carries a token");
        Running {
            child,
            url: url.to_string(),
            token: token.to_string(),
        }
    }

    fn api(&self, path: &str) -> String {
        format!("{}{path}", self.url)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Read one line, giving up after `patience`.
fn read_line(stdout: &mut impl Read, patience: Duration) -> Option<String> {
    let deadline = Instant::now() + patience;
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while Instant::now() < deadline {
        match stdout.read(&mut byte) {
            Ok(0) => return None,
            Ok(_) if byte[0] == b'\n' => return Some(String::from_utf8_lossy(&line).to_string()),
            Ok(_) => line.push(byte[0]),
            Err(_) => return None,
        }
    }
    None
}

fn run(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(BINARY)
        .args(args)
        .output()
        .expect("the binary runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
fn serve_prints_one_line_a_script_can_use_and_leaves_its_files_behind() {
    let dir = tempfile::tempdir().unwrap();
    let server = Running::start(dir.path());

    // Port 0 means "pick one", and the line must carry the real port.
    assert!(
        server.url.starts_with("http://127.0.0.1:"),
        "{}",
        server.url
    );
    let port: u16 = server
        .url
        .trim_end_matches('/')
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(port > 0);
    assert_eq!(server.token.len(), 64, "a 32-byte token in hexadecimal");

    for name in ["nocturne.db", "server.token", "server.json", "vault.key"] {
        assert!(
            dir.path().join(name).exists(),
            "{name} should have been created"
        );
    }
    // The token and the vault key are secrets: nobody else may read them.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["server.token", "vault.key"] {
            let mode = std::fs::metadata(dir.path().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "{name} is readable by others");
        }
    }

    // `server.json` is how `call` finds a running instance without arguments.
    let instance: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("server.json")).unwrap())
            .unwrap();
    assert_eq!(
        instance["url"].as_str().unwrap().trim_end_matches('/'),
        server.url.trim_end_matches('/')
    );
    assert!(instance["pid"].as_u64().unwrap() > 0);
}

#[test]
fn call_is_a_client_for_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().display().to_string();
    let server = Running::start(dir.path());

    // No URL and no token: it finds the instance through the data directory.
    let (code, out, _) = run(&["call", "devices.list", "--data-dir", &data]);
    assert_eq!(code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        serde_json::json!([])
    );

    // Parameters as an argument; the result is the only thing on stdout, so
    // it can be piped straight into another program.
    let (code, out, _) = run(&[
        "call",
        "devices.create",
        r#"{"name":"CORE-SW1","address":"10.0.0.1"}"#,
        "--data-dir",
        &data,
    ]);
    assert_eq!(code, 0, "{out}");
    let created: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(created["name"], "CORE-SW1");

    let (code, out, _) = run(&["call", "devices.list", "--data-dir", &data]);
    assert_eq!(code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&out)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // An application error exits 1 and explains itself on standard error,
    // leaving standard output clean.
    let (code, out, err) = run(&[
        "call",
        "devices.get",
        r#"{"id":"gone"}"#,
        "--data-dir",
        &data,
    ]);
    assert_eq!((code, out.trim()), (1, ""));
    assert!(err.contains("not_found"), "{err}");

    // A call that never arrives is a different kind of failure: exit 2.
    let (code, _, err) = run(&[
        "call",
        "devices.list",
        "--url",
        "http://127.0.0.1:1/",
        "--token",
        "x",
    ]);
    assert_eq!(code, 2, "{err}");

    // The right token is required even from the command line. A refusal at
    // the door is exit 2, like an unreachable server: the method never ran,
    // which is what a script needs to tell apart from "it ran and said no".
    let (code, _, err) = run(&[
        "call",
        "devices.list",
        "--url",
        &server.api(""),
        "--token",
        "wrong",
    ]);
    assert_eq!(code, 2);
    assert!(err.contains("unauthorized"), "{err}");

    // `methods` lists what this build can do.
    let (code, out, _) = run(&["methods", "--data-dir", &data]);
    assert_eq!(code, 0);
    assert!(
        out.contains("devices.list") && out.contains("app.info"),
        "{out}"
    );
}

#[test]
fn token_prints_the_token_and_creates_one_when_there_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().display().to_string();

    let (code, first, _) = run(&["token", "--data-dir", &data]);
    assert_eq!(code, 0);
    assert_eq!(first.trim().len(), 64);
    assert!(dir.path().join("server.token").exists());

    // Asking again gives the same token: it is the instance's identity, not a new one each time.
    let (_, again, _) = run(&["token", "--data-dir", &data]);
    assert_eq!(again.trim(), first.trim());
}

#[test]
fn a_public_bind_without_tls_is_refused_with_a_way_forward() {
    let dir = tempfile::tempdir().unwrap();
    let (code, _, err) = run(&[
        "--data-dir",
        &dir.path().display().to_string(),
        "--bind",
        "0.0.0.0",
        "--port",
        "0",
    ]);
    assert_ne!(code, 0, "it must not serve the network in clear text");
    let err = err.to_lowercase();
    assert!(
        err.contains("clear text") || err.contains("plain http"),
        "{err}"
    );
    // And it says what to do instead, rather than only refusing.
    assert!(err.contains("tls") || err.contains("https"), "{err}");
    assert!(err.contains("allow-insecure-http"), "{err}");
}

#[cfg(unix)]
#[test]
fn ctrl_c_stops_it_cleanly_and_it_tidies_up() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = Running::start(dir.path());
    let instance = dir.path().join("server.json");
    assert!(instance.exists());

    // SIGTERM is what a service manager sends, and Ctrl-C is the same path.
    let pid = server.child.id().to_string();
    let signalled = Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .expect("kill runs");
    assert!(signalled.success());

    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        match server.child.try_wait().unwrap() {
            Some(status) => break status,
            None if Instant::now() > deadline => panic!("the server did not stop within 20 s"),
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    assert!(
        status.success(),
        "a signalled shutdown is not a failure: {status}"
    );
    assert!(
        !instance.exists(),
        "server.json should be removed on the way out"
    );

    // The data directory is usable again: nothing is left locked.
    let second = Running::start(dir.path());
    assert!(second.url.starts_with("http://"));
}

#[test]
fn two_instances_cannot_share_a_data_directory() {
    let dir = tempfile::tempdir().unwrap();
    let _first = Running::start(dir.path());

    let (code, _, err) = run(&[
        "--data-dir",
        &dir.path().display().to_string(),
        "--port",
        "0",
    ]);
    assert_ne!(code, 0, "the second instance must refuse");
    assert!(
        err.to_lowercase().contains("already running"),
        "the refusal should say why: {err}"
    );
}
