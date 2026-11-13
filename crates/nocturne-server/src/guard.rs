//! Protection against DNS rebinding and cross-site requests.
//!
//! * A server bound to loopback only answers requests whose `Host` is a
//!   loopback name with the server's own port. A page on `evil.example` that
//!   re-points its name at `127.0.0.1` still sends `Host: evil.example`.
//! * On the API, a request that carries an `Origin` must come from a page the
//!   server itself served: same scheme, host and port as the request target.
//!
//! The token remains the real gate; these checks keep a browser from being
//! used as a stepping stone and protect the routes that need no token.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;

use axum::extract::{Request, State};
use axum::http::header::{HOST, ORIGIN};
use axum::http::uri::Authority;
use axum::http::{HeaderMap, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::envelope::ApiError;
use crate::options::is_loopback;
use crate::router::is_api_path;
use crate::state::Shared;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scheme {
    Http,
    Https,
}

impl Scheme {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Scheme::Http => "http",
            Scheme::Https => "https",
        }
    }

    fn default_port(self) -> u16 {
        match self {
            Scheme::Http => 80,
            Scheme::Https => 443,
        }
    }
}

/// What the server knows about itself that the checks compare requests against.
#[derive(Clone, Debug)]
pub(crate) struct OriginPolicy {
    scheme: Scheme,
    /// The port the listener really has (never 0).
    port: u16,
    /// Bound to a loopback address, so only loopback names are valid targets.
    loopback_only: bool,
}

impl OriginPolicy {
    pub(crate) fn new(scheme: Scheme, local: SocketAddr) -> OriginPolicy {
        OriginPolicy {
            scheme,
            port: local.port(),
            loopback_only: is_loopback(local.ip()),
        }
    }

    /// Decide whether a request may proceed. `api` is whether it targets `/api/…`.
    pub(crate) fn check(&self, uri: &Uri, headers: &HeaderMap, api: bool) -> Result<(), ApiError> {
        let target = Target::of(uri, headers);

        if self.loopback_only {
            let addressed_to_us = target.as_ref().is_ok_and(|target| {
                is_loopback_host(&target.host) && self.port_of(target) == self.port
            });
            if !addressed_to_us {
                let seen = match &target {
                    Ok(target) => format!("was `{}`", printable(&target.raw)),
                    Err(problem) => (*problem).to_string(),
                };
                return Err(ApiError::forbidden(format!(
                    "this server listens on a loopback address and only answers requests \
                     addressed to {scheme}://127.0.0.1:{port} or {scheme}://localhost:{port}, \
                     but the Host of this request {seen}; if you are forwarding a port, keep \
                     the port number {port} on both ends",
                    scheme = self.scheme.as_str(),
                    port = self.port,
                )));
            }
        }

        if api {
            let mut origins = headers.get_all(ORIGIN).iter();
            if let Some(origin) = origins.next() {
                let same_origin = origins.next().is_none()
                    && target
                        .as_ref()
                        .is_ok_and(|target| self.is_same_origin(origin.as_bytes(), target));
                if !same_origin {
                    return Err(ApiError::forbidden(format!(
                        "cross-origin requests are not accepted: this one comes from a page at \
                         `{}`, which is not where this server is being reached",
                        printable(&String::from_utf8_lossy(origin.as_bytes()))
                    )));
                }
            }
        }
        Ok(())
    }

    fn port_of(&self, target: &Target) -> u16 {
        target.port.unwrap_or(self.scheme.default_port())
    }

    fn is_same_origin(&self, origin: &[u8], target: &Target) -> bool {
        let Some(origin) = std::str::from_utf8(origin)
            .ok()
            .and_then(|origin| Uri::from_str(origin).ok())
        else {
            // Includes the opaque origin `null` of sandboxed and `file:` pages.
            return false;
        };
        let (Some(scheme), Some(authority)) = (origin.scheme_str(), origin.authority()) else {
            return false;
        };
        // A serialised origin is scheme://host[:port] and nothing else.
        if origin.path() != "/" || origin.query().is_some() || authority.as_str().contains('@') {
            return false;
        }
        scheme.eq_ignore_ascii_case(self.scheme.as_str())
            && authority.host().eq_ignore_ascii_case(&target.host)
            && authority.port_u16().unwrap_or(self.scheme.default_port()) == self.port_of(target)
    }
}

/// The host and port a request was addressed to.
#[derive(Debug)]
struct Target {
    /// As sent, for error messages.
    raw: String,
    /// Without the port; IPv6 literals keep their brackets.
    host: String,
    port: Option<u16>,
}

