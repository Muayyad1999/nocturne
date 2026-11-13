//! File transfer over SSH.
//!
//! `sftp.open` authenticates to a device exactly as a terminal session does
//! and keeps the SFTP channel in the backend under an id, so a UI can browse
//! with short calls and come back to the same session. Transfers of whole
//! files are jobs, because they take as long as the file is large.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nocturne_app::connect;
use nocturne_app::jobs::JobSpec;
use nocturne_app::model::Transport;
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppError, AppResult, Router, inventory, jobs};
use nocturne_transport::{Sftp, SftpEntry, SshSession};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::hostkey;
use crate::ssh;

/// Largest file `sftp.read_text` / `sftp.write_text` handle. Anything bigger
/// is a transfer, not an edit.
const MAX_TEXT: u64 = 1024 * 1024;
/// SFTP sessions one backend holds at once.
const MAX_SESSIONS: usize = 16;
/// How often idle SFTP sessions are swept, and how long they may idle.
const IDLE_AFTER: Duration = Duration::from_secs(30 * 60);

/// An open SFTP session.
struct Open {
    id: String,
    device_id: String,
    device_name: String,
    peer: String,
    home: String,
    opened_at: i64,
    sftp: Arc<Sftp>,
    /// Dropped with the session: the SSH connection goes with it.
    _ssh: Arc<SshSession>,
    last_used: Mutex<std::time::Instant>,
}

#[derive(Default)]
pub struct SftpState {
    sessions: Mutex<HashMap<String, Arc<Open>>>,
}

#[derive(Debug, Clone, Serialize)]
struct SftpSessionInfo {
    sftp_id: String,
    device_id: String,
    device_name: String,
    peer: String,
    /// The directory the device puts the user in.
    home: String,
    opened_at: i64,
}

impl Open {
    fn info(&self) -> SftpSessionInfo {
        SftpSessionInfo {
            sftp_id: self.id.clone(),
            device_id: self.device_id.clone(),
            device_name: self.device_name.clone(),
            peer: self.peer.clone(),
            home: self.home.clone(),
            opened_at: self.opened_at,
        }
    }
}

fn session(app: &App, sftp_id: &str) -> AppResult<Arc<Open>> {
    let session = app
        .state::<SftpState>()
        .sessions
        .lock()
        .get(sftp_id)
        .cloned()
        .ok_or_else(|| {
            AppError::not_found(format!(
                "SFTP session `{sftp_id}` (it may have been closed; open a new one with sftp.open)"
            ))
        })?;
    *session.last_used.lock() = std::time::Instant::now();
    Ok(session)
}

// ---------------------------------------------------------------------- open

#[derive(Debug, Deserialize)]
struct OpenParams {
    device_id: String,
}

async fn open(app: &App, params: OpenParams) -> AppResult<SftpSessionInfo> {
    let device = inventory::find(app, &params.device_id)?;
    if device.transport != Transport::Ssh {
        return Err(AppError::invalid(format!(
            "`{}` is not reached over SSH, and SFTP is part of SSH. Copy files to it another way (TFTP, a USB stick, the console).",
            device.name
        )));
    }
    {
        let state = app.state::<SftpState>();
        let sessions = state.sessions.lock();
        if sessions.len() >= MAX_SESSIONS {
            return Err(AppError::new(
                "limit",
                format!(
                    "{MAX_SESSIONS} SFTP sessions are open, which is the most Nocturne holds at once; close one first"
                ),
            ));
        }
    }
    let params = ssh::device_params(app, &device)?;
    let connecting = async {
        let session = SshSession::connect(&params, connect::verifier(app))
            .await
            .map_err(AppError::from)?;
        let sftp = session.sftp().await.map_err(AppError::from)?;
        Ok((session, sftp))
    };
    let (ssh_session, sftp) = hostkey::with_details(app, connecting).await?;
    let peer = ssh_session.info().peer.clone();
    // Not fatal: a device whose SFTP server has no notion of a home directory
    // still browses from the root.
    let home = sftp.realpath(".").await.unwrap_or_else(|err| {
        tracing::debug!(%err, device = %device.name, "the SFTP home directory is unknown");
        "/".to_string()
    });

    let open = Arc::new(Open {
        id: new_id(),
        device_id: device.id.clone(),
        device_name: device.name.clone(),
        peer,
        home,
        opened_at: now_ms(),
        sftp: Arc::new(sftp),
        _ssh: Arc::new(ssh_session),
        last_used: Mutex::new(std::time::Instant::now()),
    });
    let info = open.info();
    app.state::<SftpState>()
        .sessions
        .lock()
        .insert(open.id.clone(), open);
    app.audit(
        "sftp.open",
        Some(&device.id),
        format!("Opened an SFTP session to {}", device.name),
        json!({"sftp_id": info.sftp_id, "peer": info.peer}),
    );
    app.publish("sftp.opened", &info);
    Ok(info)
}

