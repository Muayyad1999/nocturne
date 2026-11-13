//! `call` and `methods`: a small client for shells and scripts.
//!
//! The result goes to standard output as JSON and nothing else does, so the
//! output can be piped straight into `jq`. Failures are one line on standard
//! error, `error [code]: message`, and an exit status that tells an
//! application error (1) from a call that never arrived (2).

// `Failure` carries the error code, the message and the details the server
// sent, so it is a wide type. This is a command-line client: it builds at
// most one per run, on its way to printing it and exiting.
#![allow(clippy::result_large_err)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use nocturne_server::{data_dir, instance, tls, token};
use reqwest::{StatusCode, Url};
use serde_json::{Value, json};

use super::{CallArgs, ClientArgs, NOT_DELIVERED, REFUSED, print_line};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Why a command failed, ready to be printed.
#[derive(Debug, PartialEq)]
struct Failure {
    code: String,
    message: String,
    details: Option<Value>,
    exit: u8,
}

impl Failure {
    /// The call did not get as far as the application.
    fn not_delivered(code: &str, message: impl Into<String>) -> Failure {
        Failure {
            code: code.to_string(),
            message: message.into(),
            details: None,
            exit: NOT_DELIVERED,
        }
    }

    fn report(&self) -> ExitCode {
        eprintln!("error [{}]: {}", self.code, self.message);
        if let Some(details) = &self.details {
            eprintln!("{}", pretty(details));
        }
        ExitCode::from(self.exit)
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

pub(super) fn call(args: CallArgs) -> ExitCode {
    let outcome = read_params(args.params.as_deref()).and_then(|params| {
        let mut body = json!({"method": args.method});
        if let Some(params) = params {
            body["params"] = params;
        }
        run(&args.client, |server| async move {
            server
                .request(reqwest::Method::POST, "api/rpc", Some(body))
                .await
        })
    });
    finish(outcome, |result| print_line(&pretty(&result)))
}

pub(super) fn methods(args: ClientArgs) -> ExitCode {
    let outcome = run(&args, |server| async move {
        let info = server
            .request(reqwest::Method::GET, "api/info", None)
            .await?;
        info.get("methods")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| {
                Failure::not_delivered("internal", "the server's reply lists no methods")
            })
    });
    finish(outcome, |methods| {
        for method in methods.iter().filter_map(Value::as_str) {
            print_line(method);
        }
    })
}

fn finish<T>(outcome: Result<T, Failure>, print: impl FnOnce(T)) -> ExitCode {
    match outcome {
        Ok(value) => {
            print(value);
            ExitCode::SUCCESS
        }
        Err(failure) => failure.report(),
    }
}

/// The parameters given on the command line: absent, JSON, or `-` for standard input.
fn read_params(argument: Option<&str>) -> Result<Option<Value>, Failure> {
    let text = match argument {
        None => return Ok(None),
        Some("-") => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text).map_err(|err| {
                Failure::not_delivered(
                    "invalid",
                    format!("could not read the parameters from standard input: {err}"),
                )
            })?;
            text
        }
        Some(text) => text.to_string(),
    };
    parse_params(&text).map(Some)
}

fn parse_params(text: &str) -> Result<Value, Failure> {
    serde_json::from_str(text).map_err(|err| {
        Failure::not_delivered(
            "invalid",
            format!("the parameters are not valid JSON: {err}"),
        )
    })
}

/// Find the server, then run `command` against it.
fn run<T, F, Fut>(args: &ClientArgs, command: F) -> Result<T, Failure>
where
    F: FnOnce(Server) -> Fut,
    Fut: Future<Output = Result<T, Failure>>,
{
    let server = Server::locate(args)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| {
            Failure::not_delivered(
                "internal",
                format!("could not start the async runtime: {err}"),
            )
        })?;
    runtime.block_on(command(server))
}

/// Where the server is and how to talk to it.
struct Server {
    base: Url,
    token: String,
    http: reqwest::Client,
}

/// What the command line and the data directory say about the server.
#[derive(Debug, PartialEq)]
struct Target {
    base: Url,
    token: String,
}

