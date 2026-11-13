//! `GET /api/events`: the application's event bus over a WebSocket.
//!
//! Each text frame is one event, `{"topic", "payload", "at"}`. The server
//! never expects anything from the client except proof of life: it pings on
//! a fixed interval and drops a client that stays silent for too long.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::rejection::QueryRejection;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{CloseFrame, Message, WebSocketUpgrade, close_code};
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use futures::{Sink, SinkExt, Stream, StreamExt};
use nocturne_suite::Event;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::envelope::ApiError;
use crate::options::Timing;
use crate::state::Shared;
use crate::text::printable;

/// The topic of the notice sent when events were dropped for a slow client.
const LAGGED_TOPIC: &str = "events.lagged";

/// Clients have nothing to say on this socket; anything bigger is not ours.
const MAX_CLIENT_MESSAGE_BYTES: usize = 64 * 1024;

/// How long the closing handshake may take before the socket is simply dropped.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Deserialize)]
pub(crate) struct EventsQuery {
    topics: Option<String>,
}

/// The upgrade handler. Authentication has already happened in middleware.
pub(crate) async fn connect(
    State(state): State<Shared>,
    query: Result<Query<EventsQuery>, QueryRejection>,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let filter = match query {
        Ok(Query(query)) => TopicFilter::parse(query.topics.as_deref()),
        Err(rejection) => Err(ApiError::invalid(format!(
            "the query string of the event stream address is not valid: {}",
            rejection.body_text()
        ))),
    };
    let filter = match filter {
        Ok(filter) => filter,
        Err(refusal) => return refusal.into_response(),
    };
    let upgrade = match upgrade {
        Ok(upgrade) => upgrade,
        Err(rejection) => {
            return ApiError::invalid_with(
                rejection.status(),
                format!(
                    "/api/events is a WebSocket endpoint: connect to it with a WebSocket client \
                     (ws:// or wss://). {}",
                    rejection.body_text()
                ),
            )
            .into_response();
        }
    };
    if state.shutdown.is_cancelled() {
        return ApiError::unavailable("the server is shutting down").into_response();
    }
    let Ok(slot) = state.event_streams.clone().try_acquire_owned() else {
        return ApiError::unavailable(
            "too many event streams are open on this server; close unused Nocturne tabs",
        )
        .into_response();
    };

    // Subscribed before the handshake is answered: nothing published after
    // the client sees the connection open can be missed.
    let events = state.app.subscribe();
    let in_flight = state.in_flight.enter();
    let timing = state.timing;
    let shutdown = state.shutdown.clone();
    upgrade
        .max_message_size(MAX_CLIENT_MESSAGE_BYTES)
        .max_frame_size(MAX_CLIENT_MESSAGE_BYTES)
        .on_failed_upgrade(|err| tracing::debug!(%err, "event stream upgrade failed"))
        .on_upgrade(move |socket| async move {
            let _slot = slot;
            let _in_flight = in_flight;
            run(socket, events, filter, timing, shutdown).await;
        })
}

/// Which topics a client asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TopicFilter {
    /// `None` is everything.
    patterns: Option<Vec<Pattern>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Pattern {
    Exact(String),
    Prefix(String),
}

impl TopicFilter {
    /// Parse the `topics` query value: a comma list in which a trailing `*`
    /// matches a prefix. Absent or blank is everything.
    pub(crate) fn parse(topics: Option<&str>) -> Result<TopicFilter, ApiError> {
        let patterns = topics
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|pattern| !pattern.is_empty())
            .map(|pattern| match pattern.split_once('*') {
                None => Ok(Pattern::Exact(pattern.to_string())),
                Some((prefix, "")) => Ok(Pattern::Prefix(prefix.to_string())),
                Some(_) => Err(ApiError::invalid(format!(
                    "the topic filter `{}` is not understood: `*` may only be the last character \
                     (`devices.*`)",
                    printable(pattern, 64)
                ))),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TopicFilter {
            patterns: (!patterns.is_empty()).then_some(patterns),
        })
    }

    pub(crate) fn matches(&self, topic: &str) -> bool {
        let Some(patterns) = &self.patterns else {
            return true;
        };
        patterns.iter().any(|pattern| match pattern {
            Pattern::Exact(exact) => topic == exact,
            Pattern::Prefix(prefix) => topic.starts_with(prefix.as_str()),
        })
    }
}

/// Why a session is over.
#[derive(Debug)]
enum Ended {
    /// The server is shutting down.
    Shutdown,
    /// The client said nothing for the idle timeout.
    Silent,
    /// The client closed the stream.
    ClosedByPeer,
    /// The connection is gone.
    Lost,
}

