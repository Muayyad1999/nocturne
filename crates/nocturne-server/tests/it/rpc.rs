//! `POST /api/rpc`, `/api/health` and `/api/info`.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::support::{TestServer, assert_security_headers, raw, raw_request, within};

const MIB: usize = 1024 * 1024;

#[tokio::test]
async fn successful_calls_and_application_errors_share_one_envelope() {
    let server = TestServer::start().await;

    let (status, reply) = server.rpc("devices.list", Value::Null).await;
    assert_eq!(status, 200);
    assert_eq!(reply, json!({"ok": true, "result": []}));

    let (status, reply) = server
        .rpc(
            "devices.create",
            json!({"name": "core-sw1", "address": "10.0.0.1"}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(reply["ok"], true);
    let created = &reply["result"];
    assert_eq!(created["name"], "core-sw1");
    assert_eq!(created["address"], "10.0.0.1");
    let id = created["id"].as_str().expect("a created device has an id");
    assert!(!id.is_empty());

    let listed = server.call("devices.list", Value::Null).await;
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], id);
    assert_eq!(listed[0]["name"], "core-sw1");

    // The application says no: still 200, because the request was delivered.
    let (status, reply) = server
        .rpc("devices.get", json!({"id": "no-such-device"}))
        .await;
    assert_eq!(status, 200);
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "not_found");
    let message = reply["error"]["message"].as_str().unwrap();
    assert!(message.contains("no-such-device"), "{message}");
    let fields: Vec<&str> = reply["error"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        ["code", "message"],
        "no `details` when there are none"
    );
    assert!(reply.get("result").is_none());

    let (status, reply) = server.rpc("devices.get", json!({"id": 5})).await;
    assert_eq!((status, &reply["error"]["code"]), (200, &json!("invalid")));

    server.shutdown().await;
}

#[tokio::test]
async fn an_unknown_method_is_an_application_error() {
    let server = TestServer::start().await;
    let (status, reply) = server.rpc("devices.teleport", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "unknown_method");
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("devices.teleport")
    );
    server.shutdown().await;
}

#[tokio::test]
async fn params_are_optional_and_the_content_type_is_not_policed() {
    let server = TestServer::start().await;
    let post = |body: &'static str| {
        server
            .http
            .post(server.url("/api/rpc"))
            .bearer_auth(&server.token)
            .body(body)
    };

    // No `params` at all, and no Content-Type (what `curl -d` without -H comes close to).
    let response = post(r#"{"method": "devices.list"}"#).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"ok": true, "result": []})
    );

    let response = post(r#"{"method": "test.echo", "params": [1, "two", null], "id": 7}"#)
        .header("content-type", "text/plain")
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"ok": true, "result": [1, "two", null]})
    );
    server.shutdown().await;
}

#[tokio::test]
async fn malformed_bodies_are_400_with_the_envelope() {
    let server = TestServer::start().await;
    let bodies = [
        "",
        "{",
        "this is not json",
        r#"{"method": "devices.list"} trailing"#,
        "[]",
        r#""devices.list""#,
        "{}",
        r#"{"params": {}}"#,
        r#"{"method": ""}"#,
        r#"{"method": 42}"#,
        r#"{"method": null, "params": {}}"#,
    ];
    for body in bodies {
        let response = server
            .http
            .post(server.url("/api/rpc"))
            .bearer_auth(&server.token)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{body:?}");
        let reply: Value = response.json().await.unwrap();
        assert_eq!(reply["ok"], false, "{body:?}");
        assert_eq!(reply["error"]["code"], "invalid", "{body:?}");
        assert!(
            !reply["error"]["message"].as_str().unwrap().is_empty(),
            "{body:?} needs an explanation"
        );
    }
    // The server is none the worse for it.
    assert_eq!(server.call("devices.list", Value::Null).await, json!([]));
    server.shutdown().await;
}

/// A request body of exactly `size` bytes that is a valid call of `test.echo`.
fn echo_body_of(size: usize) -> String {
    let frame = r#"{"method":"test.echo","params":{"pad":""}}"#;
    let pad = "a".repeat(size - frame.len());
    let body = format!(r#"{{"method":"test.echo","params":{{"pad":"{pad}"}}}}"#);
    assert_eq!(body.len(), size);
    body
}

#[tokio::test]
async fn bodies_up_to_16_mib_are_accepted_and_larger_ones_are_413() {
    let server = TestServer::start().await;
    let send = |body: String| {
        server
            .http
            .post(server.url("/api/rpc"))
            .bearer_auth(&server.token)
            .header("content-type", "application/json")
            .body(body)
            .send()
    };

    let response = within("the 16 MiB call", send(echo_body_of(16 * MIB)))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let reply: Value = response.json().await.unwrap();
    assert_eq!(reply["ok"], true);
    let frame_len = r#"{"method":"test.echo","params":{"pad":""}}"#.len();
    assert_eq!(
        reply["result"]["pad"].as_str().unwrap().len(),
        16 * MIB - frame_len,
        "the whole body arrived"
    );

    let response = within("the oversized call", send(echo_body_of(16 * MIB + 1)))
        .await
        .unwrap();
    assert_eq!(response.status(), 413);
    let reply: Value = response.json().await.unwrap();
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "invalid");
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("16 MiB")
    );

    // Without a declared length the limit is enforced while reading.
    let chunks = (0..17).map(|_| Ok::<_, std::io::Error>(vec![b' '; MIB]));
    let response = within(
        "the oversized streamed call",
        server
            .http
            .post(server.url("/api/rpc"))
            .bearer_auth(&server.token)
            .body(reqwest::Body::wrap_stream(futures::stream::iter(chunks)))
            .send(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), 413);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"]["code"],
        "invalid"
    );

    assert_eq!(server.call("devices.list", Value::Null).await, json!([]));
    server.shutdown().await;
}

