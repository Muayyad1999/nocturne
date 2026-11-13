//! The TFTP server against real clients: `curl` and tftp-hpa's `tftp`.

mod support;

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use nocturne_netprobe::tftp::{TftpConfig, TftpEvent, TftpServer};
use tokio::sync::mpsc::UnboundedReceiver;

async fn start(
    root: &Path,
    allow_overwrite: bool,
) -> (TftpServer, u16, UnboundedReceiver<TftpEvent>) {
    let config = TftpConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        root: root.to_path_buf(),
        allow_read: true,
        allow_write: true,
        allow_overwrite,
        timeout_ms: 2000,
        retries: 3,
        max_file_bytes: 64 * 1024 * 1024,
    };
    let (server, events) = TftpServer::start(config).await.unwrap();
    let port = server.local_addr().port();
    (server, port, events)
}

fn pattern(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| (index % 253) as u8 ^ (index / 253) as u8)
        .collect()
}

/// Runs a client program off the async threads and returns whether it succeeded.
async fn run(mut command: Command) -> (bool, String) {
    tokio::task::spawn_blocking(move || {
        let output = command.output().expect("the client program runs");
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        (output.status.success(), said)
    })
    .await
    .unwrap()
}

/// Waits for the outcome (Finished or Failed) of the next transfer.
async fn outcome(events: &mut UnboundedReceiver<TftpEvent>) -> TftpEvent {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(20), events.recv())
            .await
            .unwrap()
            .unwrap();
        if matches!(
            event,
            TftpEvent::Finished { .. } | TftpEvent::Failed { .. } | TftpEvent::Denied { .. }
        ) {
            return event;
        }
    }
}

fn curl_speaks_tftp() -> bool {
    let Some(curl) = support::require_program("curl") else {
        return false;
    };
    let version = Command::new(curl)
        .arg("--version")
        .output()
        .expect("curl runs");
    let supported = String::from_utf8_lossy(&version.stdout).contains("tftp");
    if !supported {
        eprintln!("skipping: this curl was built without TFTP");
    }
    supported
}

fn curl(args: &[&str]) -> Command {
    let mut command = Command::new("curl");
    command
        .args(["--silent", "--show-error", "--max-time", "30"])
        .args(args);
    command
}

#[tokio::test]
async fn curl_downloads() {
    if !curl_speaks_tftp() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("images")).unwrap();
    let (_server, port, mut events) = start(dir.path(), false).await;

    for (name, size, extra) in [
        ("running-config", 4321usize, vec![]),
        ("images/firmware.bin", 300_000, vec![]),
        // curl rejects "tsize=0" from any server, so an empty file is fetched without options.
        ("empty", 0, vec!["--tftp-no-options"]),
        ("block-multiple.bin", 2048, vec![]),
        ("big-blocks.bin", 200_000, vec!["--tftp-blksize", "8192"]),
        ("no-options.bin", 70_000, vec!["--tftp-no-options"]),
    ] {
        std::fs::write(dir.path().join(name), pattern(size)).unwrap();
        let target = out.path().join("download");
        let url = format!("tftp://127.0.0.1:{port}/{name}");
        let mut args = vec!["--output", target.to_str().unwrap()];
        args.extend(extra);
        args.push(&url);
        let (ok, said) = run(curl(&args)).await;
        assert!(ok, "curl {name}: {said}");
        // curl creates no file for an empty transfer.
        let downloaded = std::fs::read(&target).unwrap_or_default();
        assert!(
            downloaded == pattern(size),
            "{name}: curl received {} bytes, expected {size}",
            downloaded.len()
        );
        let _ = std::fs::remove_file(&target);
        let finished = outcome(&mut events).await;
        assert!(
            matches!(finished, TftpEvent::Finished { bytes, .. } if bytes == size as u64),
            "{name}: {finished:?}"
        );
    }

    // A file that is not there: curl fails, and nothing is downloaded.
    let target = out.path().join("missing");
    let url = format!("tftp://127.0.0.1:{port}/no-such-file");
    let (ok, said) = run(curl(&["--output", target.to_str().unwrap(), &url])).await;
    assert!(!ok, "curl should fail: {said}");
    assert!(said.to_lowercase().contains("not found"), "{said}");
    assert!(matches!(
        outcome(&mut events).await,
        TftpEvent::Denied { .. }
    ));
}

