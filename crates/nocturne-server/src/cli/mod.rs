//! Command line of the `nocturne-server` binary.

mod browser;
mod client;
mod serve;
mod signals;

use std::io::{self, Write};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::{Args, Parser, Subcommand};
use nocturne_server::{DEFAULT_PORT, data_dir, token};

/// Exit status of `call` and `methods` when the application refused the call.
const REFUSED: u8 = 1;
/// Exit status when a call never reached the application: wrong usage, no
/// server, no connection, wrong token.
const NOT_DELIVERED: u8 = 2;

#[derive(Parser)]
#[command(
    name = "nocturne-server",
    version,
    about = "Nocturne without the desktop window: the same backend, used from a browser.",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    serve: ServeArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server (also what happens when no command is given)
    Serve(ServeArgs),
    /// Call one RPC method of a running server and print the result as JSON
    #[command(after_help = EXIT_STATUS)]
    Call(CallArgs),
    /// List the RPC methods of a running server
    #[command(after_help = EXIT_STATUS)]
    Methods(ClientArgs),
    /// Print the access token, creating it if there is none yet
    Token(TokenArgs),
}

const EXIT_STATUS: &str = "Exit status:\n  0  success, the result is on standard output\n  1  \
                           the application refused the call (`error [code]: message` on standard \
                           error)\n  2  the call did not reach the application: wrong usage, no \
                           server, no connection or a wrong token";

#[derive(Args)]
struct ServeArgs {
    /// Where Nocturne keeps its data [default: the platform's data directory + "Nocturne"]
    #[arg(long, value_name = "DIR", env = data_dir::ENV)]
    data_dir: Option<PathBuf>,

    /// Address to listen on, optionally with a port (0.0.0.0, ::1, 192.0.2.10:8443)
    #[arg(long, value_name = "ADDR", default_value = "127.0.0.1", value_parser = parse_bind)]
    bind: Bind,

    /// Port to listen on; 0 lets the system pick a free one [default: 7878]
    #[arg(long, value_name = "N")]
    port: Option<u16>,

    /// Access token [default: the content of <data-dir>/server.token, created on first start].
    /// Other users of this machine can see command lines: prefer the environment variable
    #[arg(long, value_name = "T", env = "NOCTURNE_TOKEN", hide_env_values = true)]
    token: Option<String>,

    /// Serve HTTPS with this certificate chain (PEM)
    #[arg(
        long,
        value_name = "F",
        requires = "tls_key",
        conflicts_with = "tls_self_signed"
    )]
    tls_cert: Option<PathBuf>,

    /// The private key of --tls-cert (PEM)
    #[arg(
        long,
        value_name = "F",
        requires = "tls_cert",
        conflicts_with = "tls_self_signed"
    )]
    tls_key: Option<PathBuf>,

    /// Serve HTTPS with a self-signed certificate kept in the data directory
    #[arg(long)]
    tls_self_signed: bool,

    /// Accept plain HTTP on an address other machines can reach (the token and device
    /// passwords then cross the network unencrypted)
    #[arg(long)]
    allow_insecure_http: bool,

    /// Serve the web UI from this directory instead of the built-in one (for UI development)
    #[arg(long, value_name = "DIR")]
    ui_dir: Option<PathBuf>,

    /// Open the web UI in the default browser once the server is up
    #[arg(long)]
    open: bool,

    /// How much to log to standard error: error, warn, info, debug, trace, or a filter such
    /// as "info,nocturne_server=debug"
    #[arg(long, value_name = "L", default_value = "info")]
    log_level: String,
}

#[derive(Args)]
struct ClientArgs {
    /// Address of the server [default: the server running on the data directory]
    #[arg(long, value_name = "URL")]
    url: Option<String>,

    /// Access token [default: the `#token=` part of --url, else $NOCTURNE_TOKEN, else the
    /// content of <data-dir>/server.token]
    #[arg(long, value_name = "T")]
    token: Option<String>,

    /// The server's data directory [default: the platform's data directory + "Nocturne"]
    #[arg(long, value_name = "DIR", env = data_dir::ENV)]
    data_dir: Option<PathBuf>,

    /// Do not verify the server's TLS certificate
    #[arg(long)]
    insecure: bool,
}

#[derive(Args)]
struct CallArgs {
    /// The method to call, for example devices.list
    method: String,

