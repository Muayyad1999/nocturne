//! Bearer-token authentication.

use axum::extract::{Query, Request, State};
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::envelope::ApiError;
use crate::state::Shared;

/// Compares presented tokens with the configured one in constant time.
///
/// Both sides are hashed first, so the comparison always covers 32 bytes and
/// reveals neither the token's content nor its length through timing.
pub(crate) struct TokenCheck {
    digest: [u8; 32],
}

impl TokenCheck {
    pub(crate) fn new(token: &str) -> TokenCheck {
        TokenCheck {
            digest: digest(token),
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

fn check(state: &Shared, presented: &[Option<&str>], how: &str) -> Result<(), ApiError> {
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
        Err(ApiError::unauthorized(format!(
            "the access token is not valid for this server. {WHERE_THE_TOKEN_IS}"
        )))
    } else {
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
    );
    match verdict {
        Ok(()) => next.run(request).await,
        Err(refusal) => refusal.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

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
