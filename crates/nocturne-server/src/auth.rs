//! Bearer-token authentication, and the record of who was turned away.
//!
//! A refused request is audited, but not one entry each: anybody who can
//! reach the port can send thousands a second, and an audit trail that can
//! be filled through the front door is no trail. Refusals are counted, and
//! each minute that had any becomes one entry with the counts — never with
//! the token that was tried, which is very often a real token for something
//! else.

use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::extract::{ConnectInfo, Query, Request, State};
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use nocturne_suite::App;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::envelope::ApiError;
use crate::state::Shared;

/// How long refused requests are gathered before they become one audit entry.
const TALLY_WINDOW: Duration = Duration::from_secs(60);
/// How many source addresses one entry names. A flood from spoofed or
/// rotating addresses must not turn the entry itself into the flood.
const SOURCES_NAMED: usize = 8;
/// How long refusals are gathered once the server has begun to stop. A
/// stopping server still answers what its open connections had already
/// sent, and a tally opened then cannot wait out a minute the server does
/// not have. Written at once, each of those requests would be its own entry
/// — the flood this file exists to prevent, in the one moment nobody is
/// watching. Short enough not to hold the stop up.
const STOPPING_WINDOW: Duration = Duration::from_millis(250);

/// Compares presented tokens with the configured one in constant time.
///
/// Both sides are hashed first, so the comparison always covers 32 bytes and
/// reveals neither the token's content nor its length through timing.
pub(crate) struct TokenCheck {
    digest: [u8; 32],
    /// The requests refused in the minute that is running, if any were.
    refused: Mutex<Tally>,
}

impl TokenCheck {
    pub(crate) fn new(token: &str) -> TokenCheck {
        TokenCheck {
            digest: digest(token),
            refused: Mutex::default(),
        }
    }

    pub(crate) fn accepts(&self, presented: &str) -> bool {
        self.digest.ct_eq(&digest(presented)).into()
    }
}

fn digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// The token of an `Authorization: Bearer <token>` header, if there is one.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("Bearer").then(|| token.trim())
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

const WHERE_THE_TOKEN_IS: &str = "The token is the `#token=…` part of the link the server prints \
                                  when it starts; `nocturne-server token` prints it again";

/// Why a request was turned away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusal {
    /// A token was sent and it is not this server's: a guess, or a stale link.
    WrongToken,
    /// No token at all: a scanner, a health check, a tab opened without the link.
    NoToken,
}

/// The refused requests of one minute.
#[derive(Debug, Default, PartialEq, Eq)]
struct Tally {
    wrong_token: u64,
    no_token: u64,
    /// Where they came from, up to [`SOURCES_NAMED`] addresses. Behind a
    /// reverse proxy this is the proxy.
    from: BTreeSet<IpAddr>,
    /// Whether there were more addresses than are named.
    more_sources: bool,
}

impl Tally {
    fn is_empty(&self) -> bool {
        self.wrong_token == 0 && self.no_token == 0
    }

    fn add(&mut self, refusal: Refusal, from: Option<IpAddr>) {
        match refusal {
            Refusal::WrongToken => self.wrong_token += 1,
            Refusal::NoToken => self.no_token += 1,
        }
        if let Some(from) = from
            && !self.from.contains(&from)
        {
            if self.from.len() < SOURCES_NAMED {
                self.from.insert(from);
            } else {
                self.more_sources = true;
            }
        }
    }

    /// Write the minute up as one audit entry. Counts and addresses only:
    /// nothing a client sent is in it.
    fn audit(self, app: &App) {
        if self.is_empty() {
            return;
        }
        let count = self.wrong_token + self.no_token;
        app.audit(
            "server.token_refused",
            None,
            format!(
                "Refused {count} request{} to the server within a minute: {} with a wrong access token, {} with none",
                if count == 1 { "" } else { "s" },
                self.wrong_token,
                self.no_token
            ),
            json!({
                "count": count,
                "bad_token": self.wrong_token,
                "missing_token": self.no_token,
                "from": self.from.iter().map(IpAddr::to_string).collect::<Vec<_>>(),
                "more_sources": self.more_sources,
                "window_s": TALLY_WINDOW.as_secs(),
            }),
        );
    }
}

