//! DNS-rebinding and cross-site protection: the `Host` and `Origin` checks,
//! and the absence of any CORS header.

use nocturne_server::Timing;
use serde_json::json;

use crate::support::{RawResponse, TestServer, raw, raw_get, raw_request, refusal, ws_connect};

fn assert_forbidden(response: &RawResponse, context: &str) {
    assert_eq!(response.status, 403, "{context}");
    let reply = response.json();
    assert_eq!(reply["ok"], false, "{context}");
    assert_eq!(reply["error"]["code"], "forbidden", "{context}");
    assert!(
        !reply["error"]["message"].as_str().unwrap().is_empty(),
        "{context}"
    );
}

fn assert_no_cors(response: &RawResponse, context: &str) {
    let cors: Vec<_> = response
        .headers
        .iter()
        .filter(|(name, _)| name.starts_with("access-control-"))
        .collect();
    assert!(cors.is_empty(), "{context}: {cors:?}");
}

#[tokio::test]
async fn a_loopback_server_only_answers_to_loopback_names_with_its_port() {
    let server = TestServer::start().await;
    let port = server.addr.port();

    let accepted = [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("LocalHost:{port}"),
        format!("[::1]:{port}"),
    ];
    for host in &accepted {
        for path in ["/api/health", "/"] {
            let response = raw_get(server.addr, path, &[("Host", host)]).await;
            assert_eq!(response.status, 200, "Host: {host} on {path}");
        }
    }

    let refused = [
        format!("evil.example:{port}"),
        "evil.example".to_string(),
        format!("localhost.evil.example:{port}"),
        format!("127.0.0.1.evil.example:{port}"),
        format!("192.0.2.10:{port}"),
        // The right name with the wrong port, or no port at all (so: 80).
        format!("localhost:{}", port.wrapping_add(1).max(1)),
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ];
    for host in &refused {
        // Every route, including the ones that need no token.
        for path in [
            "/api/health",
            "/",
            "/assets/app.js",
            "/api/rpc",
            "/api/events",
        ] {
            let response = raw_get(server.addr, path, &[("Host", host)]).await;
            assert_forbidden(&response, &format!("Host: {host} on {path}"));
            assert_no_cors(&response, host);
        }
    }

    // No Host at all (HTTP/1.0 style).
    let response = raw(server.addr, "GET /api/health HTTP/1.0\r\n\r\n").await;
    assert_forbidden(&response, "no Host header");

    // The refusal says what would have worked.
    let response = raw_get(server.addr, "/", &[("Host", "evil.example")]).await;
    let message = response.json()["error"]["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(message.contains("evil.example"), "{message}");
    assert!(
        message.contains(&format!("http://127.0.0.1:{port}")),
        "{message}"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn a_valid_token_does_not_make_up_for_a_foreign_host() {
    let server = TestServer::start().await;
    let authorization = format!("Bearer {}", server.token);
    let request = raw_request(
        "POST",
        "/api/rpc",
        &[
            ("Host", "evil.example"),
            ("Authorization", &authorization),
            ("Content-Type", "application/json"),
        ],
        r#"{"method": "devices.list"}"#,
    );
    assert_forbidden(&raw(server.addr, request).await, "token + foreign host");
    server.shutdown().await;
}

#[tokio::test]
async fn api_requests_from_another_origin_are_refused() {
    let server = TestServer::start().await;
    let host = server.host();
    let port = server.addr.port();
    let authorization = format!("Bearer {}", server.token);
    let call = |origin: Option<&str>| {
        let mut headers = vec![
            ("Host", host.as_str()),
            ("Authorization", authorization.as_str()),
            ("Content-Type", "application/json"),
        ];
        if let Some(origin) = origin {
            headers.push(("Origin", origin));
        }
        raw(
            server.addr,
            raw_request(
                "POST",
                "/api/rpc",
                &headers,
                r#"{"method": "devices.list"}"#,
            ),
        )
    };

    // No Origin (curl, scripts) and the server's own origin are fine.
    let response = call(None).await;
    assert_eq!(response.status, 200);
    assert_eq!(response.json(), json!({"ok": true, "result": []}));
    assert_no_cors(&response, "no origin");
    let own = format!("http://{host}");
    let response = call(Some(&own)).await;
    assert_eq!(response.status, 200, "{own}");
    assert_no_cors(&response, "same origin");

    let foreign = [
        "http://evil.example".to_string(),
        format!("http://evil.example:{port}"),
        // Same host, other scheme or port.
        format!("https://{host}"),
        format!("http://127.0.0.1:{}", port.wrapping_add(1).max(1)),
        // Another name for the same machine is still another origin.
        format!("http://localhost:{port}"),
        "null".to_string(),
    ];
    for origin in &foreign {
        let response = call(Some(origin)).await;
        assert_forbidden(&response, &format!("Origin: {origin}"));
        assert_no_cors(&response, origin);
    }

    // The page was loaded via `localhost`: then that is its origin, and it matches.
    let localhost = format!("localhost:{port}");
    let response = raw(
        server.addr,
        raw_request(
            "POST",
            "/api/rpc",
            &[
                ("Host", &localhost),
                ("Authorization", &authorization),
                ("Origin", &format!("http://{localhost}")),
            ],
            r#"{"method": "devices.list"}"#,
        ),
    )
    .await;
    assert_eq!(response.status, 200);
    server.shutdown().await;
}

#[tokio::test]
async fn origin_applies_to_the_whole_api_and_only_to_the_api() {
    let server = TestServer::start().await;
    let host = server.host();
    let foreign = [("Host", host.as_str()), ("Origin", "http://evil.example")];

    for path in [
        "/api/health",
        "/api/info",
        "/api/rpc",
        "/api/events",
        "/api/nope",
    ] {
        assert_forbidden(&raw_get(server.addr, path, &foreign).await, path);
    }
    // A link from another site may open the UI; it just cannot call anything.
    for path in ["/", "/index.html", "/devices"] {
        assert_eq!(
            raw_get(server.addr, path, &foreign).await.status,
            200,
            "{path}"
        );
    }
    server.shutdown().await;
}

#[tokio::test]
async fn preflight_requests_get_no_cors_headers() {
    let server = TestServer::start().await;
    let host = server.host();
    let preflight = raw_request(
        "OPTIONS",
        "/api/rpc",
        &[
            ("Host", &host),
            ("Origin", "http://evil.example"),
            ("Access-Control-Request-Method", "POST"),
            (
                "Access-Control-Request-Headers",
                "authorization, content-type",
            ),
        ],
        "",
    );
    let response = raw(server.addr, preflight).await;
    assert_forbidden(&response, "preflight");
    assert_no_cors(&response, "preflight");

    // Even a same-origin OPTIONS learns nothing that would enable cross-origin use.
    let own = format!("http://{host}");
    let response = raw(
        server.addr,
        raw_request(
            "OPTIONS",
            "/api/health",
            &[("Host", &host), ("Origin", &own)],
            "",
        ),
    )
    .await;
    assert_eq!(response.status, 405);
    assert_no_cors(&response, "same-origin OPTIONS");
    server.shutdown().await;
}

#[tokio::test]
async fn a_websocket_from_another_origin_is_refused() {
    let server = TestServer::start().await;
    let url = server.ws_url(&format!("/api/events?token={}", server.token));

    let (status, headers, body) =
        refusal(ws_connect(&url, &[("origin", "http://evil.example")]).await);
    assert_eq!(status, 403);
    assert_eq!(body["error"]["code"], "forbidden");
    assert!(
        !headers
            .keys()
            .any(|name| name.as_str().starts_with("access-control-"))
    );

    let own = format!("http://{}", server.host());
    ws_connect(&url, &[("origin", &own)])
        .await
        .expect("the server's own page may open the stream");
    server.shutdown().await;
}

#[tokio::test]
async fn a_network_facing_server_accepts_any_host_but_still_checks_the_origin() {
    let server = TestServer::start_with(
        |options| {
            options.bind = "0.0.0.0:0".parse().unwrap();
            options.allow_insecure_http = true;
        },
        Timing::default(),
    )
    .await;
    let port = server.addr.port();
    let reach = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let authorization = format!("Bearer {}", server.token);
    let noc = format!("noc.example:{port}");
    let call = |origin: &str| {
        raw(
            reach,
            raw_request(
                "POST",
                "/api/rpc",
                &[
                    ("Host", &noc),
                    ("Authorization", &authorization),
                    ("Origin", origin),
                ],
                r#"{"method": "devices.list"}"#,
            ),
        )
    };

    // Whatever name the machine is known by on the network.
    assert_eq!(
        raw_get(reach, "/api/health", &[("Host", &noc)])
            .await
            .status,
        200
    );
    assert_eq!(call(&format!("http://{noc}")).await.status, 200);
    assert_forbidden(&call("http://evil.example").await, "foreign origin");
    assert_forbidden(&call(&format!("https://{noc}")).await, "other scheme");

    // It listens everywhere, but the link it hands out is one that works from here.
    assert_eq!(
        server.handle().base_url(),
        format!("http://127.0.0.1:{port}/")
    );
    server.shutdown().await;
}