#[tokio::test]
async fn curl_uploads() {
    if !curl_speaks_tftp() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let (_server, port, mut events) = start(dir.path(), false).await;

    for (name, size, extra) in [
        ("sw1-backup.cfg", 9876usize, vec![]),
        ("big-upload.bin", 250_000, vec!["--tftp-blksize", "4096"]),
        ("plain-upload.bin", 33_000, vec!["--tftp-no-options"]),
    ] {
        let local = source.path().join("upload");
        std::fs::write(&local, pattern(size)).unwrap();
        let url = format!("tftp://127.0.0.1:{port}/{name}");
        let mut args = vec!["--upload-file", local.to_str().unwrap()];
        args.extend(extra);
        args.push(&url);
        let (ok, said) = run(curl(&args)).await;
        assert!(ok, "curl -T {name}: {said}");
        let finished = outcome(&mut events).await;
        assert!(
            matches!(finished, TftpEvent::Finished { bytes, .. } if bytes == size as u64),
            "{name}: {finished:?}"
        );
        assert!(
            std::fs::read(dir.path().join(name)).unwrap() == pattern(size),
            "{name}: stored bytes differ"
        );
    }

    // The file exists and overwriting is off: curl is told so and the file is untouched.
    let local = source.path().join("upload");
    std::fs::write(&local, b"replacement").unwrap();
    let url = format!("tftp://127.0.0.1:{port}/sw1-backup.cfg");
    let (ok, said) = run(curl(&["--upload-file", local.to_str().unwrap(), &url])).await;
    assert!(!ok, "curl should fail: {said}");
    assert!(matches!(
        outcome(&mut events).await,
        TftpEvent::Denied { .. }
    ));
    assert!(std::fs::read(dir.path().join("sw1-backup.cfg")).unwrap() == pattern(9876));
}

#[tokio::test]
async fn tftp_hpa_downloads_and_uploads() {
    let Some(tftp) = support::require_program("tftp") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("startup-config"), pattern(12_345)).unwrap();
    let (_server, port, mut events) = start(dir.path(), true).await;
    let client = |command: String| {
        let mut tftp = Command::new(&tftp);
        tftp.args(["-m", "binary", "127.0.0.1", &port.to_string(), "-c"]);
        tftp.args(command.split(' '));
        tftp.current_dir(work.path());
        tftp
    };

    let (ok, said) = run(client("get startup-config downloaded".to_owned())).await;
    assert!(
        ok && !said.to_lowercase().contains("error"),
        "tftp get: {said}"
    );
    assert!(
        std::fs::read(work.path().join("downloaded")).unwrap() == pattern(12_345),
        "downloaded bytes differ"
    );
    assert!(matches!(
        outcome(&mut events).await,
        TftpEvent::Finished { bytes: 12_345, .. }
    ));

    std::fs::write(work.path().join("to-upload"), pattern(54_321)).unwrap();
    let (ok, said) = run(client("put to-upload uploaded.bin".to_owned())).await;
    assert!(
        ok && !said.to_lowercase().contains("error"),
        "tftp put: {said}"
    );
    assert!(matches!(
        outcome(&mut events).await,
        TftpEvent::Finished { bytes: 54_321, .. }
    ));
    assert!(
        std::fs::read(dir.path().join("uploaded.bin")).unwrap() == pattern(54_321),
        "uploaded bytes differ"
    );

    // Replacing a file is allowed on this server.
    std::fs::write(work.path().join("to-upload"), pattern(777)).unwrap();
    let (ok, said) = run(client("put to-upload uploaded.bin".to_owned())).await;
    assert!(
        ok && !said.to_lowercase().contains("error"),
        "tftp put again: {said}"
    );
    assert!(matches!(
        outcome(&mut events).await,
        TftpEvent::Finished { bytes: 777, .. }
    ));
    assert!(std::fs::read(dir.path().join("uploaded.bin")).unwrap() == pattern(777));

    // tftp-hpa prints the server's error and exits; the reason must reach the user.
    let (_ok, said) = run(client("get ../etc/passwd stolen".to_owned())).await;
    assert!(said.contains("'..' is not allowed"), "{said}");
    assert!(matches!(
        outcome(&mut events).await,
        TftpEvent::Denied { .. }
    ));
    assert!(
        std::fs::read(work.path().join("stolen"))
            .map(|bytes| bytes.is_empty())
            .unwrap_or(true)
    );
}
