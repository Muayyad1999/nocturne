//! The syslog receiver: UDP and TCP sockets, the bounded queue behind them
//! and the task that writes what they hand over.
//!
//! Binding a socket is the one thing here that can fail for a reason the
//! user has to act on — the port needs privileges, something else already
//! has it, the address is not on this machine — so a failure is kept and
//! reported through `logs.status` rather than logged and forgotten. The
//! receiver is not retried in a loop: it is restarted when the settings
//! change (`receiver::spawn`), because nothing else about the machine is
//! going to change on its own.

use std::net::{IpAddr, SocketAddr};

use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult};
use nocturne_netprobe::syslog::{SyslogMessage, SyslogServer};
use tokio::sync::mpsc::{Receiver, Sender, channel, error::TrySendError};

use crate::settings::LogsSettings;
use crate::state::{BATCH_ROWS, LogsState};
use crate::{ingest, status};

/// Stop whatever is running, then start what the settings ask for.
///
/// Never returns an error for a port that cannot be bound: that is a state
/// of the receiver, not a failure of the call that tried to start it.
pub async fn apply(app: &App) -> AppResult<()> {
    let settings = LogsSettings::load(app);
    let state = app.state::<LogsState>();
    let generation = {
        let mut receiver = state.syslog.receiver.lock();
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
    *state.syslog.queue.lock() = None;

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
        let mut receiver = state.syslog.receiver.lock();
        if receiver.generation != generation {
            // Another call overtook this one; its receiver wins and ours is
            // dropped here, closing the sockets it just opened.
            return Ok(());
        }
        receiver.listening = listening.clone();
        receiver.server = Some(server);
        receiver.queue_limit = limit;
    }
    *state.syslog.queue.lock() = Some(queue.clone());

    app.spawn("logs.syslog.relay", relay(app.clone(), messages, queue));
    app.spawn(
        "logs.syslog.writer",
        writer(app.clone(), writer_queue, generation),
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
    app.state::<LogsState>().syslog.receiver.lock().error = Some(error);
    status::publish(app);
}

/// Everything the sockets parse, handed to the writer — or dropped and
/// counted when the writer is more than the `queue_limit` setting behind.
async fn relay(
    app: App,
    mut messages: tokio::sync::mpsc::UnboundedReceiver<SyslogMessage>,
    queue: Sender<SyslogMessage>,
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
            state.syslog.counters.received(now);
            match queue.try_send(message) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => dropped += 1,
                Err(TrySendError::Closed(_)) => return,
            }
        }
        state.syslog.rate.lock().record(now, received as u64);
        if dropped > 0 {
            state.syslog.counters.dropped(dropped);
        }
    }
}

/// The writer task: everything the sockets hand over, in batches.
async fn writer(app: App, mut queue: Receiver<SyslogMessage>, generation: u64) {
    let state = app.state::<LogsState>();
    let mut messages = Vec::with_capacity(BATCH_ROWS);
    // `recv_many` returns as soon as there is anything at all, so a quiet
    // network's single line is written straight away and a storm arrives in
    // batches of up to `BATCH_ROWS` without a timer.
    while queue.recv_many(&mut messages, BATCH_ROWS).await > 0 {
        if state.syslog.receiver.lock().generation != generation {
            return;
        }
        if let Err(err) = ingest::write_batch(&app, &messages, "syslog") {
            tracing::error!(%err, count = messages.len(), "a batch of syslog lines could not be stored");
        }
        messages.clear();
    }
}

/// The addresses the receiver is listening on, for tests and for the UI's
/// "point your devices here" hint.
pub fn listening(app: &App) -> Vec<String> {
    app.state::<LogsState>()
        .syslog
        .receiver
        .lock()
        .listening
        .clone()
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