#[tokio::test]
async fn a_panicking_method_is_a_500_and_the_server_keeps_serving() {
    let server = TestServer::start().await;
    let (status, reply) = server.rpc("test.panic", Value::Null).await;
    assert_eq!(status, 500);
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "internal");
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("test.panic")
    );

    assert_eq!(server.call("devices.list", Value::Null).await, json!([]));
    assert_eq!(
        server.call("test.panic", json!({"really": false})).await,
        Value::Null
    );
    server.shutdown().await;
}

#[tokio::test]
async fn a_call_runs_to_completion_even_if_the_client_hangs_up() {
    let server = TestServer::start().await;
    let mut events = server.app.subscribe();

    let body =
        r#"{"method": "test.sleep", "params": {"ms": 300, "then_publish": "test.finished"}}"#;
    let request = raw_request(
        "POST",
        "/api/rpc",
        &[
            ("Host", &server.host()),
            ("Authorization", &format!("Bearer {}", server.token)),
        ],
        body,
    );
    let mut stream = TcpStream::connect(server.addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    // Give the request time to arrive, then walk away before the call is done.
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(stream);

    let finished = within("the abandoned call to finish", async {
        loop {
            let event = events.recv().await.unwrap();
            if event.topic == "test.finished" {
                return event;
            }
        }
    })
    .await;
    assert_eq!(finished.payload, json!({"slept_ms": 300}));
    server.shutdown().await;
}

#[tokio::test]
async fn only_post_reaches_the_rpc_endpoint() {
    let server = TestServer::start().await;
    for method in [
        reqwest::Method::GET,
        reqwest::Method::PUT,
        reqwest::Method::DELETE,
    ] {
        let response = server
            .http
            .request(method.clone(), server.url("/api/rpc"))
            .bearer_auth(&server.token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 405, "{method}");
        assert_eq!(response.headers()["allow"], "POST", "{method}");
        let reply: Value = response.json().await.unwrap();
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["code"], "invalid");
    }
    server.shutdown().await;
}

#[tokio::test]
async fn health_needs_no_token_and_says_nothing_about_the_data() {
    let server = TestServer::start().await;
    server
        .call(
            "devices.create",
            json!({"name": "sw1", "address": "10.0.0.1"}),
        )
        .await;

    let response = server
        .http
        .get(server.url("/api/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_security_headers(
        |name| {
            response
                .headers()
                .get(name)
                .map(|value| value.to_str().unwrap().to_string())
        },
        "/api/health",
    );
    let health: Value = response.json().await.unwrap();
    // The package version, as cargo knows it.
    assert_eq!(
        health,
        json!({"status": "ok", "version": env!("CARGO_PKG_VERSION")})
    );

    let head = server
        .http
        .head(server.url("/api/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), 200);
    assert_eq!(head.bytes().await.unwrap().len(), 0);

    let post = server
        .http
        .post(server.url("/api/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(post.status(), 405);
    server.shutdown().await;
}

#[tokio::test]
async fn info_lists_the_version_and_every_method() {
    let server = TestServer::start().await;
    let response = server
        .http
        .get(server.url("/api/info"))
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let info: Value = response.json().await.unwrap();
    let fields: Vec<&str> = info
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(fields, ["version", "methods"]);
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));

    // Straight from the application, not through the server.
    let expected: Vec<Value> = server.app.methods().into_iter().map(Value::from).collect();
    assert_eq!(info["methods"], Value::Array(expected));
    let methods = info["methods"].as_array().unwrap();
    for known in ["devices.list", "devices.create", "devices.get", "test.echo"] {
        assert!(methods.contains(&json!(known)), "{known} is missing");
    }
    server.shutdown().await;
}

#[tokio::test]
async fn unknown_api_paths_are_404_not_the_web_page() {
    let server = TestServer::start().await;
    for path in [
        "/api",
        "/api/",
        "/api/nope",
        "/api/rpc/extra",
        "/api/v2/rpc",
    ] {
        let response = raw(
            server.addr,
            raw_request("GET", path, &[("Host", &server.host())], ""),
        )
        .await;
        assert_eq!(response.status, 404, "{path}");
        assert_eq!(
            response.header("content-type"),
            Some("application/json"),
            "{path}"
        );
        let reply = response.json();
        assert_eq!(reply["ok"], false, "{path}");
        assert_eq!(reply["error"]["code"], "not_found", "{path}");
    }
    server.shutdown().await;
}
