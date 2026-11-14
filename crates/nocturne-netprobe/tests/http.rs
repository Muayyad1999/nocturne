//! The HTTP check against real servers: Python's `http.server` (plain HTTP) and
//! `openssl s_server -www` (HTTPS), cross-checked with `curl`.

mod support;

use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nocturne_netprobe::ProbeError;
use nocturne_netprobe::http::{self, HttpOptions, HttpReport};

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Retries until the freshly started server accepts connections.
async fn check_when_up(url: &str, options: &HttpOptions) -> Option<HttpReport> {
    for _ in 0..50 {
        match http::check(url, options, unix_now()).await {
            Ok(report) => return Some(report),
            Err(ProbeError::Refused) => tokio::time::sleep(Duration::from_millis(100)).await,
            Err(error) => panic!("checking {url} failed: {error}"),
        }
    }
    eprintln!("skipping: the test server for {url} did not start");
    None
}

/// What curl makes of the same URL: status code and downloaded size.
fn curl(url: &str, extra: &[&str]) -> Option<(u16, u64)> {
    let output = Command::new("curl")
        .args([
            "-s",
            "-o",
            "/dev/null",
            "--noproxy",
            "*",
            "-w",
            "%{http_code} %{size_download}",
        ])
        .args(extra)
        .arg(url)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let (status, size) = text.trim().split_once(' ')?;
    Some((status.parse().ok()?, size.parse().ok()?))
}

#[tokio::test]
async fn python_http_server_and_curl_agree_with_the_check() {
    let Some(python) = support::require_program("python3") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let page = "<html><body>running-config of sw1</body></html>\n".repeat(200);
    std::fs::write(dir.path().join("index.html"), &page).unwrap();
    std::fs::write(dir.path().join("firmware.bin"), vec![0u8; 100_000]).unwrap();
    let port = support::free_tcp_port();
    let mut command = Command::new(python);
    command
        .args([
            "-m",
            "http.server",
            &port.to_string(),
            "--bind",
            "127.0.0.1",
            "--directory",
        ])
        .arg(dir.path());
    let _server = support::Helper::spawn(&mut command).unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let options = HttpOptions::default();

    let Some(index) = check_when_up(&format!("{base}/index.html"), &options).await else {
        return;
    };
    assert_eq!(index.status, 200);
    assert_eq!(index.version, "HTTP/1.0");
    assert_eq!(index.body_bytes, page.len() as u64);
    assert!(
        index
            .body_preview
            .starts_with("<html><body>running-config of sw1")
    );
    let server = index
        .headers
        .iter()
        .find(|(name, _)| name == "Server")
        .map(|(_, value)| value.as_str());
    // `SimpleHTTP`, with nothing assumed after it. Python used to send
    // `SimpleHTTP/0.6 Python/3.11.2` and 3.15 sends `SimpleHTTP Python/3.15.2`
    // — the slash and the handler version are gone. What this test is for is
    // that the header the server sent is the header we parsed, not which
    // version string Python felt like using.
    assert!(
        server.is_some_and(|server| server.starts_with("SimpleHTTP")),
        "{:?}",
        index.headers
    );
    let length = index
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .unwrap();
    assert_eq!(length.1, page.len().to_string());

    let binary = http::check(&format!("{base}/firmware.bin"), &options, unix_now())
        .await
        .unwrap();
    assert_eq!((binary.status, binary.body_bytes), (200, 100_000));
    assert_eq!(binary.body_preview, "", "binary content has no preview");

    let missing = http::check(&format!("{base}/no-such-file"), &options, unix_now())
        .await
        .unwrap();
    assert_eq!(missing.status, 404);
    assert_eq!(missing.reason, "File not found");

    // http.server redirects a directory without trailing slash.
    std::fs::create_dir(dir.path().join("configs")).unwrap();
    std::fs::write(dir.path().join("configs/index.html"), "configs\n").unwrap();
    let redirected = http::check(&format!("{base}/configs"), &options, unix_now())
        .await
        .unwrap();
    assert_eq!(redirected.status, 200);
    assert_eq!(
        redirected.redirects,
        vec![(301, format!("{base}/configs/"))]
    );
    assert_eq!(redirected.final_url, format!("{base}/configs/"));
    assert_eq!(redirected.body_preview, "configs\n");

    let head = HttpOptions {
        method: "HEAD".to_owned(),
        ..HttpOptions::default()
    };
    let headers_only = http::check(&format!("{base}/firmware.bin"), &head, unix_now())
        .await
        .unwrap();
    assert_eq!((headers_only.status, headers_only.body_bytes), (200, 0));

    if support::require_program("curl").is_some() {
        for (path, ours) in [
            ("/index.html", &index),
            ("/firmware.bin", &binary),
            ("/no-such-file", &missing),
        ] {
            let Some((status, size)) = curl(&format!("{base}{path}"), &[]) else {
                continue;
            };
            assert_eq!(
                (ours.status, ours.body_bytes),
                (status, size),
                "{path}: curl disagrees"
            );
        }
        let (status, size) = curl(&format!("{base}/configs"), &["-L"]).unwrap();
        assert_eq!((redirected.status, redirected.body_bytes), (status, size));
    }
}

#[tokio::test]
async fn https_against_openssl_s_server() {
    if support::require_program("openssl").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("key.pem").display().to_string();
    let cert = dir.path().join("cert.pem").display().to_string();
    let made = Command::new("openssl")
        .args([
            "req", "-x509", "-nodes", "-newkey", "rsa:2048", "-days", "30", "-keyout", &key,
            "-out", &cert,
        ])
        .args([
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=DNS:localhost",
        ])
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let port = support::free_tcp_port();
    let mut command = Command::new("openssl");
    // -www: answer every request with a status page.
    command.args([
        "s_server",
        "-accept",
        &format!("127.0.0.1:{port}"),
        "-key",
        &key,
        "-cert",
        &cert,
        "-www",
        "-quiet",
    ]);
    let _server = support::Helper::spawn(&mut command).unwrap();
    let url = format!("https://localhost:{port}/");

    let lenient = HttpOptions {
        verify_tls: false,
        ..HttpOptions::default()
    };
    let Some(report) = check_when_up(&url, &lenient).await else {
        return;
    };
    assert_eq!(report.status, 200);
    assert!(
        report.body_bytes > 100,
        "s_server -www prints its session parameters"
    );
    assert!(
        report.body_preview.contains("s_server"),
        "{}",
        report.body_preview
    );
    let tls = report.tls.as_ref().unwrap();
    assert_eq!(tls.protocol, "TLSv1.3");
    assert!(!tls.trusted && tls.hostname_matches);
    assert_eq!(tls.chain[0].common_name, "localhost");
    assert_eq!(tls.chain[0].public_key, "RSA 2048");
    assert!(report.tls_ms.is_some_and(|ms| ms > 0.0));

    // With verification on, a self-signed certificate stops the check before any request is sent.
    let error = http::check(&url, &HttpOptions::default(), unix_now())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, ProbeError::Protocol(text) if text.contains("certificate")),
        "{error:?}"
    );

    if support::require_program("curl").is_some() {
        // The page lists the cipher suites each client offered, so only the status compares.
        if let Some((status, _size)) = curl(&url, &["-k"]) {
            assert_eq!(report.status, status);
        }
    }
}
