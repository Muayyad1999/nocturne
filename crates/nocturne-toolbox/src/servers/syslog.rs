//! `probe.syslog.*` — a syslog listener for a device being staged.
//!
//! This is **not** Nocturne's log collection. `nocturne-logs` owns that: it has
//! the long-lived receiver, the settings for it, the ingest rules, the store
//! and `logs.status`. What this is, is the thing an engineer does with a laptop
//! and `tcpdump` while a switch boots for the first time: point the device at
//! this port for ten minutes and watch what it shouts. So:
//!
//! * it is off unless asked, and it is a job, so it stops with the job and
//!   with the application;
//! * it keeps its messages **in memory only** — nothing is written to the log
//!   store, because two features writing the same stream is how a log ends up
//!   with every line twice;
//! * it listens on 5515 by default, not the 5514 the `logs` receiver takes, so
//!   that staging never fights log collection for a port — and if the port is
//!   taken anyway, that is what it says.
//!
//! The method names live under `probe.` deliberately: the global `syslog.`
//! namespace belongs to whichever feature ends up owning syslog properly.

use std::collections::VecDeque;
use std::net::SocketAddr;

use nocturne_app::jobs::{self, JobSpec};
use nocturne_app::{App, AppError, AppResult, NoParams, Router, util};
use nocturne_netprobe::syslog::{SyslogMessage, SyslogServer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::errors::probe as probe_error;
use crate::probe::history::{self, Run};
use crate::servers::tftp::bind_address;
use crate::settings::ToolboxSettings;
use crate::state::State;

/// A running listener, as the rest of the application sees it.
#[derive(Clone, Debug, Serialize)]
pub struct Running {
    pub udp_address: String,
    pub tcp_address: Option<String>,
    pub job_id: String,
    pub run_id: String,
    pub started_at: i64,
    /// Messages received since it started.
    pub received: u64,
    /// Senders seen, with how many messages each sent.
    pub senders: Vec<Sender>,
    /// Messages kept in memory (the newest `keep_messages`).
    #[serde(skip)]
    pub messages: VecDeque<SyslogMessage>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Sender {
    pub address: String,
    pub messages: u64,
    /// The device of the inventory at that address, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct StartParams {
    pub bind: Option<String>,
    pub port: Option<u16>,
    /// Also listen on TCP (RFC 6587), on the same port.
    pub tcp: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct Started {
    pub udp_address: String,
    pub tcp_address: Option<String>,
    pub job_id: String,
    pub run_id: String,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub listening: bool,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub server: Option<Running>,
    pub defaults: Defaults,
    /// Said once, so nobody mistakes this for log collection.
    pub note: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Defaults {
    pub bind: String,
    pub port: u16,
    pub tcp: bool,
    pub keep_messages: u32,
}

#[derive(Debug, Serialize)]
pub struct Stopped {
    pub stopped: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessagesParams {
    pub limit: Option<usize>,
    /// Only the messages of this sender address.
    pub source: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Messages {
    pub messages: Vec<SyslogMessage>,
    /// Messages in memory, before `limit`.
    pub total: usize,
    pub listening: bool,
}

const NOTE: &str = "This is a staging listener: it keeps messages in memory while it runs and stores nothing. Nocturne's log collection is the logs feature (Settings → Logs).";

#[derive(Debug, Serialize)]
struct Received {
    job_id: String,
    #[serde(flatten)]
    message: SyslogMessage,
}

async fn start(app: &App, params: StartParams) -> AppResult<Started> {
    app.require_local_access("the syslog listener")?;
    let settings = ToolboxSettings::load(app).syslog;
    let state = app.state::<State>();
    if let Some(running) = state.syslog.lock().clone() {
        return Err(AppError::conflict(format!(
            "a syslog listener is already running on {}; stop it first",
            running.udp_address
        )));
    }
    let port = params.port.unwrap_or(settings.port);
    let udp = bind_address(params.bind.as_deref().unwrap_or(&settings.bind), port)?;
    let tcp: Option<SocketAddr> = params.tcp.unwrap_or(settings.tcp).then_some(udp);

    // Bound before the job starts: a port that cannot be had is an error with
    // the reason in it, not a listener that looks as though it is working.
    let (server, mut messages) = SyslogServer::start(udp, tcp).await.map_err(probe_error)?;
    let udp_address = server.udp_addr().to_string();
    let tcp_address = server.tcp_addr().map(|address| address.to_string());

    let mut record = Run::plain(
        "syslog",
        format!("Syslog listener on {udp_address}"),
        history::where_from(app),
        &udp_address,
    );
    record.method = Some(match &tcp_address {
        Some(tcp) => format!("UDP {udp_address} and TCP {tcp}"),
        None => format!("UDP {udp_address}"),
    });
    history::begin(app, &record);
    let run_id = record.id.clone();

    let keep = settings.keep_messages as usize;
    let running = Running {
        udp_address: udp_address.clone(),
        tcp_address: tcp_address.clone(),
        job_id: String::new(),
        run_id: run_id.clone(),
        started_at: util::now_ms(),
        received: 0,
        senders: Vec::new(),
        messages: VecDeque::new(),
    };
    let known = crate::probe::known::Known::load(app);
    let title = format!("Syslog listener on {udp_address}");
    let job = jobs::start(
        app,
        JobSpec::new("probe.syslog.listen", title),
        move |ctx| async move {
            record.job_id = Some(ctx.id().to_string());
            ctx.info(format!(
                "listening on {}; messages are kept in memory while this job runs and stored nowhere",
                record.method.clone().unwrap_or_default()
            ));
            let app = ctx.app().clone();
            let state = app.state::<State>();
            loop {
                tokio::select! {
                    biased;
                    _ = ctx.token().cancelled() => break,
                    message = messages.recv() => match message {
                        Some(message) => {
                            app.publish(
                                "probe.syslog.message",
                                &Received { job_id: ctx.id().to_string(), message: message.clone() },
                            );
                            let mut slot = state.syslog.lock();
                            if let Some(running) = slot.as_mut() {
                                running.received += 1;
                                let source = message.source.clone();
                                match running
                                    .senders
                                    .iter_mut()
                                    .find(|sender| sender.address == source)
                                {
                                    Some(sender) => sender.messages += 1,
                                    None => {
                                        let address = source
                                            .rsplit_once(':')
                                            .map(|(host, _)| host.to_string())
                                            .unwrap_or_else(|| source.clone());
                                        running.senders.push(Sender {
                                            device: known.of_text(&address).map(|device| device.name),
                                            address: source.clone(),
                                            messages: 1,
                                        });
                                    }
                                }
                                if running.messages.len() >= keep {
                                    running.messages.pop_front();
                                }
                                running.messages.push_back(message);
                                let count = running.received;
                                drop(slot);
                                ctx.progress(count, format!("{count} messages from {source}"));
                            }
                        }
                        None => break,
                    },
                }
            }
            server.stop().await;
            // Only this job's own record is cleared: if a later start has
            // already taken the slot, taking it here would hide a listener
            // that really is running.
            let finished = {
                let mut slot = state.syslog.lock();
                match slot.as_ref() {
                    Some(running) if running.run_id == record.id => slot.take(),
                    _ => None,
                }
            };
            let received = finished
                .as_ref()
                .map(|running| running.received)
                .unwrap_or(0);
            ctx.info("stopped listening");
            let mut record = record;
            // The messages themselves are not kept: they were a window onto a
            // device being staged, not a log.
            history::succeeded(
                &app,
                &mut record,
                format!("{received} messages"),
                &json!({"received": received, "senders": finished.map(|running| running.senders)}),
            );
            Ok(json!({"received": received}))
        },
    );

    let running = Running {
        job_id: job.id.clone(),
        ..running
    };
    *app.state::<State>().syslog.lock() = Some(running);
    app.audit(
        "probe.syslog.start",
        None,
        format!("Started a syslog listener on {udp_address}"),
        json!({"tcp": tcp_address}),
    );
    Ok(Started {
        udp_address,
        tcp_address,
        job_id: job.id,
        run_id,
    })
}

fn stop(app: &App) -> AppResult<Stopped> {
    let state = app.state::<State>();
    let running = state.syslog.lock().clone();
    match running {
        Some(running) => {
            // A job that has already ended cannot clear the slot itself.
            if !jobs::cancel(app, &running.job_id).unwrap_or(false) {
                let mut slot = state.syslog.lock();
                if slot
                    .as_ref()
                    .is_some_and(|slot| slot.run_id == running.run_id)
                {
                    slot.take();
                }
            }
            app.audit(
                "probe.syslog.stop",
                None,
                format!("Stopped the syslog listener on {}", running.udp_address),
                Value::Null,
            );
            Ok(Stopped { stopped: true })
        }
        None => Ok(Stopped { stopped: false }),
    }
}

fn status(app: &App) -> Status {
    let settings = ToolboxSettings::load(app).syslog;
    let server = app.state::<State>().syslog.lock().clone();
    Status {
        listening: server.is_some(),
        server,
        defaults: Defaults {
            bind: settings.bind,
            port: settings.port,
            tcp: settings.tcp,
            keep_messages: settings.keep_messages,
        },
        note: NOTE,
    }
}

fn received(app: &App, params: MessagesParams) -> Messages {
    let server = app.state::<State>().syslog.lock().clone();
    let limit = params.limit.unwrap_or(200).clamp(1, 10_000);
    let source = params
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match server {
        Some(running) => {
            let matching: Vec<SyslogMessage> = running
                .messages
                .iter()
                .filter(|message| source.is_none_or(|source| message.source.starts_with(source)))
                .cloned()
                .collect();
            let total = matching.len();
            Messages {
                messages: matching
                    .into_iter()
                    .skip(total.saturating_sub(limit))
                    .collect(),
                total,
                listening: true,
            }
        }
        None => Messages {
            messages: Vec::new(),
            total: 0,
            listening: false,
        },
    }
}

pub fn register(router: &mut Router) {
    router.add(
        "probe.syslog.start",
        |app: App, params: StartParams| async move { start(&app, params).await },
    );
    router.add("probe.syslog.stop", |app: App, _: NoParams| async move {
        stop(&app)
    });
    router.add("probe.syslog.status", |app: App, _: NoParams| async move {
        Ok(status(&app))
    });
    router.add(
        "probe.syslog.messages",
        |app: App, params: MessagesParams| async move { Ok(received(&app, params)) },
    );
}
