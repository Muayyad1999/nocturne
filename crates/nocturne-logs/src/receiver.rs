//! The syslog receiver and the supervisor that follows the settings.
//!
//! Binding a socket is the one thing here that can fail for a reason the
//! user has to act on — the port needs privileges, something else already
//! has it, the address is not on this machine — so a failure is kept and
//! reported through `logs.status` rather than logged and forgotten. The
//! receiver is not retried in a loop: it is restarted when the settings
//! change, because nothing else about the machine is going to change on its
//! own.

use std::net::{IpAddr, SocketAddr};

use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult};
use nocturne_netprobe::syslog::SyslogServer;
use tokio::sync::mpsc::{Sender, channel, error::TrySendError};

use crate::settings::LogsSettings;
use crate::state::LogsState;
use crate::{ingest, status};

/// Stop whatever is running, then start what the settings ask for.
///
/// Never returns an error for a port that cannot be bound: that is a state
/// of the receiver, not a failure of the call that tried to start it.
pub async fn apply(app: &App) -> AppResult<()> {
    let settings = LogsSettings::load(app);
    let state = app.state::<LogsState>();
    let generation = {
        let mut receiver = state.receiver.lock();
        // Nothing to do when the receiver is already running exactly this:
        // rebinding would change the ports under whoever is sending.
        if receiver.applied.as_ref() == Some(&settings.syslog)
            && (receiver.server.is_some() || receiver.error.is_some() || !settings.syslog.enabled)
        {
            return Ok(());
        }
        receiver.applied = Some(settings.syslog.clone());
        receiver.generation += 1;
        receiver.enabled = settings.syslog.enabled;
        receiver.listening.clear();
        receiver.error = None;
        // Dropping the old server closes its sockets and ends its tasks.
        receiver.server = None;
        receiver.generation
    };
    *state.queue.lock() = None;

    if !settings.syslog.enabled {
        status::publish(app);
        return Ok(());
    }

    let bind: IpAddr = match settings.syslog.bind.trim().parse() {
        Ok(bind) => bind,
        Err(_) => {
            fail(
                app,
                format!(
                    "`{}` is not an address this machine can listen on (try 0.0.0.0 for every interface)",
                    settings.syslog.bind
                ),
            );
            return Ok(());
        }
    };
    let udp = SocketAddr::new(bind, settings.syslog.udp_port);
    let tcp =
        (settings.syslog.tcp_port != 0).then(|| SocketAddr::new(bind, settings.syslog.tcp_port));

    let (server, messages) = match SyslogServer::start(udp, tcp).await {
        Ok(started) => started,
        Err(err) => {
            fail(app, err.to_string());
            return Ok(());
        }
    };
    let listening = std::iter::once(format!("udp {}", server.udp_addr()))
        .chain(server.tcp_addr().map(|addr| format!("tcp {addr}")))
        .collect::<Vec<_>>();

    let limit = settings.queue_limit as usize;
    let (queue, writer_queue) = channel(limit);
    {
        let mut receiver = state.receiver.lock();
        if receiver.generation != generation {
            // Another call overtook this one; its receiver wins and ours is
            // dropped here, closing the sockets it just opened.
            return Ok(());
        }
        receiver.listening = listening.clone();
        receiver.server = Some(server);
        receiver.queue_limit = limit;
    }
    *state.queue.lock() = Some(queue.clone());

    app.spawn("logs.syslog.relay", relay(app.clone(), messages, queue));
    app.spawn(
        "logs.syslog.writer",
        ingest::writer(app.clone(), writer_queue, generation),
    );
    app.audit(
        "logs.receiver.start",
        None,
        format!("Syslog receiver listening on {}", listening.join(", ")),
        serde_json::json!({"listening": listening}),
    );
    status::publish(app);
    Ok(())
}

fn fail(app: &App, error: String) {
    tracing::warn!(%error, "the syslog receiver could not start");
    app.state::<LogsState>().receiver.lock().error = Some(error);
    status::publish(app);
}

/// Everything the sockets parse, handed to the writer — or dropped and
/// counted when the writer is more than [`QUEUE_LIMIT`] behind.
async fn relay(
    app: App,
    mut messages: tokio::sync::mpsc::UnboundedReceiver<nocturne_netprobe::syslog::SyslogMessage>,
    queue: Sender<nocturne_netprobe::syslog::SyslogMessage>,
) {
    let state = app.state::<LogsState>();
    let mut batch = Vec::with_capacity(256);
    // This loop does nothing but count and hand over, so the engine's own
    // unbounded channel never builds up: the bound that matters is the
    // writer's queue, which is the one the drop counter is about.
    loop {
        let received = messages.recv_many(&mut batch, 256).await;
        if received == 0 {
            return;
        }
        let now = now_ms();
        let mut dropped = 0u64;
        for message in batch.drain(..) {
            state.counters.received(now);
            match queue.try_send(message) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => dropped += 1,
                Err(TrySendError::Closed(_)) => return,
            }
        }
        state.rate.lock().record(now, received as u64);
        if dropped > 0 {
            state.counters.dropped(dropped);
        }
    }
}

/// Follow the settings: a change to the `logs` section restarts the receiver.
pub fn spawn(app: &App) {
    let app = app.clone();
    tokio::spawn(async move {
        // Subscribed before the first apply, so a settings change made while
        // the application is starting cannot slip between the two and leave
        // the receiver following settings nobody asked for any more.
        let mut events = app.subscribe();
        if let Err(err) = apply(&app).await {
            tracing::error!(%err, "the syslog receiver could not be started");
        }
        let shutdown = app.shutdown_token();
        loop {
            let event = tokio::select! {
                _ = shutdown.cancelled() => break,
                event = events.recv() => event,
            };
            match event {
                Ok(event)
                    if event.topic == "settings.changed"
                        && event.payload["section"] == crate::settings::SECTION =>
                {
                    if let Err(err) = apply(&app).await {
                        tracing::error!(%err, "the syslog receiver could not be restarted");
                    }
                }
                Ok(event)
                    if event.topic == "devices.changed" || event.topic == "collect.updated" =>
                {
                    crate::attribute::invalidate(&app);
                }
                Ok(_) => {}
                // Falling behind the bus loses events, not correctness: the
                // next one still restarts the receiver.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// The addresses the receiver is listening on, for tests and for the UI's
/// "point your devices here" hint.
pub fn listening(app: &App) -> Vec<String> {
    app.state::<LogsState>().receiver.lock().listening.clone()
}

/// The UDP port actually bound, which is not the configured one when the
/// configured one was 0.
pub fn bound_port(app: &App, protocol: &str) -> AppResult<u16> {
    listening(app)
        .iter()
        .find_map(|entry| {
            let rest = entry.strip_prefix(protocol)?.trim();
            rest.parse::<SocketAddr>().ok().map(|addr| addr.port())
        })
        .ok_or_else(|| {
            AppError::unavailable(format!(
                "the syslog receiver is not listening on {protocol}"
            ))
        })
}
