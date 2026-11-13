//! Lab consoles as ordinary connections.
//!
//! This is what makes a simulated device a device: the core asks the lab
//! module to open a console for a `lab` reference, and gets back a
//! [`Connection`] like any transport's. Input is applied to the simulator at
//! once — followed by a sliver of simulated time and an immediate drain — so
//! typing and automation are never quantised to the ticker.
//!
//! The connector deliberately holds no `App`: `ConnectState` lives in the
//! app's state map, so an `App` inside it would keep the application alive for
//! ever. It holds a channel instead, served by a task that stops with the app.

use std::sync::Arc;

use async_trait::async_trait;
use nocturne_app::connect::{ConnectState, SimConnector};
use nocturne_app::model::LabRef;
use nocturne_app::{App, AppError, AppResult};
use nocturne_netsim::ConsoleId;
use nocturne_transport::{Backend, Connection, ConnectionInfo, Input, Output, TransportKind};
use tokio::sync::{mpsc, oneshot};

use crate::model;
use crate::runtime::{self, RunningLab};

/// Simulated time applied after a burst of console input, so that a command
/// which needs the network to answer starts moving before the next tick.
const INPUT_STEP_US: u64 = 2_000;

struct OpenRequest {
    lab: LabRef,
    cols: u16,
    rows: u16,
    reply: oneshot::Sender<AppResult<Connection>>,
}

/// A handle that opens lab consoles without holding an `App`.
#[derive(Clone)]
pub struct Opener {
    requests: mpsc::UnboundedSender<OpenRequest>,
}

impl Opener {
    pub async fn open(&self, lab: &LabRef, cols: u16, rows: u16) -> AppResult<Connection> {
        let (reply, answer) = oneshot::channel();
        let request = OpenRequest {
            lab: lab.clone(),
            cols,
            rows,
            reply,
        };
        self.requests
            .send(request)
            .map_err(|_| AppError::unavailable("the lab simulator is shutting down"))?;
        answer.await.map_err(|_| {
            AppError::unavailable("the lab simulator stopped before the console was open")
        })?
    }
}

#[async_trait]
impl SimConnector for Opener {
    async fn open(&self, _app: &App, lab: &LabRef, cols: u16, rows: u16) -> AppResult<Connection> {
        // The core now hands the application in, so a connector no longer has
        // to avoid holding one. This one still goes through its channel
        // because `expose` opens consoles the same way from a running SSH
        // server, which has no application to hand it.
        Opener::open(self, lab, cols, rows).await
    }
}

/// Register the connector and the task that serves it.
pub fn install(app: &App) -> Opener {
    let (requests, incoming) = mpsc::unbounded_channel();
    let opener = Opener { requests };
    app.state::<ConnectState>()
        .set_sim_connector(Arc::new(opener.clone()));
    app.spawn("labs.consoles", serve(app.clone(), incoming));
    opener
}

async fn serve(app: App, mut incoming: mpsc::UnboundedReceiver<OpenRequest>) {
    while let Some(request) = incoming.recv().await {
        let app = app.clone();
        // Opening may have to start the lab first, which must not hold up
        // anybody else's console.
        tokio::spawn(async move {
            let OpenRequest {
                lab,
                cols,
                rows,
                reply,
            } = request;
            let _ = reply.send(open(&app, &lab, cols, rows).await);
        });
    }
}

/// Open a console on a lab node, starting the lab if it is stopped.
pub async fn open(app: &App, reference: &LabRef, cols: u16, rows: u16) -> AppResult<Connection> {
    // A deleted lab is a `not_found`, not a silently started empty one.
    let record = model::load(app, &reference.lab_id)?;
    let lab = runtime::start(app, &record.id).await?;
    let node = lab
        .with(|sim| sim.node_by_name(&reference.node))
        .ok_or_else(|| {
            AppError::not_found(format!(
                "node `{}` in lab `{}`",
                reference.node,
                record.name()
            ))
        })?;
    let console = lab
        .with(|sim| {
            let console = sim.console_open(node)?;
            sim.console_resize(console, cols.max(1), rows.max(1));
            Ok::<ConsoleId, nocturne_netsim::LabError>(console)
        })
        .map_err(model::sim_error)?;

    let info = ConnectionInfo {
        kind: Some(TransportKind::Sim),
        peer: format!("lab:{}/{}", record.name(), reference.node),
        ..ConnectionInfo::default()
    };
    let (connection, backend) = Connection::pair(info, Connection::DEFAULT_BUFFER);
    tokio::spawn(bridge(lab, node, console, backend));
    Ok(connection)
}

/// Carry bytes between a connection and a console until either end closes.
async fn bridge(
    lab: Arc<RunningLab>,
    node: nocturne_netsim::NodeId,
    console: ConsoleId,
    mut backend: Backend,
) {
    let mut wake = lab.wake();
    // Cancelled when the lab stops and when this node is removed.
    let closing = lab.register_console(node);
    let reason = loop {
        let output = lab.with(|sim| sim.console_output(console));
        if !output.is_empty()
            && backend
                .output
                .send(Output::Data(output.into()))
                .await
                .is_err()
        {
            // The application dropped its reader: nothing left to say.
            break None;
        }
        tokio::select! {
            _ = closing.cancelled() => break Some(if lab.cancel.is_cancelled() {
                lab.closing_reason()
            } else {
                "the node was removed from the lab".to_string()
            }),
            // The sender lives in the lab this task holds, so this only ends
            // if something went very wrong; treat it as the lab being gone.
            woken = wake.changed() => if woken.is_err() {
                break Some(lab.closing_reason());
            },
            input = backend.input.recv() => match input {
                Some(Input::Data(bytes)) => {
                    let paused = lab.speed() <= 0.0;
                    lab.with(|sim| {
                        sim.console_input(console, &bytes);
                        if !paused {
                            sim.advance(INPUT_STEP_US);
                        }
                    });
                    // The typing may have been `write memory`, which no
                    // device announces.
                    lab.measure(node);
                }
                // A break on a console line is what Ctrl-C is to the device.
                Some(Input::Break) => lab.with(|sim| sim.console_input(console, &[0x03])),
                Some(Input::Resize { cols, rows }) => {
                    lab.with(|sim| sim.console_resize(console, cols.max(1), rows.max(1)));
                }
                Some(Input::Close) | None => break None,
            },
        }
    };
    closing.cancel();
    lab.with(|sim| sim.console_close(console));
    let _ = backend.output.send(Output::Closed { reason }).await;
}
