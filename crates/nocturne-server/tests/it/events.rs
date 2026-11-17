//! `GET /api/events`: delivery, filters, the lagged notice, keep-alive and shutdown.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::{SinkExt, StreamExt};
use nocturne_server::Timing;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::support::{
    Socket, TestServer, next_event, next_event_on, raw, raw_request, refusal, within, ws_connect,
};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[tokio::test]
async fn a_mutating_call_is_announced_on_the_event_stream() {
    let server = TestServer::start().await;
    let mut events = server.events("").await;

    let before = now_ms();
    let created = server
        .call(
            "devices.create",
            json!({"name": "edge-fw1", "address": "192.0.2.1"}),
        )
        .await;
    let id = created["id"].as_str().unwrap();

    let event = next_event_on(&mut events, "devices.changed").await;
    let after = now_ms();
    let fields: Vec<&str> = event
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        ["topic", "payload", "at"],
        "one event per frame, nothing else in it"
    );
    assert_eq!(event["payload"], json!({"ids": [id], "change": "created"}));
    let at = event["at"]
        .as_i64()
        .expect("`at` is milliseconds since the epoch");
    assert!(
        (before..=after).contains(&at),
        "{before} <= {at} <= {after}"
    );

    server.call("devices.delete", json!({"ids": [id]})).await;
    let event = next_event_on(&mut events, "devices.changed").await;
    assert_eq!(event["payload"], json!({"ids": [id], "change": "deleted"}));
    server.shutdown().await;
}

#[tokio::test]
async fn every_open_stream_gets_every_event() {
    let server = TestServer::start().await;
    let mut first = server.events("").await;
    let mut second = server.events("").await;
    for n in 0..3 {
        server.app.publish("fanout.test", &json!({"n": n}));
    }
    for socket in [&mut first, &mut second] {
        for n in 0..3 {
            let event = next_event_on(socket, "fanout.test").await;
            assert_eq!(event["payload"], json!({"n": n}));
        }
    }
    server.shutdown().await;
}