/// Serve one event stream until it ends, then close the socket.
pub(crate) async fn run<S, E>(
    socket: S,
    mut events: broadcast::Receiver<Event>,
    filter: TopicFilter,
    timing: Timing,
    shutdown: CancellationToken,
) where
    S: Stream<Item = Result<Message, E>> + Sink<Message>,
{
    let (mut sink, mut stream) = socket.split();
    // Reading and writing are independent futures polled side by side, so a
    // client that stopped reading (writes block) is still timed out, and one
    // that only reads slowly keeps getting credit for its pongs.
    let ended = tokio::select! {
        biased;
        _ = shutdown.cancelled() => Ended::Shutdown,
        ended = watch_peer(&mut stream, timing.ws_idle_timeout) => ended,
        ended = forward(&mut sink, &mut events, &filter, timing.ws_ping_interval) => ended,
    };

    let farewell = match ended {
        Ended::Lost => return,
        // The reply to the peer's close frame is already queued; `close` flushes it.
        Ended::ClosedByPeer => None,
        Ended::Shutdown => Some(CloseFrame {
            code: close_code::AWAY,
            reason: "the server is shutting down".into(),
        }),
        Ended::Silent => Some(CloseFrame {
            code: close_code::POLICY,
            reason: format!(
                "no traffic from the client for {} s",
                timing.ws_idle_timeout.as_secs()
            )
            .into(),
        }),
    };
    let closing = async {
        match farewell {
            None => {
                let _ = sink.close().await;
            }
            Some(frame) => {
                if sink.send(Message::Close(Some(frame))).await.is_ok() {
                    // Give the peer the chance to answer, so both ends see a clean close.
                    while let Some(Ok(message)) = stream.next().await {
                        if matches!(message, Message::Close(_)) {
                            break;
                        }
                    }
                }
            }
        }
    };
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, closing).await;
}

/// Read from the client until it closes, disappears or stays silent too long.
async fn watch_peer<R, E>(stream: &mut R, idle_timeout: Duration) -> Ended
where
    R: Stream<Item = Result<Message, E>> + Unpin,
{
    loop {
        match tokio::time::timeout(idle_timeout, stream.next()).await {
            Err(_) => return Ended::Silent,
            Ok(None | Some(Err(_))) => return Ended::Lost,
            Ok(Some(Ok(Message::Close(_)))) => return Ended::ClosedByPeer,
            // Pongs, pings and anything else: the client is alive.
            Ok(Some(Ok(_))) => {}
        }
    }
}

/// Write events and pings to the client until the socket fails.
async fn forward<W>(
    sink: &mut W,
    events: &mut broadcast::Receiver<Event>,
    filter: &TopicFilter,
    ping_interval: Duration,
) -> Ended
where
    W: Sink<Message> + Unpin,
{
    let mut ping = tokio::time::interval_at(Instant::now() + ping_interval, ping_interval);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        let frame = tokio::select! {
            _ = ping.tick() => Message::Ping(Default::default()),
            received = events.recv() => match received {
                Ok(event) if filter.matches(&event.topic) => match serde_json::to_string(&event) {
                    Ok(text) => Message::Text(text.into()),
                    Err(err) => {
                        tracing::error!(topic = %event.topic, %err, "an event could not be encoded");
                        continue;
                    }
                },
                Ok(_) => continue,
                // The client fell behind and the bus dropped events for it.
                // Which ones is unknown, so it is told regardless of its filter.
                Err(RecvError::Lagged(missed)) => Message::Text(lagged_notice(missed).into()),
                // Cannot happen while the application is alive; nothing more will come.
                Err(RecvError::Closed) => return Ended::Shutdown,
            },
        };
        if sink.send(frame).await.is_err() {
            return Ended::Lost;
        }
    }
}

