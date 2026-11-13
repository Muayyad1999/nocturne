//! Logs to stderr and to a file in the data directory.
//!
//! Nothing here may record what a method was asked or what it answered: RPC
//! parameters and results carry device configurations and, briefly, secrets.
//! Method names and outcomes are enough to follow what happened.

use std::path::Path;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Start logging. Returns the guard that must live as long as the program:
/// dropping it stops the file writer.
pub fn start(data_dir: &Path, level: Option<&str>) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let filter = || {
        EnvFilter::try_from_env("NOCTURNE_LOG").unwrap_or_else(|_| {
            EnvFilter::new(level.unwrap_or("info"))
        })
    };
    let stderr = tracing_subscriber::fmt::layer().with_writer(std::io::stderr).with_filter(filter());
    use tracing_subscriber::layer::Layer;

    let logs = data_dir.join("logs");
    match std::fs::create_dir_all(&logs) {
        Ok(()) => {
            let appender = tracing_appender::rolling::Builder::new()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix("nocturne")
                .filename_suffix("log")
                .max_log_files(5)
                .build(&logs)
                .ok();
            match appender {
                Some(appender) => {
                    let (writer, guard) = tracing_appender::non_blocking(appender);
                    let file = tracing_subscriber::fmt::layer()
                        .with_writer(writer)
                        .with_ansi(false)
                        .with_filter(filter());
                    tracing_subscriber::registry().with(stderr).with(file).init();
                    Some(guard)
                }
                None => {
                    tracing_subscriber::registry().with(stderr).init();
                    None
                }
            }
        }
        Err(err) => {
            tracing_subscriber::registry().with(stderr).init();
            tracing::warn!(%err, path = %logs.display(), "no log file: logging to the terminal only");
            None
        }
    }
}