// ------------------------------------------------------------------ browsing

#[derive(Debug, Deserialize)]
struct PathParams {
    sftp_id: String,
    path: String,
}

#[derive(Debug, Deserialize)]
struct RenameParams {
    sftp_id: String,
    from: String,
    to: String,
}

#[derive(Debug, Deserialize)]
struct SftpRef {
    sftp_id: String,
}

#[derive(Debug, Serialize)]
struct Listing {
    path: String,
    entries: Vec<SftpEntry>,
}

async fn list(app: &App, params: PathParams) -> AppResult<Listing> {
    let session = session(app, &params.sftp_id)?;
    let path = if params.path.trim().is_empty() {
        session.home.clone()
    } else {
        params.path.clone()
    };
    let mut entries = session.sftp.list(&path).await.map_err(AppError::from)?;
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(Listing { path, entries })
}

async fn stat(app: &App, params: PathParams) -> AppResult<SftpEntry> {
    Ok(session(app, &params.sftp_id)?
        .sftp
        .stat(&params.path)
        .await?)
}

async fn mkdir(app: &App, params: PathParams) -> AppResult<Value> {
    let session = session(app, &params.sftp_id)?;
    session.sftp.mkdir(&params.path).await?;
    app.audit(
        "sftp.mkdir",
        Some(&session.device_id),
        format!("Created {} on {}", params.path, session.device_name),
        json!({"path": params.path}),
    );
    Ok(json!({}))
}

async fn remove(app: &App, params: PathParams) -> AppResult<Value> {
    let session = session(app, &params.sftp_id)?;
    session.sftp.remove(&params.path).await?;
    app.audit(
        "sftp.remove",
        Some(&session.device_id),
        format!("Deleted {} on {}", params.path, session.device_name),
        json!({"path": params.path}),
    );
    Ok(json!({}))
}

async fn rename(app: &App, params: RenameParams) -> AppResult<Value> {
    let session = session(app, &params.sftp_id)?;
    session.sftp.rename(&params.from, &params.to).await?;
    app.audit(
        "sftp.rename",
        Some(&session.device_id),
        format!(
            "Renamed {} to {} on {}",
            params.from, params.to, session.device_name
        ),
        json!({"from": params.from, "to": params.to}),
    );
    Ok(json!({}))
}

async fn close(app: &App, params: SftpRef) -> AppResult<Value> {
    let session = app
        .state::<SftpState>()
        .sessions
        .lock()
        .remove(&params.sftp_id);
    let Some(session) = session else {
        return Err(AppError::not_found(format!(
            "SFTP session `{}`",
            params.sftp_id
        )));
    };
    shut_down(session).await;
    app.publish("sftp.closed", &json!({"sftp_id": params.sftp_id}));
    Ok(json!({}))
}

/// Hang up politely if nothing else is using the channel.
async fn shut_down(session: Arc<Open>) {
    let Ok(open) = Arc::try_unwrap(session) else {
        // A transfer still holds it; it goes when that finishes.
        return;
    };
    if let Ok(sftp) = Arc::try_unwrap(open.sftp) {
        sftp.close().await;
    }
}