impl Target {
    /// From the request target itself when it names an authority (HTTP/2,
    /// absolute-form), otherwise from the `Host` header.
    fn of(uri: &Uri, headers: &HeaderMap) -> Result<Target, &'static str> {
        let mut hosts = headers.get_all(HOST).iter();
        let header = match (hosts.next(), hosts.next()) {
            (Some(_), Some(_)) => return Err("was sent more than once"),
            (Some(host), None) => Some(Target::parse(
                std::str::from_utf8(host.as_bytes()).map_err(|_| "is not valid text")?,
            )?),
            (None, _) => None,
        };
        match (uri.authority(), header) {
            (Some(authority), Some(header)) => {
                let target = Target::from_authority(authority)?;
                // A request that names two different targets is addressed to neither.
                if target.host.eq_ignore_ascii_case(&header.host) && target.port == header.port {
                    Ok(target)
                } else {
                    Err("does not match the request target")
                }
            }
            (Some(authority), None) => Target::from_authority(authority),
            (None, Some(header)) => Ok(header),
            (None, None) => Err("is missing"),
        }
    }

    fn parse(raw: &str) -> Result<Target, &'static str> {
        let authority = Authority::from_str(raw).map_err(|_| "is not a valid host name")?;
        Target::from_authority(&authority)
    }

    fn from_authority(authority: &Authority) -> Result<Target, &'static str> {
        if authority.as_str().contains('@') {
            return Err("contains user information");
        }
        Ok(Target {
            raw: authority.as_str().to_string(),
            host: authority.host().to_string(),
            port: authority.port_u16(),
        })
    }
}

/// `localhost` or a loopback address literal. Names that merely resolve to
/// loopback do not count: that is exactly what DNS rebinding produces.
fn is_loopback_host(host: &str) -> bool {
    let name = host.strip_suffix('.').unwrap_or(host);
    if name.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        Some(v6) => v6
            .parse::<Ipv6Addr>()
            .is_ok_and(|ip| is_loopback(IpAddr::V6(ip))),
        None => host.parse::<Ipv4Addr>().is_ok_and(|ip| ip.is_loopback()),
    }
}

/// A header value as it may be quoted back in an error message.
fn printable(value: &str) -> String {
    crate::text::printable(value, 100)
}

