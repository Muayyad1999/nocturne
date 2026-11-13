//! Who gets in: no token, a wrong token, the right token; in the header and,
//! for the event stream, in the query.

use serde_json::{Value, json};

use crate::support::{TestServer, next_event_on, refusal, ws_connect};

const WRONG: &str = "0000000000000000000000000000000000000000000000000000000000000000";

async fn assert_unauthorized(response: reqwest::Response, context: &str) {
    assert_eq!(response.status(), 401, "{context}");
    assert_eq!(
        response.headers()["www-authenticate"],
        "Bearer",
        "{context}"
    );
    assert_eq!(
        response.headers()["content-type"],
        "application/json",
        "{context}"
    );
    let reply: Value = response.json().await.unwrap();
    assert_eq!(reply["ok"], false, "{context}");
    assert_eq!(reply["error"]["code"], "unauthorized", "{context}");
    let message = reply["error"]["message"].as_str().unwrap();
    assert!(message.contains("token"), "{context}: {message}");
}

#[tokio::test]
async fn the_api_requires_the_token_in_the_authorization_header() {
    let server = TestServer::start().await;
    let rpc = || {
        server
            .http
            .post(server.url("/api/rpc"))
            .json(&json!({"method": "devices.list"}))
    };
    let info = || server.http.get(server.url("/api/info"));

    for (name, request) in [
        ("rpc", &rpc as &dyn Fn() -> reqwest::RequestBuilder),
        ("info", &info),
    ] {
        assert_unauthorized(
            request().send().await.unwrap(),
            &format!("{name}: no token"),
        )
        .await;
        assert_unauthorized(
            request().bearer_auth(WRONG).send().await.unwrap(),
            &format!("{name}: wrong token"),
        )
        .await;
        // Close is not good enough.
        let truncated = &server.token[..server.token.len() - 1];
        for almost in [
            truncated.to_string(),
            format!("{}0", server.token),
            server.token.to_uppercase(),
            String::new(),
        ] {
            assert_unauthorized(
                request().bearer_auth(&almost).send().await.unwrap(),
                &format!("{name}: {almost:?}"),
            )
            .await;
        }
        // Other schemes, or no scheme at all.
        for header in [
            format!("Basic {}", server.token),
            server.token.clone(),
            format!("Token {}", server.token),
        ] {
            assert_unauthorized(
                request()
                    .header("authorization", &header)
                    .send()
                    .await
                    .unwrap(),
                &format!("{name}: {header:?}"),
            )
            .await;
        }

        let granted = request().bearer_auth(&server.token).send().await.unwrap();
        assert_eq!(granted.status(), 200, "{name}: right token");
        // The scheme name is case-insensitive.
        let lower = request()
            .header("authorization", format!("bearer {}", server.token))
            .send()
            .await
            .unwrap();
        assert_eq!(lower.status(), 200, "{name}: lower-case scheme");
    }
    server.shutdown().await;
}

#[tokio::test]
async fn a_token_in_the_query_only_opens_the_event_stream() {
    let server = TestServer::start().await;
    let with_query = format!("?token={}", server.token);

    let rpc = server
        .http
        .post(server.url(&format!("/api/rpc{with_query}")))
        .json(&json!({"method": "devices.list"}))
        .send()
        .await
        .unwrap();
    assert_unauthorized(rpc, "rpc with the token in the query").await;
    let info = server
        .http
        .get(server.url(&format!("/api/info{with_query}")))
        .send()
        .await
        .unwrap();
    assert_unauthorized(info, "info with the token in the query").await;
    server.shutdown().await;
}

#[tokio::test]
async fn health_and_the_web_ui_need_no_token() {
    let server = TestServer::start().await;
    for path in ["/api/health", "/", "/index.html"] {
        let response = server.http.get(server.url(path)).send().await.unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert!(
            response.headers().get("www-authenticate").is_none(),
            "{path}"
        );
    }
    server.shutdown().await;
}

#[tokio::test]
async fn the_event_stream_takes_the_token_from_the_query_or_the_header() {
    let server = TestServer::start().await;
    let events = server.ws_url("/api/events");
    let right_header = format!("Bearer {}", server.token);
    let wrong_header = format!("Bearer {WRONG}");

    // Refused: nothing, a wrong query, a wrong header, both wrong.
    let refused: [(String, Vec<(&str, &str)>); 5] = [
        (events.clone(), vec![]),
        (format!("{events}?token={WRONG}"), vec![]),
        (format!("{events}?token="), vec![]),
        (events.clone(), vec![("authorization", &wrong_header)]),
        (
            format!("{events}?token={WRONG}"),
            vec![("authorization", &wrong_header)],
        ),
    ];
    for (url, headers) in &refused {
        let (status, response_headers, body) = refusal(ws_connect(url, headers).await);
        assert_eq!(status, 401, "{url} {headers:?}");
        assert_eq!(response_headers["www-authenticate"], "Bearer", "{url}");
        assert_eq!(body["error"]["code"], "unauthorized", "{url}");
    }

    // Accepted: the query (browsers), the header (everything else), and
    // either one being right when both are present.
    let accepted: [(String, Vec<(&str, &str)>); 4] = [
        (format!("{events}?token={}", server.token), vec![]),
        (events.clone(), vec![("authorization", &right_header)]),
        (
            format!("{events}?token={}", server.token),
            vec![("authorization", &wrong_header)],
        ),
        (
            format!("{events}?token={WRONG}"),
            vec![("authorization", &right_header)],
        ),
    ];
    for (url, headers) in &accepted {
        let mut socket = ws_connect(url, headers)
            .await
            .unwrap_or_else(|err| panic!("{url} {headers:?}: {err}"));
        // It is a working stream, not just an accepted handshake.
        server.app.publish("auth.probe", &json!({"n": 1}));
        let event = next_event_on(&mut socket, "auth.probe").await;
        assert_eq!(event["payload"], json!({"n": 1}));
    }
    server.shutdown().await;
}