impl Server {
    fn locate(args: &ClientArgs) -> Result<Server, Failure> {
        let data_dir = args
            .data_dir
            .clone()
            .filter(|dir| !dir.as_os_str().is_empty())
            .or_else(data_dir::default_path);
        let target = resolve(
            args.url.as_deref(),
            args.token.as_deref(),
            std::env::var("NOCTURNE_TOKEN").ok().as_deref(),
            data_dir.as_deref(),
        )?;

        // reqwest is built without a TLS provider of its own; this process uses ring.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut http = reqwest::Client::builder()
            // The server is on this machine or this network: never through a proxy.
            .no_proxy()
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("nocturne-server/", env!("CARGO_PKG_VERSION")));
        if args.insecure {
            http = http.tls_danger_accept_invalid_certs(true);
        } else if let Some(certificate) = data_dir.as_deref().and_then(own_certificate) {
            // A server started with --tls-self-signed on this data directory is trusted as is.
            http = http.tls_certs_merge([certificate]);
        }
        let http = http.build().map_err(|err| {
            Failure::not_delivered(
                "internal",
                format!("could not set up the HTTP client: {err}"),
            )
        })?;
        Ok(Server {
            base: target.base,
            token: target.token,
            http,
        })
    }

    /// One request; the unwrapped `result` of the reply envelope, or for
    /// routes without an envelope the reply itself.
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, Failure> {
        let url = self
            .base
            .join(path)
            .map_err(|err| Failure::not_delivered("invalid", format!("bad address: {err}")))?;
        let mut request = self.http.request(method, url).bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|err| self.unreachable(&err))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|err| self.unreachable(&err))?;
        interpret(status, &bytes, &self.base)
    }

    fn unreachable(&self, err: &reqwest::Error) -> Failure {
        let cause = root_cause(err);
        let hint = if cause.to_ascii_lowercase().contains("certificate") {
            "; the server's TLS certificate is not trusted here: pass --insecure to skip the \
             check, or run this on the server's machine"
        } else if err.is_connect() {
            "; is nocturne-server running? (an instance file can be left over from a server \
             that was killed)"
        } else {
            ""
        };
        Failure::not_delivered(
            "unavailable",
            format!("could not reach {}: {cause}{hint}", self.base),
        )
    }
}

/// The innermost error: reqwest's own messages only say that sending failed.
fn root_cause(err: &dyn std::error::Error) -> String {
    let mut cause = err;
    while let Some(source) = cause.source() {
        cause = source;
    }
    cause.to_string()
}

/// The certificate a `--tls-self-signed` server on `data_dir` presents, if there is one.
fn own_certificate(data_dir: &Path) -> Option<reqwest::Certificate> {
    let pem = std::fs::read(data_dir.join(tls::CERT_FILE)).ok()?;
    reqwest::Certificate::from_pem(&pem).ok()
}

/// Decide which server to talk to and with which token.
///
/// The address is `--url`, else the instance file. The token is `--token`,
/// else the `#token=` fragment of the address (the link the server prints can
/// be passed as is), else `NOCTURNE_TOKEN`, else the token file.
fn resolve(
    url: Option<&str>,
    token_flag: Option<&str>,
    token_env: Option<&str>,
    data_dir: Option<&Path>,
) -> Result<Target, Failure> {
    let url = match url {
        Some(url) => url.to_string(),
        None => recorded_url(data_dir)?,
    };
    let mut base = Url::parse(&url).map_err(|err| {
        Failure::not_delivered("invalid", format!("`{url}` is not a usable address: {err}"))
    })?;
    if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
        return Err(Failure::not_delivered(
            "invalid",
            format!("`{url}` is not a usable address: it must start with http:// or https://"),
        ));
    }
    let in_fragment = base
        .fragment()
        .and_then(|fragment| fragment.strip_prefix("token="))
        .map(str::to_string);
    base.set_fragment(None);
    base.set_query(None);
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }

    let given = [token_flag, in_fragment.as_deref(), token_env]
        .into_iter()
        .flatten()
        .find(|token| !token.is_empty())
        .map(str::to_string);
    let token = match given {
        Some(token) => token,
        None => stored_token(data_dir)?,
    };
    Ok(Target { base, token })
}

fn recorded_url(data_dir: Option<&Path>) -> Result<String, Failure> {
    let Some(data_dir) = data_dir else {
        return Err(Failure::not_delivered(
            "unavailable",
            "no server address: pass --url (this system has no default data directory to look in)",
        ));
    };
    match instance::find(data_dir) {
        Ok(Some(instance)) => Ok(instance.url),
        Ok(None) => Err(Failure::not_delivered(
            "unavailable",
            format!(
                "no running server found: `{}` does not exist. Start one with `nocturne-server \
                 serve`, or name a server with --url",
                data_dir.join(instance::FILE_NAME).display()
            ),
        )),
        Err(err) => Err(Failure::not_delivered("unavailable", format!("{err:#}"))),
    }
}