/// Close every SFTP session (at shutdown).
pub(crate) async fn close_all(app: &App) {
    let sessions: Vec<Arc<Open>> = app
        .state::<SftpState>()
        .sessions
        .lock()
        .drain()
        .map(|(_, session)| session)
        .collect();
    for session in sessions {
        shut_down(session).await;
    }
}

/// Forget SFTP sessions nobody has used for a while: each one holds an SSH
/// connection open on the device.
pub(crate) async fn reap_idle(app: App) {
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let state = app.state::<SftpState>();
        let stale: Vec<Arc<Open>> = {
            let mut sessions = state.sessions.lock();
            let stale: Vec<String> = sessions
                .values()
                .filter(|session| session.last_used.lock().elapsed() >= IDLE_AFTER)
                .map(|session| session.id.clone())
                .collect();
            stale.iter().filter_map(|id| sessions.remove(id)).collect()
        };
        for session in stale {
            let id = session.id.clone();
            shut_down(session).await;
            app.publish(
                "sftp.closed",
                &json!({"sftp_id": id, "reason": "idle for 30 minutes"}),
            );
        }
    }
}

// ----------------------------------------------------------------- text files

#[derive(Debug, Serialize)]
struct TextResult {
    text: String,
    /// Size of the file on the device, in bytes.
    size: u64,
}

async fn read_text(app: &App, params: PathParams) -> AppResult<TextResult> {
    let session = session(app, &params.sftp_id)?;
    let attributes = session.sftp.stat(&params.path).await?;
    if attributes.is_dir {
        return Err(AppError::invalid(format!(
            "{} is a directory, not a file",
            params.path
        )));
    }
    if attributes.size > MAX_TEXT {
        return Err(AppError::invalid(format!(
            "{} is {} bytes; sftp.read_text reads at most {MAX_TEXT}. Download it instead.",
            params.path, attributes.size
        )));
    }
    let bytes = session.sftp.read(&params.path).await?;
    let size = bytes.len() as u64;
    let text = String::from_utf8(bytes).map_err(|_| {
        AppError::invalid(format!(
            "{} is not UTF-8 text, so it cannot be shown or edited here. Download it instead.",
            params.path
        ))
    })?;
    Ok(TextResult { text, size })
}

#[derive(Debug, Deserialize)]
struct WriteTextParams {
    sftp_id: String,
    path: String,
    text: String,
}

#[derive(Debug, Serialize)]
struct WriteResult {
    /// Bytes written.
    size: u64,
}

async fn write_text(app: &App, params: WriteTextParams) -> AppResult<WriteResult> {
    let session = session(app, &params.sftp_id)?;
    let bytes = params.text.into_bytes();
    if bytes.len() as u64 > MAX_TEXT {
        return Err(AppError::invalid(format!(
            "sftp.write_text writes at most {MAX_TEXT} bytes; upload a file instead"
        )));
    }
    session.sftp.write(&params.path, &bytes).await?;
    app.audit(
        "sftp.write",
        Some(&session.device_id),
        format!(
            "Wrote {} ({} bytes) on {}",
            params.path,
            bytes.len(),
            session.device_name
        ),
        json!({"path": params.path, "size": bytes.len()}),
    );
    Ok(WriteResult {
        size: bytes.len() as u64,
    })
}

// ------------------------------------------------------------------ transfers

#[derive(Debug, Deserialize)]
struct TransferParams {
    sftp_id: String,
    /// Path on the device.
    remote: String,
    /// Path on this machine.
    local: String,
}

/// Where a transfer may touch this machine. A relative path has no meaning to
/// a user who did not start the process, so it is refused outright.
fn local_path(local: &str) -> AppResult<PathBuf> {
    let path = Path::new(local.trim());
    if path.as_os_str().is_empty() {
        return Err(AppError::invalid("no local file named"));
    }
    if !path.is_absolute() {
        return Err(AppError::invalid(format!(
            "`{local}` is not an absolute path; name the file in full"
        )));
    }
    Ok(path.to_path_buf())
}