/// Count a refused request, and see to it that the minute it falls in is
/// written up.
///
/// The request itself only takes a lock and adds one. The audit entry — a
/// database write — is made by a task that the first refusal of a minute
/// starts and that every later one in that minute merely adds to, so a flood
/// costs the server a counter and the trail one line.
fn note_refusal(state: &Shared, refusal: Refusal, from: Option<IpAddr>) {
    let opens_the_minute = {
        let mut tally = state
            .token
            .refused
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let opens = tally.is_empty();
        tally.add(refusal, from);
        opens
    };
    if !opens_the_minute {
        return;
    }
    // Weak, so a tally waiting out its minute does not keep the application
    // (and the lock on its data directory) alive after the server is gone.
    let server = Arc::downgrade(state);
    let shutdown = state.shutdown.clone();
    let stopping = shutdown.is_cancelled();
    // Counted as work in progress, so that a server that is stopping waits
    // the moment it takes to write the entry instead of losing it.
    let in_flight = state.in_flight.enter();
    tokio::spawn(async move {
        if stopping {
            tokio::time::sleep(STOPPING_WINDOW).await;
        } else {
            tokio::select! {
                _ = tokio::time::sleep(TALLY_WINDOW) => {}
                _ = shutdown.cancelled() => {}
            }
        }
        if let Some(state) = server.upgrade() {
            let tally = std::mem::take(
                &mut *state
                    .token
                    .refused
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner),
            );
            tally.audit(&state.app);
        }
        drop(in_flight);
    });
}

/// The address the request came from, as far as the server can see.
fn client(request: &Request) -> Option<IpAddr> {
    request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| address.ip())
}

fn check(
    state: &Shared,
    presented: &[Option<&str>],
    how: &str,
    from: Option<IpAddr>,
) -> Result<(), ApiError> {
    let mut any = false;
    // Every candidate is compared, so a valid query token is not masked by a stale header.
    let mut accepted = false;
    for token in presented.iter().flatten() {
        any = true;
        accepted |= state.token.accepts(token);
    }
    if accepted {
        Ok(())
    } else if any {
        note_refusal(state, Refusal::WrongToken, from);
        Err(ApiError::unauthorized(format!(
            "the access token is not valid for this server. {WHERE_THE_TOKEN_IS}"
        )))
    } else {
        note_refusal(state, Refusal::NoToken, from);
        Err(ApiError::unauthorized(format!(
            "this request needs the access token: send it as {how}. {WHERE_THE_TOKEN_IS}"
        )))
    }
}

/// Middleware for ordinary API routes: the token must be in the `Authorization` header.
pub(crate) async fn require_header(
    State(state): State<Shared>,
    request: Request,
    next: Next,
) -> Response {
    let verdict = check(
        &state,
        &[bearer(request.headers())],
        "`Authorization: Bearer <token>`",
        client(&request),
    );
    match verdict {
        Ok(()) => next.run(request).await,
        Err(refusal) => refusal.into_response(),
    }
}