    /// Its parameters as JSON; `-` reads them from standard input
    params: Option<String>,

    #[command(flatten)]
    client: ClientArgs,
}

#[derive(Args)]
struct TokenArgs {
    /// Where Nocturne keeps its data [default: the platform's data directory + "Nocturne"]
    #[arg(long, value_name = "DIR", env = data_dir::ENV)]
    data_dir: Option<PathBuf>,
}

/// What `--bind` named.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Bind {
    ip: IpAddr,
    port: Option<u16>,
}

fn parse_bind(text: &str) -> Result<Bind, String> {
    let localhost = |port| Bind {
        ip: IpAddr::from([127, 0, 0, 1]),
        port,
    };
    if text.eq_ignore_ascii_case("localhost") {
        return Ok(localhost(None));
    }
    if let Some((name, port)) = text.rsplit_once(':')
        && name.eq_ignore_ascii_case("localhost")
        && let Ok(port) = port.parse()
    {
        return Ok(localhost(Some(port)));
    }
    if let Ok(ip) = text.parse::<IpAddr>() {
        return Ok(Bind { ip, port: None });
    }
    let bracketed = text
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .and_then(|inner| inner.parse::<Ipv6Addr>().ok());
    if let Some(ip) = bracketed {
        return Ok(Bind {
            ip: ip.into(),
            port: None,
        });
    }
    if let Ok(address) = text.parse::<SocketAddr>() {
        return Ok(Bind {
            ip: address.ip(),
            port: Some(address.port()),
        });
    }
    Err(format!(
        "`{text}` is not an IP address; use for example 127.0.0.1, 0.0.0.0, ::1 or 192.0.2.10:8443"
    ))
}

impl ServeArgs {
    fn bind_address(&self) -> anyhow::Result<SocketAddr> {
        let port = match (self.bind.port, self.port) {
            (Some(in_bind), Some(port)) if in_bind != port => {
                bail!("--bind names port {in_bind} but --port says {port}; give the port only once")
            }
            (Some(port), _) | (None, Some(port)) => port,
            (None, None) => DEFAULT_PORT,
        };
        Ok(SocketAddr::new(self.bind.ip, port))
    }
}

/// The data directory to use: the one given, else the platform default.
fn resolve_data_dir(explicit: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    explicit
        .filter(|dir| !dir.as_os_str().is_empty())
        .or_else(data_dir::default_path)
        .context(
            "cannot tell where Nocturne's data directory is on this system; name it with --data-dir",
        )
}

pub(crate) fn run() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        None => serve::run(cli.serve),
        Some(Command::Serve(args)) => serve::run(args),
        Some(Command::Call(args)) => client::call(args),
        Some(Command::Methods(args)) => client::methods(args),
        Some(Command::Token(args)) => print_token(args),
    }
}

fn print_token(args: TokenArgs) -> ExitCode {
    let token = resolve_data_dir(args.data_dir).and_then(|dir| {
        data_dir::ensure(&dir)?;
        token::load_or_create(&dir)
    });
    match token {
        Ok(token) => {
            print_line(&token);
            ExitCode::SUCCESS
        }
        Err(err) => fail(&err),
    }
}

/// Report a failure that is not an RPC error and pick the exit status for it.
fn fail(err: &anyhow::Error) -> ExitCode {
    eprintln!("error: {err:#}");
    ExitCode::FAILURE
}