#[tokio::test]
async fn a_topic_filter_limits_what_is_sent() {
    let server = TestServer::start().await;
    let mut filtered = server.events("topics=alpha.*,exact.topic").await;

    server.app.publish("beta.one", &json!(1));
    server.app.publish("alpha.one", &json!(2));
    server.app.publish("alph.two", &json!(3));
    server.app.publish("exact.topic", &json!(4));
    server.app.publish("exact.topic.longer", &json!(5));
    server.app.publish("alpha.", &json!(6));
    server.app.publish("exact", &json!(7));
    // What the application itself announces (`audit.appended`, `devices.changed`) is filtered too.
    server
        .call(
            "devices.create",
            json!({"name": "sw1", "address": "10.0.0.1"}),
        )
        .await;
    server.app.publish("exact.topic", &json!("end"));

    let mut seen = Vec::new();
    loop {
        let event = next_event(&mut filtered).await;
        if event["payload"] == "end" {
            break;
        }
        seen.push((
            event["topic"].as_str().unwrap().to_string(),
            event["payload"].clone(),
        ));
    }
    assert_eq!(
        seen,
        [
            ("alpha.one".to_string(), json!(2)),
            ("exact.topic".to_string(), json!(4)),
            ("alpha.".to_string(), json!(6)),
        ]
    );

    // The same events without a filter, to show the filter is what removed the rest.
    let mut unfiltered = server.events("").await;
    server
        .call(
            "devices.create",
            json!({"name": "sw2", "address": "10.0.0.2"}),
        )
        .await;
    assert_eq!(
        next_event_on(&mut unfiltered, "audit.appended").await["topic"],
        "audit.appended"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn filter_spellings() {
    let server = TestServer::start().await;
    // URL-encoded, with spaces, with empty entries, `*` alone, and blank (= everything).
    for query in [
        "topics=probe.%2A",
        "topics=%20probe.*%20,%20other",
        "topics=,probe.*,,",
        "topics=*",
        "topics=",
        "topics=p*",
    ] {
        let mut socket = server.events(query).await;
        server.app.publish("probe.one", &json!({"query": query}));
        let event = next_event_on(&mut socket, "probe.one").await;
        assert_eq!(event["payload"]["query"], query);
    }

    // `*` anywhere but at the end is refused before the stream opens.
    for query in ["topics=a.*.c", "topics=*.changed", "topics=ok,b*d"] {
        let url = server.ws_url(&format!("/api/events?token={}&{query}", server.token));
        let (status, _, body) = refusal(ws_connect(&url, &[]).await);
        assert_eq!(status, 400, "{query}");
        assert_eq!(body["error"]["code"], "invalid", "{query}");
        assert!(
            body["error"]["message"].as_str().unwrap().contains('*'),
            "{query}"
        );
    }
    server.shutdown().await;
}

#[tokio::test]
async fn a_client_that_falls_behind_is_told_how_many_events_it_missed() {
    let server = TestServer::start().await;
    // Filtered to the burst, so that nothing else the application says gets in between.
    let mut events = server.events("topics=burst.*").await;

    // The test and the server share one thread: nothing is delivered while
    // this loop runs, so the stream falls behind by far more than the event
    // bus keeps for it.
    const BURST: u64 = 20_000;
    for n in 0..BURST {
        server.app.publish("burst.test", &json!({"n": n}));
    }

    let notice = next_event(&mut events).await;
    assert_eq!(
        notice["topic"], "events.lagged",
        "the notice comes before what is left"
    );
    let fields: Vec<&str> = notice["payload"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(fields, ["missed"]);
    let missed = notice["payload"]["missed"].as_u64().unwrap();
    assert!(missed > 0 && missed < BURST, "missed {missed} of {BURST}");
    let at = notice["at"].as_i64().unwrap();
    assert!((now_ms() - 60_000..=now_ms()).contains(&at), "{at}");

    // What follows is the tail of the burst: in order, nothing skipped, up to the last event.
    let first = next_event(&mut events).await["payload"]["n"]
        .as_u64()
        .unwrap();
    // `missed` counts everything the bus dropped, which is at least the burst events not delivered.
    assert!(
        first > 0 && first <= missed,
        "first delivered {first}, missed {missed}"
    );
    for n in first + 1..BURST {
        let event = next_event(&mut events).await;
        assert_eq!(event["topic"], "burst.test");
        assert_eq!(
            event["payload"]["n"], n,
            "after a gap notice the rest is complete"
        );
    }

    // And the stream carries on normally afterwards.
    server.app.publish("burst.after", &json!("still alive"));
    assert_eq!(next_event(&mut events).await["payload"], "still alive");
    server.shutdown().await;
}

fn fast_keepalive() -> Timing {
    Timing {
        ws_ping_interval: Duration::from_millis(150),
        ws_idle_timeout: Duration::from_millis(900),
        ..Timing::default()
    }
}

#[tokio::test]
async fn the_server_pings_and_keeps_clients_that_answer() {
    let server = TestServer::start_with(|_| {}, fast_keepalive()).await;
    let mut socket = server.events("").await;
    let started = Instant::now();

    // Reading makes the client library answer each ping with a pong.
    let mut pings = 0;
    while started.elapsed() < Duration::from_millis(2_700) {
        match within("a ping", socket.next()).await {
            Some(Ok(Message::Ping(_))) => pings += 1,
            Some(Ok(other)) => panic!("unexpected frame {other:?}"),
            Some(Err(err)) => panic!("the stream failed: {err}"),
            None => panic!(
                "dropped after {:?} although every ping was answered",
                started.elapsed()
            ),
        }
    }
    // Three idle timeouts have passed and the client was never dropped —
    // which the loop above would have caught, because a drop ends the stream
    // and lands in the `None` arm.
    //
    // The guarantee is that the server keeps pinging a client that answers and
    // keeps it. How *many* pings fit in 2.7 s is the scheduler's business: the
    // interval is a floor, so a busy runner can only deliver fewer, and the
    // old lower bound of six was a bet on the machine managing one every
    // 450 ms. The upper bound is the one worth keeping — a timer that fired in
    // a loop rather than on its interval would sail past it, and no amount of
    // load can make a 150 ms interval produce more than eighteen in 2.7 s.
    assert!(
        pings >= 2,
        "{pings} pings in 2.7 s: the server stopped pinging"
    );
    assert!(pings <= 19, "{pings} pings in 2.7 s, at a 150 ms interval");

    server.app.publish("keepalive.test", &json!(1));
    assert_eq!(
        next_event_on(&mut socket, "keepalive.test").await["payload"],
        1
    );
    server.shutdown().await;
}

#[tokio::test]
async fn a_silent_client_is_dropped() {
    let server = TestServer::start_with(|_| {}, fast_keepalive()).await;
    let mut socket = server.events("").await;

    // Not reading means not answering pings: as far as the server can tell, nobody is there.
    // Long enough to pass the idle timeout (0.9 s), short enough that the server is still
    // waiting for the answer to its close frame (it waits a second): a client that only
    // starts reading after that window finds the socket gone, which proves nothing about
    // *why* it was dropped.
    //
    // This sleep cannot be turned into a wait for a condition, and it is the
    // one thing left in this file that a loaded machine can upset. Reading is
    // what the test must not do — the client library answers a ping the moment
    // the stream is polled, and a client that answers is a client the server
    // is right to keep — so there is nothing to poll. The window it has to
    // land in is `ws_idle_timeout` (0.9 s, configured above) to that plus
    // `events::CLOSE_TIMEOUT` (1 s, not configurable), and 1.2 s sits 300 ms
    // into a 1 s window. Both ends are timers the runner can delay.
    tokio::time::sleep(Duration::from_millis(1_200)).await;

    // What the server sent meanwhile: pings, then the reason, then the end of the stream.
    let mut pings = 0;
    let close = loop {
        match within("the close frame", socket.next()).await {
            Some(Ok(Message::Ping(_))) => pings += 1,
            Some(Ok(Message::Close(frame))) => break frame.expect("the close frame says why"),
            other => panic!("unexpected {other:?}"),
        }
    };
    assert_eq!(close.code, CloseCode::Policy);
    assert!(close.reason.contains("no traffic"), "{}", close.reason);
    // The close frame above is the whole claim: the server noticed the silence
    // and said why it was closing. The loop that found it also establishes
    // that the pings stopped, because a server that kept pinging would never
    // have sent one and the read would have timed out instead.
    //
    // How many pings fit in the 0.9 s before the drop is the scheduler's
    // business: the interval is a floor, so a busy runner delivers fewer, and
    // a lower bound of three was a bet on it managing one every 300 ms. The
    // upper bound stays — at a 150 ms interval nothing can produce more than
    // this inside the idle timeout, so a ping loop still fails here.
    assert!(
        pings >= 1,
        "the server never pinged before dropping the client"
    );
    assert!(pings <= 8, "{pings} pings before the drop");

    // The connection is really gone, not just marked.
    let end = within("the end of the stream", async {
        loop {
            match socket.next().await {
                None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            }
        }
    });
    end.await;
    server.shutdown().await;
}

#[tokio::test]
async fn shutdown_closes_open_streams_cleanly_and_promptly() {
    let mut server = TestServer::start().await;
    let mut first = server.events("").await;
    let mut second = server.events("topics=nothing").await;
    server.app.publish("before.shutdown", &json!(1));
    assert_eq!(
        next_event_on(&mut first, "before.shutdown").await["payload"],
        1
    );

    let handle = server.take_handle();
    let started = Instant::now();
    let shutdown = tokio::spawn(handle.shutdown());

    for socket in [&mut first, &mut second] {
        let close = loop {
            match within("the close frame", socket.next()).await {
                Some(Ok(Message::Close(frame))) => break frame.expect("the close frame says why"),
                Some(Ok(Message::Ping(_))) => {}
                other => panic!("unexpected {other:?}"),
            }
        };
        assert_eq!(close.code, CloseCode::Away);
        assert!(close.reason.contains("shutting down"), "{}", close.reason);
        // The client library answers the close; then the stream ends without an error.
        assert!(
            within("the end of the stream", socket.next())
                .await
                .is_none()
        );
    }

    within("shutdown to finish", shutdown).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "open event streams must not hold shutdown for the whole grace period ({:?})",
        started.elapsed()
    );

    // Nothing listens any more.
    assert!(tokio::net::TcpStream::connect(server.addr).await.is_err());
}

#[tokio::test]
async fn a_stream_closed_by_the_client_ends_quietly() {
    let server = TestServer::start().await;
    let mut socket: Socket = server.events("").await;
    socket.close(None).await.unwrap();
    // The server answers the close and the stream ends.
    let leftovers = within("the closing handshake", async {
        let mut frames = Vec::new();
        while let Some(Ok(frame)) = socket.next().await {
            frames.push(frame);
        }
        frames
    })
    .await;
    assert!(
        leftovers
            .iter()
            .all(|frame| matches!(frame, Message::Close(_))),
        "{leftovers:?}"
    );

    // The server did not mind; new streams work.
    let mut again = server.events("").await;
    server.app.publish("after.close", &json!(true));
    assert_eq!(
        next_event_on(&mut again, "after.close").await["payload"],
        true
    );
    server.shutdown().await;
}

#[tokio::test]
async fn messages_from_the_client_are_ignored() {
    let server = TestServer::start().await;
    let mut socket = server.events("").await;
    socket
        .send(Message::Text("{\"subscribe\": \"devices.*\"}".into()))
        .await
        .unwrap();
    socket
        .send(Message::Binary(vec![1, 2, 3].into()))
        .await
        .unwrap();
    server.app.publish("still.flowing", &json!(1));
    assert_eq!(
        next_event_on(&mut socket, "still.flowing").await["payload"],
        1
    );
    server.shutdown().await;
}

#[tokio::test]
async fn a_plain_get_on_the_event_stream_explains_itself() {
    let server = TestServer::start().await;
    let authorization = format!("Bearer {}", server.token);
    let response = raw(
        server.addr,
        raw_request(
            "GET",
            "/api/events",
            &[("Host", &server.host()), ("Authorization", &authorization)],
            "",
        ),
    )
    .await;
    assert!((400..500).contains(&response.status), "{}", response.status);
    let reply: Value = response.json();
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "invalid");
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("WebSocket")
    );
    server.shutdown().await;
}