/// Middleware for the event stream: browsers cannot set headers on a
/// WebSocket, so the token may also arrive as `?token=…`.
pub(crate) async fn require_header_or_query(
    State(state): State<Shared>,
    request: Request,
    next: Next,
) -> Response {
    let query = Query::<TokenQuery>::try_from_uri(request.uri())
        .ok()
        .and_then(|Query(query)| query.token);
    let verdict = check(
        &state,
        &[bearer(request.headers()), query.as_deref()],
        "`?token=<token>` in the WebSocket address or as `Authorization: Bearer <token>`",
        client(&request),
    );
    match verdict {
        Ok(()) => next.run(request).await,
        Err(refusal) => refusal.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{HeaderValue, StatusCode};
    use nocturne_suite::{AppBuilder, AppConfig, Event};
    use serde_json::Value;
    use tokio::sync::broadcast::Receiver;
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt;

    use crate::guard::{OriginPolicy, Scheme};
    use crate::options::Timing;
    use crate::state::ServerState;
    use crate::ui::UiSource;

    const TOKEN: &str = "the-real-token-of-this-server";
    const GUESS: &str = "a-guess-that-is-somebody-elses-real-token";
    const HOST: &str = "127.0.0.1:7878";

    /// A server's shared state over an application with nothing in it but
    /// the audit trail, which is all these tests read.
    fn state() -> (Shared, Receiver<Event>) {
        let app = AppBuilder::new(AppConfig::in_memory()).unwrap().build();
        let events = app.subscribe();
        let state = ServerState::new(
            app,
            TOKEN,
            OriginPolicy::new(Scheme::Http, HOST.parse().unwrap()),
            UiSource::fallback(),
            Timing::default(),
            CancellationToken::new(),
        );
        (state, events)
    }

    /// One request to an API route that needs the token, as if it had
    /// arrived from `from`.
    async fn ask(router: &Router, token: Option<&str>, from: &str) -> StatusCode {
        let mut request = Request::builder()
            .uri("/api/info")
            .header("host", HOST)
            .body(Body::empty())
            .unwrap();
        if let Some(token) = token {
            request.headers_mut().insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
            );
        }
        let from: IpAddr = from.parse().unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(from, 50_000)));
        router.clone().oneshot(request).await.unwrap().status()
    }

    /// The audit entries written so far.
    fn audited(events: &mut Receiver<Event>) -> Vec<Value> {
        let mut entries = Vec::new();
        while let Ok(event) = events.try_recv() {
            if event.topic == "audit.appended" {
                entries.push(event.payload);
            }
        }
        entries
    }

    /// Long enough for the task that writes a minute up to have run.
    async fn a_minute_passes() {
        tokio::time::sleep(TALLY_WINDOW + Duration::from_secs(1)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn refused_requests_are_one_audit_entry_a_minute_with_counts_and_never_the_token() {
        let (state, mut events) = state();
        let router = crate::router::build(state);

        for _ in 0..40 {
            assert_eq!(
                ask(&router, Some(GUESS), "203.0.113.9").await,
                StatusCode::UNAUTHORIZED
            );
        }
        for _ in 0..3 {
            assert_eq!(
                ask(&router, None, "203.0.113.10").await,
                StatusCode::UNAUTHORIZED
            );
        }
        // Nothing is written while the minute runs: forty-three requests
        // have cost the trail nothing yet.
        tokio::time::sleep(TALLY_WINDOW - Duration::from_secs(1)).await;
        assert_eq!(audited(&mut events), Vec::<Value>::new());

        tokio::time::sleep(Duration::from_secs(2)).await;
        let entries = audited(&mut events);
        assert_eq!(entries.len(), 1, "{entries:#?}");
        let entry = &entries[0];
        assert_eq!(entry["source"], "server.token_refused");
        assert_eq!(
            entry["message"],
            "Refused 43 requests to the server within a minute: 40 with a wrong access token, 3 with none"
        );
        assert_eq!(
            entry["body"],
            json!({
                "count": 43,
                "bad_token": 40,
                "missing_token": 3,
                "from": ["203.0.113.9", "203.0.113.10"],
                "more_sources": false,
                "window_s": 60,
            })
        );
        assert!(!entry.to_string().contains(GUESS), "{entry}");
        assert!(!entry.to_string().contains(TOKEN), "{entry}");

        // The next refusal opens the next minute, and is its own entry.
        ask(&router, Some(GUESS), "203.0.113.9").await;
        a_minute_passes().await;
        let entries = audited(&mut events);
        assert_eq!(entries.len(), 1, "{entries:#?}");
        assert_eq!(entries[0]["body"]["count"], 1);
        assert_eq!(
            entries[0]["message"],
            "Refused 1 request to the server within a minute: 1 with a wrong access token, 0 with none"
        );

        // And a quiet minute writes nothing.
        a_minute_passes().await;
        assert_eq!(audited(&mut events), Vec::<Value>::new());
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_with_the_right_token_is_not_recorded_as_refused() {
        let (state, mut events) = state();
        let router = crate::router::build(state);
        assert_eq!(
            ask(&router, Some(TOKEN), "203.0.113.9").await,
            StatusCode::OK
        );
        a_minute_passes().await;
        assert_eq!(audited(&mut events), Vec::<Value>::new());
    }

    #[tokio::test(start_paused = true)]
    async fn a_server_that_stops_writes_up_the_minute_it_was_in() {
        let (state, mut events) = state();
        let router = crate::router::build(state.clone());
        ask(&router, Some(GUESS), "203.0.113.9").await;
        ask(&router, Some(GUESS), "203.0.113.9").await;
        assert_eq!(audited(&mut events), Vec::<Value>::new());

        state.shutdown.cancel();
        // Shutdown waits for work in progress, and the tally counts as that.
        tokio::time::timeout(Duration::from_secs(1), state.in_flight.drained())
            .await
            .expect("the tally is written without waiting out the minute");
        let entries = audited(&mut events);
        assert_eq!(entries.len(), 1, "{entries:#?}");
        assert_eq!(entries[0]["body"]["bad_token"], 2);
    }

    #[tokio::test(start_paused = true)]
    async fn refusals_while_the_server_is_stopping_are_gathered_and_not_one_entry_each() {
        let (state, mut events) = state();
        let router = crate::router::build(state.clone());
        state.shutdown.cancel();

        // What the connections that were open when the stop began still
        // had to say. Each request is given the chance to be written up
        // before the next arrives, as it would have over a network.
        for _ in 0..50 {
            assert_eq!(
                ask(&router, Some(GUESS), "203.0.113.9").await,
                StatusCode::UNAUTHORIZED
            );
            tokio::task::yield_now().await;
        }
        // The stop is not held up for long, and nothing is lost to it.
        tokio::time::timeout(Duration::from_secs(1), state.in_flight.drained())
            .await
            .expect("the tally is written within a moment of the stop");
        let entries = audited(&mut events);
        assert_eq!(entries.len(), 1, "{entries:#?}");
        assert_eq!(entries[0]["body"]["bad_token"], 50);
    }

    #[tokio::test(start_paused = true)]
    async fn ten_thousand_refused_requests_cost_the_trail_one_line() {
        let (state, mut events) = state();
        let router = crate::router::build(state.clone());

        // A guessing flood from a few hundred addresses.
        for n in 0..10_000u32 {
            let from = format!("198.51.{}.{}", 100 + n % 3, 1 + n % 100);
            assert_eq!(
                ask(&router, Some(GUESS), &from).await,
                StatusCode::UNAUTHORIZED
            );
        }
        // While it runs, nothing at all has been written.
        assert_eq!(audited(&mut events), Vec::<Value>::new());

        a_minute_passes().await;
        let entries = audited(&mut events);
        assert_eq!(entries.len(), 1, "{entries:#?}");
        let body = &entries[0]["body"];
        assert_eq!(
            (&body["count"], &body["bad_token"], &body["missing_token"]),
            (&json!(10_000), &json!(10_000), &json!(0))
        );
        // The entry stays one line however many addresses took part.
        assert_eq!(body["from"].as_array().unwrap().len(), SOURCES_NAMED);
        assert_eq!(body["more_sources"], true);
        assert!(!entries[0].to_string().contains(GUESS));
        // And the trail itself holds that one entry, not a copy per request.
        let stored = state.app.store().log_query(&Default::default()).unwrap();
        let trail: Vec<_> = stored
            .iter()
            .filter(|entry| entry.stream == "audit")
            .collect();
        assert_eq!(trail.len(), 1, "{trail:#?}");
    }

    #[test]
    fn a_flood_of_sources_is_counted_but_only_a_few_are_named() {
        let mut tally = Tally::default();
        for last in 1..=20u8 {
            let from: IpAddr = format!("198.51.100.{last}").parse().unwrap();
            tally.add(Refusal::WrongToken, Some(from));
            tally.add(Refusal::WrongToken, Some(from));
        }
        tally.add(Refusal::NoToken, None);
        assert_eq!((tally.wrong_token, tally.no_token), (40, 1));
        assert_eq!(tally.from.len(), SOURCES_NAMED);
        assert!(tally.more_sources);

        let mut few = Tally::default();
        few.add(Refusal::NoToken, Some("198.51.100.1".parse().unwrap()));
        few.add(Refusal::NoToken, Some("198.51.100.1".parse().unwrap()));
        assert!(!few.more_sources);
        assert_eq!(few.from.len(), 1);
    }

    fn headers(authorization: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_str(authorization).unwrap());
        headers
    }

    #[test]
    fn only_the_exact_token_is_accepted() {
        let check = TokenCheck::new("correct-horse");
        assert!(check.accepts("correct-horse"));
        for wrong in [
            "",
            "correct-hors",
            "correct-horse ",
            "correct-horsE",
            "correct-horse-battery",
            "CORRECT-HORSE",
        ] {
            assert!(!check.accepts(wrong), "{wrong:?}");
        }
    }

    #[test]
    fn bearer_header_parsing() {
        assert_eq!(bearer(&headers("Bearer abc")), Some("abc"));
        assert_eq!(bearer(&headers("bearer abc")), Some("abc"));
        assert_eq!(bearer(&headers("BEARER   abc  ")), Some("abc"));
        assert_eq!(bearer(&headers("Bearer ")), Some(""));
        assert_eq!(bearer(&headers("Bearer")), None);
        assert_eq!(bearer(&headers("Basic abc")), None);
        assert_eq!(bearer(&headers("abc")), None);
        assert_eq!(bearer(&HeaderMap::new()), None);
    }
}