fn stored_token(data_dir: Option<&Path>) -> Result<String, Failure> {
    let no_token = |place: PathBuf| {
        Failure::not_delivered(
            "unauthorized",
            format!(
                "no access token: pass --token, set NOCTURNE_TOKEN, or run this where the \
                 server keeps its token (`{}` does not exist)",
                place.display()
            ),
        )
    };
    let Some(data_dir) = data_dir else {
        return Err(no_token(PathBuf::from(token::FILE_NAME)));
    };
    match token::load(data_dir) {
        Ok(Some(token)) => Ok(token),
        Ok(None) => Err(no_token(data_dir.join(token::FILE_NAME))),
        Err(err) => Err(Failure::not_delivered("unauthorized", format!("{err:#}"))),
    }
}

/// Turn a reply into the value to print or the failure to report.
fn interpret(status: StatusCode, body: &[u8], server: &Url) -> Result<Value, Failure> {
    let not_nocturne = || {
        Failure::not_delivered(
            "unavailable",
            format!(
                "{server} answered HTTP {} with something that is not a Nocturne reply; is that \
                 address a nocturne-server?",
                status.as_u16()
            ),
        )
    };
    let Ok(Value::Object(mut reply)) = serde_json::from_slice::<Value>(body) else {
        return Err(not_nocturne());
    };
    match reply.get("ok") {
        Some(Value::Bool(true)) if status == StatusCode::OK => {
            Ok(reply.remove("result").unwrap_or(Value::Null))
        }
        Some(Value::Bool(false)) => {
            let error = reply.get("error");
            let text = |field: &str| {
                error
                    .and_then(|error| error.get(field))
                    .and_then(Value::as_str)
            };
            let (Some(code), Some(message)) = (text("code"), text("message")) else {
                return Err(not_nocturne());
            };
            Err(Failure {
                code: code.to_string(),
                message: message.to_string(),
                details: error
                    .and_then(|error| error.get("details"))
                    .filter(|details| !details.is_null())
                    .cloned(),
                // 200 means the application itself said no; anything else stopped short of it.
                exit: if status == StatusCode::OK {
                    REFUSED
                } else {
                    NOT_DELIVERED
                },
            })
        }
        // Routes like /api/info answer without an envelope.
        None if status == StatusCode::OK => Ok(Value::Object(reply)),
        _ => Err(not_nocturne()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Url {
        Url::parse("http://127.0.0.1:7878/").unwrap()
    }

    #[test]
    fn replies_are_interpreted_by_envelope_and_status() {
        let ok = interpret(
            StatusCode::OK,
            br#"{"ok": true, "result": [1, 2]}"#,
            &server(),
        );
        assert_eq!(ok, Ok(json!([1, 2])));
        let null = interpret(StatusCode::OK, br#"{"ok": true}"#, &server());
        assert_eq!(null, Ok(Value::Null));
        let info = interpret(
            StatusCode::OK,
            br#"{"version": "1", "methods": []}"#,
            &server(),
        );
        assert_eq!(info, Ok(json!({"version": "1", "methods": []})));

        let refused = interpret(
            StatusCode::OK,
            br#"{"ok": false, "error": {"code": "not_found", "message": "device `x` was not found"}}"#,
            &server(),
        )
        .unwrap_err();
        assert_eq!(
            refused,
            Failure {
                code: "not_found".into(),
                message: "device `x` was not found".into(),
                details: None,
                exit: 1
            }
        );

        let detailed = interpret(
            StatusCode::OK,
            br#"{"ok": false, "error": {"code": "conflict", "message": "taken", "details": {"name": "sw1"}}}"#,
            &server(),
        )
        .unwrap_err();
        assert_eq!(detailed.details, Some(json!({"name": "sw1"})));
        assert_eq!(detailed.exit, 1);

        let unauthorized = interpret(
            StatusCode::UNAUTHORIZED,
            br#"{"ok": false, "error": {"code": "unauthorized", "message": "no"}}"#,
            &server(),
        )
        .unwrap_err();
        assert_eq!(
            (unauthorized.code.as_str(), unauthorized.exit),
            ("unauthorized", 2)
        );
    }

    #[test]
    fn replies_from_something_else_are_called_out() {
        for (status, body) in [
            (StatusCode::OK, &b"<html>hello</html>"[..]),
            (StatusCode::OK, b"[1, 2]"),
            (StatusCode::NOT_FOUND, b"not found"),
            (StatusCode::BAD_GATEWAY, b""),
            (StatusCode::OK, br#"{"ok": false}"#),
            (StatusCode::OK, br#"{"ok": false, "error": {"code": 5}}"#),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                br#"{"ok": true, "result": 1}"#,
            ),
            (StatusCode::NOT_FOUND, br#"{"detail": "nope"}"#),
        ] {
            let failure = interpret(status, body, &server()).unwrap_err();
            assert_eq!(
                failure.exit,
                2,
                "{status} {:?}",
                String::from_utf8_lossy(body)
            );
            assert_eq!(failure.code, "unavailable");
            assert!(
                failure.message.contains(&status.as_u16().to_string()),
                "{}",
                failure.message
            );
        }
    }

    #[test]
    fn the_address_comes_from_the_flag_or_the_instance_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = resolve(None, Some("t"), None, Some(dir.path())).unwrap_err();
        assert_eq!((missing.code.as_str(), missing.exit), ("unavailable", 2));
        assert!(
            missing.message.contains("server.json"),
            "{}",
            missing.message
        );

        std::fs::write(
            dir.path().join("server.json"),
            r#"{"url": "http://127.0.0.1:4242/", "pid": 1}"#,
        )
        .unwrap();
        let recorded = resolve(None, Some("t"), None, Some(dir.path())).unwrap();
        assert_eq!(recorded.base.as_str(), "http://127.0.0.1:4242/");

        let explicit = resolve(
            Some("https://noc.example:8443"),
            Some("t"),
            None,
            Some(dir.path()),
        );
        assert_eq!(explicit.unwrap().base.as_str(), "https://noc.example:8443/");

        // A path prefix (a reverse proxy) is kept; query and fragment are not part of the base.
        let prefixed = resolve(
            Some("https://noc.example/nocturne?x=1#y"),
            Some("t"),
            None,
            None,
        );
        assert_eq!(
            prefixed.unwrap().base.as_str(),
            "https://noc.example/nocturne/"
        );

        for bad in ["127.0.0.1:7878", "ftp://host/", "not a url", ""] {
            let failure = resolve(Some(bad), Some("t"), None, None).unwrap_err();
            assert_eq!(
                (failure.code.as_str(), failure.exit),
                ("invalid", 2),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_token_comes_from_the_most_specific_place() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("server.token"), "from-file\n").unwrap();
        let link = "http://127.0.0.1:7878/#token=from-link";
        let plain = "http://127.0.0.1:7878/";
        let token = |url, flag, env| {
            resolve(Some(url), flag, env, Some(dir.path()))
                .unwrap()
                .token
        };

        assert_eq!(
            token(link, Some("from-flag"), Some("from-env")),
            "from-flag"
        );
        assert_eq!(token(link, None, Some("from-env")), "from-link");
        assert_eq!(token(plain, None, Some("from-env")), "from-env");
        assert_eq!(token(plain, None, None), "from-file");
        assert_eq!(
            token(plain, Some(""), Some("")),
            "from-file",
            "empty values count as absent"
        );
        // The link's token is not sent as part of the address.
        assert_eq!(
            resolve(Some(link), None, None, None).unwrap().base.as_str(),
            "http://127.0.0.1:7878/"
        );

        let empty = tempfile::tempdir().unwrap();
        let none = resolve(Some(plain), None, None, Some(empty.path())).unwrap_err();
        assert_eq!((none.code.as_str(), none.exit), ("unauthorized", 2));
        assert!(none.message.contains("--token"), "{}", none.message);
        // Looking for a token never creates one.
        assert!(!empty.path().join("server.token").exists());
    }

    #[test]
    fn parameters_must_be_json() {
        assert_eq!(parse_params(r#"{"id": "x"}"#), Ok(json!({"id": "x"})));
        assert_eq!(parse_params("[1]"), Ok(json!([1])));
        let failure = parse_params("{id: x}").unwrap_err();
        assert_eq!((failure.code.as_str(), failure.exit), ("invalid", 2));
        assert_eq!(read_params(None), Ok(None));
    }
}