/// Middleware: refuse requests that fail the checks, before anything else looks at them.
pub(crate) async fn enforce(State(state): State<Shared>, request: Request, next: Next) -> Response {
    let api = is_api_path(request.uri().path());
    match state.origin.check(request.uri(), request.headers(), api) {
        Ok(()) => next.run(request).await,
        Err(refusal) => refusal.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn policy(scheme: Scheme, local: &str) -> OriginPolicy {
        OriginPolicy::new(scheme, local.parse().unwrap())
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(
                axum::http::HeaderName::from_str(name).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        headers
    }

    fn allowed(policy: &OriginPolicy, pairs: &[(&str, &str)], api: bool) -> bool {
        policy
            .check(&Uri::from_static("/api/rpc"), &headers(pairs), api)
            .is_ok()
    }

    #[test]
    fn loopback_server_accepts_only_loopback_hosts_with_its_port() {
        let policy = policy(Scheme::Http, "127.0.0.1:7878");
        for host in [
            "127.0.0.1:7878",
            "localhost:7878",
            "LOCALHOST:7878",
            "localhost.:7878",
            "[::1]:7878",
            "127.0.0.2:7878",
        ] {
            assert!(allowed(&policy, &[("host", host)], false), "{host}");
        }
        for host in [
            "evil.example:7878",
            "evil.example",
            "localhost:7879",
            "localhost",
            "127.0.0.1",
            "127.0.0.1.evil.example:7878",
            "localhost.evil.example:7878",
            "nocturne.localhost:7878",
            "10.0.0.5:7878",
            "[::2]:7878",
            "[127.0.0.1]:7878",
            "user@localhost:7878",
            "",
        ] {
            assert!(!allowed(&policy, &[("host", host)], false), "{host:?}");
        }
        assert!(!allowed(&policy, &[], false), "a missing Host is refused");
        assert!(
            !allowed(
                &policy,
                &[("host", "localhost:7878"), ("host", "localhost:7878")],
                false
            ),
            "two Host headers are refused"
        );
    }

    #[test]
    fn default_ports_are_understood() {
        let http = policy(Scheme::Http, "127.0.0.1:80");
        assert!(allowed(&http, &[("host", "localhost")], false));
        assert!(allowed(&http, &[("host", "localhost:80")], false));
        assert!(!allowed(&http, &[("host", "localhost:443")], false));

        let https = policy(Scheme::Https, "[::1]:443");
        assert!(allowed(&https, &[("host", "[::1]")], false));
        assert!(!allowed(&https, &[("host", "[::1]:80")], false));
        assert!(allowed(
            &https,
            &[("host", "localhost"), ("origin", "https://localhost")],
            true
        ));
        assert!(allowed(
            &https,
            &[("host", "localhost"), ("origin", "https://localhost:443")],
            true
        ));
    }

    #[test]
    fn a_server_reachable_from_the_network_accepts_any_host() {
        let policy = policy(Scheme::Https, "0.0.0.0:7878");
        for host in ["noc.example:7878", "10.0.0.5:7878", "localhost:1"] {
            assert!(allowed(&policy, &[("host", host)], false), "{host}");
        }
    }

    #[test]
    fn api_requests_with_an_origin_must_be_same_origin() {
        let policy = policy(Scheme::Http, "127.0.0.1:7878");
        let host = ("host", "127.0.0.1:7878");
        assert!(
            allowed(&policy, &[host], true),
            "no Origin: not a browser page"
        );
        assert!(allowed(
            &policy,
            &[host, ("origin", "http://127.0.0.1:7878")],
            true
        ));
        assert!(allowed(
            &policy,
            &[host, ("origin", "HTTP://127.0.0.1:7878")],
            true
        ));
        for origin in [
            "http://evil.example",
            "http://evil.example:7878",
            "https://127.0.0.1:7878",
            "http://127.0.0.1:7879",
            "http://127.0.0.1",
            "http://localhost:7878",
            "http://127.0.0.1:7878/path",
            "http://127.0.0.1:7878?x=1",
            "http://user@127.0.0.1:7878",
            "ws://127.0.0.1:7878",
            "null",
            "",
            "127.0.0.1:7878",
        ] {
            assert!(
                !allowed(&policy, &[host, ("origin", origin)], true),
                "{origin:?}"
            );
        }
        assert!(
            !allowed(
                &policy,
                &[
                    host,
                    ("origin", "http://127.0.0.1:7878"),
                    ("origin", "http://127.0.0.1:7878")
                ],
                true
            ),
            "two Origin headers are refused"
        );
        // Outside the API the Origin header is not looked at.
        assert!(allowed(
            &policy,
            &[host, ("origin", "http://evil.example")],
            false
        ));
    }

    #[test]
    fn origin_is_compared_with_the_host_the_request_used() {
        let policy = policy(Scheme::Https, "0.0.0.0:8443");
        assert!(allowed(
            &policy,
            &[
                ("host", "noc.example:8443"),
                ("origin", "https://noc.example:8443")
            ],
            true
        ));
        assert!(allowed(
            &policy,
            &[
                ("host", "NOC.example:8443"),
                ("origin", "https://noc.EXAMPLE:8443")
            ],
            true
        ));
        assert!(!allowed(
            &policy,
            &[
                ("host", "noc.example:8443"),
                ("origin", "https://other.example:8443")
            ],
            true
        ));
        assert!(!allowed(
            &policy,
            &[
                ("host", "noc.example:8443"),
                ("origin", "http://noc.example:8443")
            ],
            true
        ));
        // Without a usable Host there is nothing an Origin could match.
        assert!(!allowed(
            &policy,
            &[("origin", "https://noc.example:8443")],
            true
        ));
    }

    #[test]
    fn http2_style_targets_use_the_authority() {
        let policy = policy(Scheme::Https, "127.0.0.1:7878");
        let uri = Uri::from_static("https://localhost:7878/api/rpc");
        assert!(policy.check(&uri, &HeaderMap::new(), true).is_ok());
        assert!(
            policy
                .check(
                    &uri,
                    &headers(&[("origin", "https://localhost:7878")]),
                    true
                )
                .is_ok()
        );
        let foreign = Uri::from_static("https://evil.example:7878/api/rpc");
        assert!(policy.check(&foreign, &HeaderMap::new(), true).is_err());
        // An authority and a Host header that disagree are refused outright.
        assert!(
            policy
                .check(&uri, &headers(&[("host", "127.0.0.1:7878")]), true)
                .is_err()
        );
        assert!(
            policy
                .check(&uri, &headers(&[("host", "localhost:7878")]), true)
                .is_ok()
        );
    }

    #[test]
    fn refusals_explain_themselves_without_echoing_control_characters() {
        let policy = policy(Scheme::Http, "127.0.0.1:7878");
        let refusal = policy
            .check(
                &Uri::from_static("/"),
                &headers(&[("host", "evil.example:7878")]),
                false,
            )
            .unwrap_err();
        let text = format!("{refusal:?}");
        assert!(text.contains("evil.example:7878"), "{text}");
        assert!(text.contains("http://127.0.0.1:7878"), "{text}");
        assert_eq!(printable(&"x".repeat(300)).chars().count(), 101);
    }
}
