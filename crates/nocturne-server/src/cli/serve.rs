//! `nocturne-server [serve]`: run until told to stop.

use std::io::IsTerminal;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use nocturne_server::{ServerOptions, Tls, data_dir, instance, tls, token};
use nocturne_suite::AppConfig;
use tracing_subscriber::EnvFilter;

use super::browser::Launcher;
use super::signals::Signals;
use super::{NOT_DELIVERED, ServeArgs, fail, print_line, resolve_data_dir};

/// Background work the application could not stop by itself gets this long
/// before the process exits anyway.
const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(2);

pub(super) fn run(args: ServeArgs) -> ExitCode {
    if let Err(err) = init_logging(&args.log_level) {
        eprintln!("error: {err:#}");
        return ExitCode::from(NOT_DELIVERED);
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            return fail(&anyhow::Error::new(err).context("could not start the async runtime"));
        }
    };
    let outcome = runtime.block_on(serve(args));
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => fail(&err),
    }
}

/// Logs go to standard error; standard output carries only the link.
fn init_logging(level: &str) -> anyhow::Result<()> {
    let filter = EnvFilter::try_new(level).with_context(|| {
        format!("`{level}` is not a log level; use error, warn, info, debug or trace")
    })?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .try_init()
        .map_err(|err| anyhow::anyhow!("could not set up logging: {err}"))
}

async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    let bind = args.bind_address()?;
    let data_dir = resolve_data_dir(args.data_dir)?;
    data_dir::ensure(&data_dir)?;

    // Before anything is announced: a stop request must never hit the default handler.
    let mut signals = Signals::install().context("could not listen for stop requests")?;

    let token = match args.token.filter(|token| !token.is_empty()) {
        Some(token) => token,
        None => token::load_or_create(&data_dir)?,
    };
    let tls = match (args.tls_cert, args.tls_key) {
        (Some(cert), Some(key)) => Some(Tls::Files { cert, key }),
        _ if args.tls_self_signed => {
            let certificate = tls::self_signed(&data_dir, bind.ip())?;
            // Not a log line: it must be visible whatever the log level is.
            eprintln!(
                "TLS certificate ({}) SHA-256 fingerprint: {}",
                if certificate.created {
                    "self-signed, newly created"
                } else {
                    "self-signed"
                },
                certificate.fingerprint
            );
            Some(certificate.into())
        }
        _ => None,
    };
    let options = ServerOptions {
        bind,
        token,
        tls,
        allow_insecure_http: args.allow_insecure_http,
        ui_dir: args.ui_dir,
    };
    // Refuse a dangerous configuration before the database is even opened.
    options.validate()?;
    let serves_built_in_ui = options.ui_dir.is_none();

    // `remote_users`: whoever calls this server may sit anywhere (a loopback
    // port is one SSH tunnel away from the network), so the application
    // withholds what only makes sense at the machine itself.
    let app = nocturne_suite::start(AppConfig::at(&data_dir).remote_users())
        .context("could not start Nocturne")?;
    let server = match nocturne_server::serve(app.clone(), options).await {
        Ok(server) => server,
        Err(err) => {
            app.shutdown();
            return Err(err);
        }
    };
    let _instance = instance::publish(&data_dir, &server.base_url())?;
    print_line(&format!("Nocturne is listening on {}", server.url()));

    if serves_built_in_ui && !nocturne_server::ui_is_embedded() {
        tracing::warn!(
            "this binary was built without the web UI and serves a placeholder page; build the \
             UI with `pnpm --dir ui build` and compile nocturne-server again (the API works)"
        );
    }
    let _launcher = args
        .open
        .then(|| Launcher::open(&data_dir, &server.url()))
        .flatten();

    let signal = signals.wait().await;
    tracing::info!(signal, "stopping");
    app.shutdown();
    tokio::select! {
        _ = server.shutdown() => {}
        signal = signals.wait() => tracing::warn!(signal, "asked twice: not waiting for requests to finish"),
    }
    Ok(())
}