fn transfer(app: &App, params: TransferParams, upload: bool) -> AppResult<Value> {
    app.require_local_access(if upload {
        "uploading a file from this machine"
    } else {
        "downloading a file to this machine"
    })?;
    let session = session(app, &params.sftp_id)?;
    let local = local_path(&params.local)?;
    if upload && !local.is_file() {
        return Err(AppError::not_found(format!("the file {}", local.display())));
    }
    let what = if upload { "Upload" } else { "Download" };
    let title = format!(
        "{what} {} {} {}",
        if upload {
            &params.local
        } else {
            &params.remote
        },
        if upload { "to" } else { "from" },
        session.device_name
    );
    let device_ids = vec![session.device_id.clone()];
    let job = jobs::start(
        app,
        JobSpec::new(
            if upload {
                "sftp.upload"
            } else {
                "sftp.download"
            },
            title.clone(),
        )
        .devices(&device_ids)
        .total(0),
        move |ctx| async move {
            let (progress, mut updates) = tokio::sync::mpsc::channel::<(u64, u64)>(8);
            let sftp = session.sftp.clone();
            let remote = params.remote.clone();
            let moving = async move {
                if upload {
                    sftp.upload(&local, &remote, Some(progress)).await
                } else {
                    sftp.download(&remote, &local, Some(progress)).await
                }
            };
            tokio::pin!(moving);
            let copied = loop {
                tokio::select! {
                    update = updates.recv() => match update {
                        Some((done, total)) => {
                            if total > 0 {
                                ctx.set_total(total);
                            }
                            ctx.progress(done, format!("{done} of {total} bytes"));
                        }
                        // The transfer dropped its sender: it is finishing.
                        None => break ctx.race(async { Ok((&mut moving).await?) }).await?,
                    },
                    result = &mut moving => break result.map_err(AppError::from)?,
                }
            };
            ctx.info(format!("{copied} bytes transferred"));
            ctx.app().audit(
                if upload {
                    "sftp.upload"
                } else {
                    "sftp.download"
                },
                Some(&session.device_id),
                title,
                json!({"remote": params.remote, "local": params.local, "bytes": copied}),
            );
            Ok(json!({"bytes": copied, "remote": params.remote, "local": params.local}))
        },
    );
    Ok(jobs::started(&job))
}

pub(crate) fn register(router: &mut Router) {
    router.add("sftp.open", |app: App, params: OpenParams| async move {
        open(&app, params).await
    });
    router.add("sftp.list", |app: App, params: PathParams| async move {
        list(&app, params).await
    });
    router.add("sftp.stat", |app: App, params: PathParams| async move {
        stat(&app, params).await
    });
    router.add("sftp.mkdir", |app: App, params: PathParams| async move {
        mkdir(&app, params).await
    });
    router.add("sftp.remove", |app: App, params: PathParams| async move {
        remove(&app, params).await
    });
    router.add("sftp.rename", |app: App, params: RenameParams| async move {
        rename(&app, params).await
    });
    router.add("sftp.close", |app: App, params: SftpRef| async move {
        close(&app, params).await
    });
    router.add(
        "sftp.read_text",
        |app: App, params: PathParams| async move { read_text(&app, params).await },
    );
    router.add(
        "sftp.write_text",
        |app: App, params: WriteTextParams| async move { write_text(&app, params).await },
    );
    router.add(
        "sftp.download",
        |app: App, params: TransferParams| async move { transfer(&app, params, false) },
    );
    router.add(
        "sftp.upload",
        |app: App, params: TransferParams| async move { transfer(&app, params, true) },
    );
    router.add(
        "sftp.sessions",
        |app: App, _params: nocturne_app::NoParams| async move {
            let mut sessions: Vec<SftpSessionInfo> = app
                .state::<SftpState>()
                .sessions
                .lock()
                .values()
                .map(|session| session.info())
                .collect();
            sessions.sort_by_key(|session| session.opened_at);
            Ok(sessions)
        },
    );
}