/// Write one line to standard output. A reader that went away (`| head`) is not an error.
fn print_line(text: &str) {
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "{text}").and_then(|()| stdout.flush());
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("nocturne-server").chain(args.iter().copied()))
    }

    fn serve_args(args: &[&str]) -> ServeArgs {
        let cli = parse(args).unwrap_or_else(|err| panic!("{err}"));
        match cli.command {
            None => cli.serve,
            Some(Command::Serve(args)) => args,
            Some(_) => panic!("not a serve command"),
        }
    }

    #[test]
    fn the_command_line_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn serve_is_the_default_command() {
        for args in [&[][..], &["serve"][..]] {
            let serve = serve_args(args);
            assert_eq!(serve.bind_address().unwrap().to_string(), "127.0.0.1:7878");
            assert!(!serve.allow_insecure_http && !serve.tls_self_signed && !serve.open);
            assert_eq!(serve.log_level, "info");
        }
        let with_flags = serve_args(&["--port", "0", "--bind", "0.0.0.0", "--tls-self-signed"]);
        assert_eq!(with_flags.bind_address().unwrap().to_string(), "0.0.0.0:0");
        assert!(with_flags.tls_self_signed);
        let explicit = serve_args(&["serve", "--port", "9000", "--allow-insecure-http"]);
        assert_eq!(
            explicit.bind_address().unwrap().to_string(),
            "127.0.0.1:9000"
        );
        assert!(explicit.allow_insecure_http);
    }

    #[test]
    fn bind_addresses() {
        let cases = [
            ("127.0.0.1", "127.0.0.1", None),
            ("localhost", "127.0.0.1", None),
            ("LocalHost:9000", "127.0.0.1", Some(9000)),
            ("0.0.0.0", "0.0.0.0", None),
            ("::", "::", None),
            ("::1", "::1", None),
            ("[::1]", "::1", None),
            ("[::1]:8443", "::1", Some(8443)),
            ("192.0.2.10:8443", "192.0.2.10", Some(8443)),
        ];
        for (text, ip, port) in cases {
            assert_eq!(
                parse_bind(text),
                Ok(Bind {
                    ip: ip.parse().unwrap(),
                    port
                }),
                "{text}"
            );
        }
        for bad in [
            "",
            "example.com",
            "127.0.0.1:",
            "127.0.0.1:99999",
            "1.2.3",
            "[::1",
        ] {
            let err = parse_bind(bad).unwrap_err();
            assert!(err.contains("not an IP address"), "{bad:?}: {err}");
        }

        assert_eq!(
            serve_args(&["--bind", "[::1]:8443"])
                .bind_address()
                .unwrap()
                .to_string(),
            "[::1]:8443"
        );
        assert_eq!(
            serve_args(&["--bind", "[::1]:8443", "--port", "8443"])
                .bind_address()
                .unwrap()
                .port(),
            8443
        );
        let clash = serve_args(&["--bind", "127.0.0.1:1", "--port", "2"])
            .bind_address()
            .unwrap_err();
        assert!(clash.to_string().contains("only once"), "{clash}");
    }

    #[test]
    fn tls_flags_must_make_sense_together() {
        assert!(parse(&["--tls-cert", "c.pem"]).is_err());
        assert!(parse(&["--tls-key", "k.pem"]).is_err());
        assert!(
            parse(&[
                "--tls-cert",
                "c.pem",
                "--tls-key",
                "k.pem",
                "--tls-self-signed"
            ])
            .is_err()
        );
        let both = serve_args(&["--tls-cert", "c.pem", "--tls-key", "k.pem"]);
        assert_eq!(both.tls_cert.unwrap(), PathBuf::from("c.pem"));
        assert_eq!(both.tls_key.unwrap(), PathBuf::from("k.pem"));
    }

    #[test]
    fn client_commands() {
        let cli = parse(&[
            "call",
            "devices.get",
            r#"{"id": "x"}"#,
            "--url",
            "https://noc.example:8443/",
            "--token",
            "t",
            "--insecure",
        ])
        .unwrap_or_else(|err| panic!("{err}"));
        let Some(Command::Call(call)) = cli.command else {
            panic!("not a call");
        };
        assert_eq!(call.method, "devices.get");
        assert_eq!(call.params.as_deref(), Some(r#"{"id": "x"}"#));
        assert_eq!(
            call.client.url.as_deref(),
            Some("https://noc.example:8443/")
        );
        assert_eq!(call.client.token.as_deref(), Some("t"));
        assert!(call.client.insecure);

        let Some(Command::Call(stdin)) = parse(&["call", "devices.create", "-"]).unwrap().command
        else {
            panic!("not a call");
        };
        assert_eq!(stdin.params.as_deref(), Some("-"));

        assert!(matches!(
            parse(&["methods", "--url", "http://127.0.0.1:1/"])
                .unwrap()
                .command,
            Some(Command::Methods(_))
        ));
        assert!(matches!(
            parse(&["token", "--data-dir", "/tmp/x"]).unwrap().command,
            Some(Command::Token(_))
        ));
        assert!(parse(&["call"]).is_err(), "call needs a method");
        assert!(
            parse(&["--port", "1", "token"]).is_err(),
            "serve flags do not mix with commands"
        );
    }
}
