//! `tftp.start` / `tftp.stop` / `tftp.status` — a TFTP server for gear that
//! speaks nothing else.
//!
//! The server is `nocturne_netprobe::tftp`, which confines every transfer to
//! the root directory (absolute paths, `..`, symbolic links out of the root
//! and device names are refused) and runs each transfer on its own port. This
//! module owns its lifetime: a job holds it, `tftp.stop` cancels that job, the
//! transfers are reported as `tftp.transfer` events and kept for `tftp.status`,
//! and the socket closes when the job ends — including at shutdown.
//!
//! Writing is off unless it is asked for, and overwriting is off unless that is
//! asked for too. A switch being staged should be able to fetch an image; it
//! should not be able to replace one.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use nocturne_app::jobs::{self, JobSpec};
use nocturne_app::{App, AppError, AppResult, NoParams, Router, util};
use nocturne_netprobe::tftp::{TftpConfig, TftpEvent, TftpServer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::errors::probe as probe_error;
use crate::probe::history::{self, Run};
use crate::settings::ToolboxSettings;
use crate::state::State;

/// Transfers kept for `tftp.status`. A staging session is a handful; a device
/// in a retry loop is not going to fill memory with them.
const KEPT_TRANSFERS: usize = 500;

/// A running TFTP server, as the rest of the application sees it.
#[derive(Clone, Debug, Serialize)]
pub struct Running {
    pub address: String,
    pub root: String,
    pub allow_write: bool,
    pub allow_overwrite: bool,
    pub job_id: String,
    pub run_id: String,
    pub started_at: i64,
    /// What has happened on it, newest last.
    pub transfers: Vec<TftpEvent>,
    /// Transfers dropped from the front of `transfers`.
    pub dropped: usize,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct StartParams {
    /// The directory to serve. Left out, the one in the settings.
    pub root: Option<String>,
    /// Left out, the port in the settings (6969).
    pub port: Option<u16>,
    /// The address to bind; left out, the one in the settings.
    pub bind: Option<String>,
    /// Accept uploads from devices.
    pub allow_write: bool,
    /// Let an upload replace a file that is already there.
    pub allow_overwrite: bool,
}

#[derive(Debug, Serialize)]
pub struct Started {
    /// Where it is listening, with the port it actually got.
    pub address: String,
    pub root: String,
    pub allow_write: bool,
    pub allow_overwrite: bool,
    pub job_id: String,
    pub run_id: String,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub running: bool,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub server: Option<Running>,
    /// The defaults a `tftp.start` without parameters would use.
    pub defaults: Defaults,
}

#[derive(Debug, Serialize)]
pub struct Defaults {
    pub bind: String,
    pub port: u16,
    pub root: String,
}

#[derive(Debug, Serialize)]
pub struct Stopped {
    pub stopped: bool,
}

pub(crate) fn bind_address(bind: &str, port: u16) -> AppResult<SocketAddr> {
    let host = bind.trim();
    let host: IpAddr = host.parse().map_err(|_| {
        AppError::invalid(format!(
            "`{host}` is not an address this machine can listen on; use `0.0.0.0` for every interface or one of its own addresses"
        ))
    })?;
    Ok(SocketAddr::new(host, port))
}

async fn start(app: &App, params: StartParams) -> AppResult<Started> {
    app.require_local_access("the TFTP server")?;
    let settings = ToolboxSettings::load(app).tftp;
    let state = app.state::<State>();
    if let Some(running) = state.tftp.lock().clone() {
        return Err(AppError::conflict(format!(
            "a TFTP server is already running on {} serving {}; stop it first",
            running.address, running.root
        )));
    }

    let root = params
        .root
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(&settings.root)
        .to_string();
    if root.is_empty() {
        return Err(AppError::invalid(
            "say which directory to serve: `root`, or set one under Settings → Toolbox",
        ));
    }
    let path = PathBuf::from(&root);
    if !path.is_dir() {
        return Err(AppError::invalid(format!(
            "`{root}` is not a directory that exists; TFTP serves a directory that is already there"
        )));
    }
    let address = bind_address(
        params.bind.as_deref().unwrap_or(&settings.bind),
        params.port.unwrap_or(settings.port),
    )?;
    let config = TftpConfig {
        bind: address,
        root: path,
        allow_read: true,
        allow_write: params.allow_write,
        allow_overwrite: params.allow_overwrite,
        max_file_bytes: settings.max_file_mb.saturating_mul(1024 * 1024),
        ..TftpConfig::default()
    };

    // Bound here, not inside the job, so that a port which cannot be bound is
    // an error on the button rather than a job that failed somewhere else.
    let (server, mut events) = TftpServer::start(config.clone())
        .await
        .map_err(probe_error)?;
    let bound = server.local_addr().to_string();

    let mut record = Run::plain(
        "tftp",
        format!("TFTP server on {bound}"),
        history::where_from(app),
        &root,
    );
    record.address = Some(bound.clone());
    record.method = Some(format!(
        "UDP {bound}, {}",
        if params.allow_write {
            "read and write"
        } else {
            "read only"
        }
    ));
    history::begin(app, &record);
    let run_id = record.id.clone();

    let title = format!("TFTP server on {bound} serving {root}");
    let running = Running {
        address: bound.clone(),
        root: root.clone(),
        allow_write: params.allow_write,
        allow_overwrite: params.allow_overwrite,
        job_id: String::new(),
        run_id: run_id.clone(),
        started_at: util::now_ms(),
        transfers: Vec::new(),
        dropped: 0,
    };
    let job = jobs::start(
        app,
        JobSpec::new("tftp.serve", title),
        move |ctx| async move {
            record.job_id = Some(ctx.id().to_string());
            ctx.info(format!(
                "listening on UDP {bound}, serving {root}, {}",
                match (config.allow_write, config.allow_overwrite) {
                    (false, _) => "downloads only".to_string(),
                    (true, false) =>
                        "uploads allowed, but not over a file that is already there".to_string(),
                    (true, true) => "uploads allowed, including over existing files".to_string(),
                }
            ));
            let app = ctx.app().clone();
            let state = app.state::<State>();
            loop {
                tokio::select! {
                    biased;
                    _ = ctx.token().cancelled() => break,
                    event = events.recv() => match event {
                        Some(event) => {
                            describe(&ctx, &event);
                            app.publish("tftp.transfer", &Transfer { job_id: ctx.id().to_string(), event: event.clone() });
                            let mut slot = state.tftp.lock();
                            if let Some(running) = slot.as_mut() {
                                if running.transfers.len() >= KEPT_TRANSFERS {
                                    running.transfers.remove(0);
                                    running.dropped += 1;
                                }
                                running.transfers.push(event);
                            }
                        }
                        // The server's channel closed: it is gone, and so is the job.
                        None => break,
                    },
                }
            }
            // Stopping is what closes the socket; dropping would too, but
            // waiting for it means `tftp.status` never says "running" with
            // nothing behind it.
            server.stop().await;
            // Only this job's own record is cleared: if a later `tftp.start`
            // has already taken the slot, taking it here would hide a server
            // that really is running.
            let finished = {
                let mut slot = state.tftp.lock();
                match slot.as_ref() {
                    Some(running) if running.run_id == record.id => slot.take(),
                    _ => None,
                }
            };
            let transfers = finished
                .as_ref()
                .map(|running| running.transfers.len() + running.dropped)
                .unwrap_or(0);
            ctx.info("stopped listening");
            let mut record = record;
            history::succeeded(
                &app,
                &mut record,
                format!("{transfers} transfer events"),
                &finished,
            );
            Ok(json!({"transfers": transfers}))
        },
    );

    let running = Running {
        job_id: job.id.clone(),
        ..running
    };
    *app.state::<State>().tftp.lock() = Some(running.clone());
    app.audit(
        "tftp.start",
        None,
        format!(
            "Started a TFTP server on {} serving {}",
            running.address, running.root
        ),
        json!({"allow_write": running.allow_write, "allow_overwrite": running.allow_overwrite}),
    );
    Ok(Started {
        address: running.address,
        root: running.root,
        allow_write: running.allow_write,
        allow_overwrite: running.allow_overwrite,
        job_id: running.job_id,
        run_id,
    })
}

#[derive(Debug, Serialize)]
struct Transfer {
    job_id: String,
    /// The event's own fields, as the specification asks (`{..TftpEvent}`).
    #[serde(flatten)]
    event: TftpEvent,
}

/// A job-log line per transfer, so the log reads like a transfer log.
fn describe(ctx: &jobs::JobCtx, event: &TftpEvent) {
    match event {
        TftpEvent::Started {
            peer, file, write, ..
        } => ctx.info(format!(
            "{peer} {} {file}",
            if *write {
                "is uploading"
            } else {
                "is fetching"
            }
        )),
        TftpEvent::Finished {
            bytes, elapsed_ms, ..
        } => ctx.info(format!("finished, {bytes} bytes in {elapsed_ms} ms")),
        TftpEvent::Failed { error, .. } => ctx.warn(format!("transfer failed: {error}")),
        TftpEvent::Denied { peer, file, reason } => {
            ctx.warn(format!("refused {peer} access to {file}: {reason}"))
        }
        TftpEvent::Progress { .. } => {}
    }
}

fn stop(app: &App) -> AppResult<Stopped> {
    let state = app.state::<State>();
    let running = state.tftp.lock().clone();
    match running {
        Some(running) => {
            // Cancelling the job stops the server, and the job clears the
            // slot. A job that has already ended cannot do that, so a record
            // left behind by one is cleared here instead.
            if !jobs::cancel(app, &running.job_id).unwrap_or(false) {
                let mut slot = state.tftp.lock();
                if slot
                    .as_ref()
                    .is_some_and(|slot| slot.run_id == running.run_id)
                {
                    slot.take();
                }
            }
            app.audit(
                "tftp.stop",
                None,
                format!("Stopped the TFTP server on {}", running.address),
                Value::Null,
            );
            Ok(Stopped { stopped: true })
        }
        None => Ok(Stopped { stopped: false }),
    }
}

fn status(app: &App) -> Status {
    let settings = ToolboxSettings::load(app).tftp;
    let server = app.state::<State>().tftp.lock().clone();
    Status {
        running: server.is_some(),
        server,
        defaults: Defaults {
            bind: settings.bind,
            port: settings.port,
            root: settings.root,
        },
    }
}

pub fn register(router: &mut Router) {
    router.add("tftp.start", |app: App, params: StartParams| async move {
        start(&app, params).await
    });
    router.add(
        "tftp.stop",
        |app: App, _: NoParams| async move { stop(&app) },
    );
    router.add("tftp.status", |app: App, _: NoParams| async move {
        Ok(status(&app))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bind_address_is_an_address_of_this_machine() {
        assert_eq!(
            bind_address("0.0.0.0", 6969).unwrap().to_string(),
            "0.0.0.0:6969"
        );
        assert_eq!(bind_address(" ::1 ", 69).unwrap().to_string(), "[::1]:69");
        let refused = bind_address("not an address", 69).unwrap_err();
        assert_eq!(refused.code, "invalid");
        assert!(refused.message.contains("0.0.0.0"), "{}", refused.message);
    }
}