fn lagged_notice(missed: u64) -> String {
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_millis() as i64);
    json!({"topic": LAGGED_TOPIC, "payload": {"missed": missed}, "at": at}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::mpsc;
    use serde_json::Value;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::task::JoinHandle;

    /// The server's end of an in-memory socket.
    struct ServerEnd {
        from_client: mpsc::UnboundedReceiver<Message>,
        to_client: mpsc::Sender<Message>,
    }

    impl Stream for ServerEnd {
        type Item = Result<Message, std::convert::Infallible>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Pin::new(&mut self.from_client)
                .poll_next(cx)
                .map(|item| item.map(Ok))
        }
    }

    impl Sink<Message> for ServerEnd {
        type Error = mpsc::SendError;

        fn poll_ready(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Pin::new(&mut self.to_client).poll_ready(cx)
        }

        fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            Pin::new(&mut self.to_client).start_send(item)
        }

        fn poll_flush(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Pin::new(&mut self.to_client).poll_flush(cx)
        }

        fn poll_close(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Pin::new(&mut self.to_client).poll_close(cx)
        }
    }

    /// A running session and the client's view of it.
    struct Session {
        bus: broadcast::Sender<Event>,
        send: mpsc::UnboundedSender<Message>,
        receive: mpsc::Receiver<Message>,
        shutdown: CancellationToken,
        task: JoinHandle<()>,
        started: Instant,
    }

    impl Session {
        /// `backlog` is how many frames the client's side buffers; beyond it the server's writes block.
        fn start(topics: Option<&str>, bus_capacity: usize, backlog: usize) -> Session {
            let (bus, events) = broadcast::channel(bus_capacity);
            let (send, from_client) = mpsc::unbounded();
            let (to_client, receive) = mpsc::channel(backlog);
            let shutdown = CancellationToken::new();
            let task = tokio::spawn(run(
                ServerEnd {
                    from_client,
                    to_client,
                },
                events,
                TopicFilter::parse(topics).unwrap(),
                Timing::default(),
                shutdown.clone(),
            ));
            Session {
                bus,
                send,
                receive,
                shutdown,
                task,
                started: Instant::now(),
            }
        }

        fn publish(&self, topic: &str, payload: Value, at: i64) {
            self.bus
                .send(Event {
                    topic: topic.to_string(),
                    payload,
                    at,
                })
                .unwrap();
        }

        async fn frame(&mut self) -> Option<Message> {
            self.receive.next().await
        }

        async fn json(&mut self) -> Value {
            match self.frame().await {
                Some(Message::Text(text)) => serde_json::from_str(&text).unwrap(),
                other => panic!("expected a text frame, got {other:?}"),
            }
        }

        fn seconds(&self) -> u64 {
            self.started.elapsed().as_secs()
        }

        fn pong(&self) {
            self.send
                .unbounded_send(Message::Pong(Default::default()))
                .unwrap();
        }
    }

    async fn ended(task: JoinHandle<()>) {
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the session ends")
            .unwrap();
    }

    #[test]
    fn topic_filters() {
        let all = TopicFilter::parse(None).unwrap();
        assert!(all.matches("devices.changed") && all.matches("anything"));
        for blank in ["", " ", ",", " , ,"] {
            assert_eq!(TopicFilter::parse(Some(blank)).unwrap(), all, "{blank:?}");
        }

        let some = TopicFilter::parse(Some("devices.*, alerts.raised,terminal.data.*")).unwrap();
        for topic in [
            "devices.changed",
            "devices.state",
            "devices.",
            "alerts.raised",
            "terminal.data.abc",
        ] {
            assert!(some.matches(topic), "{topic}");
        }
        for topic in [
            "devices",
            "device.changed",
            "alerts.raised.again",
            "alerts.cleared",
            "audit.appended",
            "terminal.data",
            "",
        ] {
            assert!(!some.matches(topic), "{topic}");
        }

        let star = TopicFilter::parse(Some("*")).unwrap();
        assert!(star.matches("anything.at.all") && star.matches(""));
        let prefix = TopicFilter::parse(Some("dev*")).unwrap();
        assert!(prefix.matches("devices.changed") && !prefix.matches("alerts.dev"));

        for bad in ["a.*.c", "*.changed", "a**", "ok,b*d"] {
            assert!(TopicFilter::parse(Some(bad)).is_err(), "{bad}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn events_are_forwarded_in_order_as_json() {
        let mut session = Session::start(None, 16, 16);
        session.publish(
            "devices.changed",
            json!({"ids": ["a"], "change": "created"}),
            1_700_000_000_123,
        );
        session.publish("audit.appended", json!(null), 5);
        assert_eq!(
            session.json().await,
            json!({"topic": "devices.changed", "payload": {"ids": ["a"], "change": "created"}, "at": 1_700_000_000_123_i64})
        );
        assert_eq!(
            session.json().await,
            json!({"topic": "audit.appended", "payload": null, "at": 5})
        );
    }

    #[tokio::test(start_paused = true)]
    async fn only_matching_topics_are_forwarded() {
        let mut session = Session::start(Some("devices.*"), 16, 16);
        session.publish("audit.appended", json!(1), 1);
        session.publish("devices.changed", json!(2), 2);
        session.publish("alerts.raised", json!(3), 3);
        session.publish("devices.state", json!(4), 4);
        assert_eq!(session.json().await["payload"], 2);
        assert_eq!(session.json().await["payload"], 4);
    }

    #[tokio::test(start_paused = true)]
    async fn a_client_that_fell_behind_is_told_how_much_it_missed() {
        // The bus keeps 4 events; 10 are published before the session gets to run.
        let mut session = Session::start(Some("seq"), 4, 16);
        for n in 0..10 {
            session.publish("seq", json!(n), n);
        }
        let notice = session.json().await;
        assert_eq!(notice["topic"], "events.lagged");
        assert_eq!(notice["payload"], json!({"missed": 6}));
        let at = notice["at"].as_i64().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!((now - 60_000..=now).contains(&at), "at = {at}, now = {now}");
        for n in 6..10 {
            assert_eq!(
                session.json().await,
                json!({"topic": "seq", "payload": n, "at": n})
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_lag_notice_is_sent_even_when_the_filter_matches_nothing() {
        let mut session = Session::start(Some("nothing.matches"), 2, 16);
        for n in 0..5 {
            session.publish("seq", json!(n), n);
        }
        let notice = session.json().await;
        assert_eq!(notice["topic"], "events.lagged");
        assert_eq!(notice["payload"]["missed"], 3);
    }

    #[tokio::test(start_paused = true)]
    async fn the_server_pings_every_20_seconds_and_keeps_a_client_that_answers() {
        let mut session = Session::start(None, 16, 16);
        for round in 1..=10 {
            assert_eq!(
                session.frame().await,
                Some(Message::Ping(Default::default()))
            );
            assert_eq!(session.seconds(), 20 * round);
            session.pong();
        }
        assert!(!session.task.is_finished());
    }

    #[tokio::test(start_paused = true)]
    async fn a_client_silent_for_60_seconds_is_dropped() {
        let mut session = Session::start(None, 16, 16);
        assert_eq!(
            session.frame().await,
            Some(Message::Ping(Default::default()))
        );
        assert_eq!(session.seconds(), 20);
        assert_eq!(
            session.frame().await,
            Some(Message::Ping(Default::default()))
        );
        assert_eq!(session.seconds(), 40);
        match session.frame().await {
            Some(Message::Close(Some(frame))) => {
                assert_eq!(frame.code, 1008);
                assert!(frame.reason.contains("60 s"), "{}", frame.reason);
            }
            other => panic!("expected a close frame, got {other:?}"),
        }
        assert_eq!(session.seconds(), 60);
        ended(session.task).await;
    }

    #[tokio::test(start_paused = true)]
    async fn any_frame_from_the_client_restarts_the_idle_clock() {
        let mut session = Session::start(None, 16, 16);
        tokio::time::sleep(Duration::from_secs(50)).await;
        session
            .send
            .unbounded_send(Message::Text("still here".into()))
            .unwrap();
        // Pings at 20, 40, 60, 80 and 100 s, then the drop 60 s after the last sign of life.
        let mut pings = 0;
        loop {
            match session.frame().await {
                Some(Message::Ping(_)) => pings += 1,
                Some(Message::Close(_)) => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(pings, 5);
        assert_eq!(session.seconds(), 110);
        ended(session.task).await;
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_closes_the_stream_with_going_away() {
        let mut session = Session::start(None, 16, 16);
        session.publish("a", json!(1), 1);
        assert_eq!(session.json().await["topic"], "a");

        session.shutdown.cancel();
        match session.frame().await {
            Some(Message::Close(Some(frame))) => assert_eq!(frame.code, 1001),
            other => panic!("expected a close frame, got {other:?}"),
        }
        // The client answers the close; the session ends without waiting out its timeout.
        session.send.unbounded_send(Message::Close(None)).unwrap();
        ended(session.task).await;
        assert_eq!(session.started.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_does_not_wait_long_for_a_client_that_never_answers() {
        let mut session = Session::start(None, 16, 16);
        session.shutdown.cancel();
        assert!(matches!(session.frame().await, Some(Message::Close(_))));
        ended(session.task).await;
        assert_eq!(session.started.elapsed(), CLOSE_TIMEOUT);
    }

    #[tokio::test(start_paused = true)]
    async fn a_close_from_the_client_ends_the_session() {
        let session = Session::start(None, 16, 16);
        session.send.unbounded_send(Message::Close(None)).unwrap();
        ended(session.task).await;
        assert_eq!(session.started.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn a_vanished_client_ends_the_session() {
        let session = Session::start(None, 16, 16);
        drop(session.send);
        ended(session.task).await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_client_that_stopped_reading_is_still_dropped_and_shut_down() {
        // No buffer on the client side: the server's writes block forever.
        let silent = Session::start(None, 64, 0);
        for n in 0..10 {
            silent.publish("seq", json!(n), n);
        }
        tokio::time::sleep(Duration::from_secs(59)).await;
        assert!(!silent.task.is_finished(), "not silent for long enough yet");
        ended(silent.task).await;
        // The idle timeout, then one attempt to say goodbye into the blocked socket.
        assert_eq!(
            silent.started.elapsed(),
            Duration::from_secs(60) + CLOSE_TIMEOUT
        );

        let stuck = Session::start(None, 64, 0);
        for n in 0..10 {
            stuck.publish("seq", json!(n), n);
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
        stuck.shutdown.cancel();
        ended(stuck.task).await;
        assert_eq!(
            stuck.started.elapsed(),
            Duration::from_secs(5) + CLOSE_TIMEOUT
        );
    }
}
