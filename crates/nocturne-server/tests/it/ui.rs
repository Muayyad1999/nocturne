//! Serving the interface.
//!
//! The web files are the one part of the server that answers without a token,
//! so what it will and will not hand out matters. These tests use a small
//! fixture build in a temporary directory (`--ui-dir`), with a file next to
//! it that must never be reachable.

use crate::support::*;
use serde_json::Value;

/// The fixture UI, and the server serving it.
async fn with_ui() -> (tempfile::TempDir, TestServer) {
    let root = tempfile::tempdir().unwrap();
    let ui = write_ui_fixture(root.path());
    let server =
        TestServer::start_with(move |options| options.ui_dir = Some(ui), Default::default()).await;
    (root, server)
}

#[tokio::test]
async fn the_interface_is_served_without_a_token_with_the_right_types_and_caching() {
    let (_root, server) = with_ui().await;

    // The page itself: no token, correct type, and never cached — a stale
    // index would keep pointing at assets that are no longer there.
    let index = server.http.get(server.url("/")).send().await.unwrap();
    assert_eq!(index.status(), 200);
    let headers = index.headers().clone();
    let header = |name: &str| {
        headers
            .get(name)
            .map(|value| value.to_str().unwrap().to_string())
    };
    assert!(
        header("content-type").unwrap().starts_with("text/html"),
        "{headers:?}"
    );
    assert_eq!(header("cache-control").as_deref(), Some("no-cache"));
    assert_security_headers(header, "the page");
    assert_eq!(
        header("content-security-policy").as_deref(),
        Some(CONTENT_SECURITY_POLICY)
    );
    assert_eq!(index.text().await.unwrap(), INDEX_HTML);

    // A hashed asset is immutable: its name changes when its content does.
    let script = server
        .http
        .get(server.url("/assets/app-3f9a1c.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(script.status(), 200);
    let headers = script.headers().clone();
    let header = |name: &str| {
        headers
            .get(name)
            .map(|value| value.to_str().unwrap().to_string())
    };
    assert!(
        header("content-type").unwrap().contains("javascript"),
        "{headers:?}"
    );
    assert_eq!(
        header("cache-control").as_deref(),
        Some("public, max-age=31536000, immutable")
    );
    assert_security_headers(header, "an asset");
    assert_eq!(script.text().await.unwrap(), APP_JS);

    for (path, kind, body) in [
        ("/assets/app-77aa01.css", "text/css", APP_CSS),
        ("/favicon.svg", "image/svg+xml", FAVICON_SVG),
    ] {
        let response = server.http.get(server.url(path)).send().await.unwrap();
        assert_eq!(response.status(), 200, "{path}");
        let content_type = response.headers()["content-type"]
            .to_str()
            .unwrap()
            .to_string();
        assert!(content_type.starts_with(kind), "{path}: {content_type}");
        assert_eq!(response.text().await.unwrap(), body, "{path}");
    }

    // A binary file arrives intact, not re-encoded.
    let font = server
        .http
        .get(server.url("/assets/font-51c2.woff2"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        font.bytes().await.unwrap().to_vec(),
        (0..=255u8).collect::<Vec<_>>()
    );

    // A name with a space is a normal file name, not an excuse to fail.
    let notes = server
        .http
        .get(server.url("/release%20notes.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        (notes.status(), notes.text().await.unwrap()),
        (
            reqwest::StatusCode::OK,
            "spaces works".replace("works", "work")
        )
    );
}

#[tokio::test]
async fn a_deep_link_gets_the_page_but_an_unknown_api_path_does_not() {
    let (_root, server) = with_ui().await;

    // A top-level path is served in place: the page's asset URLs are
    // relative, and from one level down they still resolve.
    for path in ["/devices", "/nonsense"] {
        let response = server.http.get(server.url(path)).send().await.unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert_eq!(response.text().await.unwrap(), INDEX_HTML, "{path}");
    }

    // Deeper than that they would not, so the browser is sent to the top
    // instead of being handed a page whose scripts 404. Nothing is lost: the
    // interface routes on the fragment, so a nested path carries no state.
    let nested = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    for path in ["/devices/core-sw-1", "/settings/security"] {
        let response = nested.get(server.url(path)).send().await.unwrap();
        assert_eq!(response.status(), 302, "{path}");
        assert_eq!(
            response.headers()["location"].to_str().unwrap(),
            "/",
            "{path}"
        );
    }

    // A file that is missing is a missing file, not the page: a browser that
    // asked for JavaScript cannot do anything with HTML.
    for path in [
        "/assets/gone-00000.js",
        "/missing.css",
        "/assets/nothing.woff2",
    ] {
        let response = server.http.get(server.url(path)).send().await.unwrap();
        assert_eq!(response.status(), 404, "{path}");
    }

    // Under /api, a wrong path is a mistake and says so rather than quietly
    // returning a web page a client would try to parse as JSON.
    for path in ["/api/nope", "/api/rpc/extra", "/api/"] {
        let response = server.http.get(server.url(path)).send().await.unwrap();
        assert_eq!(response.status(), 404, "{path}");
        let body: Value = response.json().await.unwrap_or(Value::Null);
        assert_eq!(body["ok"], false, "{path}: {body}");
        assert!(body["error"]["code"].is_string(), "{path}: {body}");
    }
}

#[tokio::test]
async fn nothing_outside_the_interface_directory_can_be_reached() {
    let (root, server) = with_ui().await;
    assert!(root.path().join("secret.txt").exists());

    // Encoded and unencoded traversal, backslashes, absolute paths, and the
    // tricks that work on a server that resolves the path before checking it.
    let attempts = [
        "/../secret.txt",
        "/..%2fsecret.txt",
        "/%2e%2e/secret.txt",
        "/%2e%2e%2fsecret.txt",
        "/assets/../../secret.txt",
        "/assets/..%5c..%5csecret.txt",
        "/....//secret.txt",
        "/%252e%252e/secret.txt",
        "/./../secret.txt",
    ];
    for path in attempts {
        let response = server.http.get(server.url(path)).send().await.unwrap();
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        assert!(
            !body.contains("outside the ui directory"),
            "{path} reached a file outside the interface directory (status {status})"
        );
    }

    // The same through a raw request, so the client cannot normalise it first.
    for path in [
        "/../secret.txt",
        "/..%2f..%2fsecret.txt",
        "/assets/../../secret.txt",
    ] {
        let response = raw_get(server.addr, path, &[]).await;
        let body = response.text();
        assert!(
            !body.contains("outside the ui directory"),
            "{path} reached a file outside the interface directory (status {})",
            response.status
        );
    }
}

#[tokio::test]
async fn an_empty_interface_directory_does_not_stop_the_api() {
    // `--ui-dir` pointing somewhere empty: a mistake, but not a fatal one.
    let empty = tempfile::tempdir().unwrap();
    let path = empty.path().to_path_buf();
    let server = TestServer::start_with(
        move |options| options.ui_dir = Some(path),
        Default::default(),
    )
    .await;

    // Nothing is invented: an empty directory has no page, and the answer
    // says so rather than showing something that is not the interface.
    let response = server.http.get(server.url("/")).send().await.unwrap();
    assert_eq!(response.status(), 404);
    let body: Value = response.json().await.unwrap_or(Value::Null);
    assert_eq!(body["ok"], false);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("no file"),
        "the refusal should say what was not found: {body}"
    );

    // The API is unaffected by the missing interface, which is what matters:
    // `nocturne-server call` and any script keep working.
    let (status, answer) = server.rpc("devices.list", Value::Null).await;
    assert_eq!((status, &answer["ok"]), (200, &Value::Bool(true)));
    server.shutdown().await;
}

#[tokio::test]
async fn the_head_of_a_page_is_the_page_without_its_body() {
    let (_root, server) = with_ui().await;
    let response = server.http.head(server.url("/")).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers().get("content-type").is_some());
    assert!(
        response.bytes().await.unwrap().is_empty(),
        "a HEAD has no body"
    );
}
