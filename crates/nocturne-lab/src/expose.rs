//! `labs.expose`: a real TCP port bridged to a node's console, so PuTTY,
//! Ansible or a monitoring tool can reach a simulated device.
//!
//! The servers hold the console opener, never an `App`: an `App` reachable
//! from the app's own state map would keep the application alive for ever.
//! A task started with the shutdown token (and holding no `App` either) closes
//! every exposed port when the application stops.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use nocturne_app::model::LabRef;
use nocturne_app::util::new_id;
use nocturne_app::{App, AppError, AppResult, Router};
use nocturne_transport::Connection;
use nocturne_transport::serve::{
    ServerHandle, SessionHandler, SshServerConfig, serve_ssh, serve_telnet,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::console::Opener;
use crate::model;

#[derive(Default)]
pub struct ExposeState {
    opener: OnceLock<Opener>,
    servers: Mutex<HashMap<String, Exposure>>,
}

struct Exposure {
    row: ExposedRow,
    handle: Option<ServerHandle>,
}

/// One exposed console, as `labs.exposed` lists it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExposedRow {
    pub id: String,
    pub lab_id: String,
    pub lab: String,
    pub node: String,
    /// `ssh` or `telnet`.
    pub protocol: String,
    /// `127.0.0.1:34567`, what a client connects to.
    pub address: String,
    pub opened_at: i64,
}

/// Remember how to open consoles, and close every port at shutdown.
pub fn install(app: &App, opener: Opener) {
    let state = app.state::<ExposeState>();
    let _ = state.opener.set(opener);
    let token = app.shutdown_token();
    let state = state.clone();
    // Not `app.spawn`: that drops the task at shutdown, and this task's whole
    // job is what happens then. It holds no `App`, so nothing is kept alive.
    tokio::spawn(async move {
        token.cancelled().await;
        close_all(&state).await;
    });
}

async fn close_all(state: &ExposeState) {
    let handles: Vec<ServerHandle> = {
        let mut servers = state.servers.lock();
        servers
            .drain()
            .filter_map(|(_, mut exposure)| exposure.handle.take())
            .collect()
    };
    for handle in handles {
        handle.shutdown().await;
    }
}

/// Close everything exposed for one lab (it was deleted).
pub async fn close_lab(app: &App, lab_id: &str) {
    let state = app.state::<ExposeState>();
    let handles: Vec<ServerHandle> = {
        let mut servers = state.servers.lock();
        let ids: Vec<String> = servers
            .iter()
            .filter(|(_, exposure)| exposure.row.lab_id == lab_id)
            .map(|(id, _)| id.clone())
            .collect();
        ids.iter()
            .filter_map(|id| {
                servers
                    .get_mut(id)
                    .and_then(|exposure| exposure.handle.take())
            })
            .collect()
    };
    state
        .servers
        .lock()
        .retain(|_, exposure| exposure.row.lab_id != lab_id);
    for handle in handles {
        handle.shutdown().await;
    }
}

/// Opens a console for every client the little server accepts.
struct ConsoleHandler {
    opener: Opener,
    lab: LabRef,
}

#[async_trait]
impl SessionHandler for ConsoleHandler {
    async fn open(
        &self,
        _peer: SocketAddr,
        _user: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Option<Connection> {
        match self.opener.open(&self.lab, cols, rows).await {
            Ok(connection) => Some(connection),
            Err(err) => {
                tracing::warn!(%err, node = %self.lab.node, "a client could not be given a lab console");
                None
            }
        }
    }
}

#[derive(Deserialize)]
struct ExposeParams {
    lab_id: String,
    node: String,
    protocol: String,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    bind: Option<String>,
}

#[derive(Deserialize)]
struct UnexposeParams {
    id: String,
}

async fn expose(app: &App, params: ExposeParams) -> AppResult<ExposedRow> {
    let record = model::load(app, &params.lab_id)?;
    let protocol = params.protocol.to_lowercase();
    if protocol != "ssh" && protocol != "telnet" {
        return Err(AppError::invalid(format!(
            "`{}` is not a protocol to expose a console on; use `ssh` or `telnet`",
            params.protocol
        )));
    }
    // The node must exist; starting the lab is left to the first client.
    let node_exists = record
        .spec
        .nodes
        .iter()
        .any(|node| node.name == params.node.trim());
    if !node_exists {
        return Err(AppError::not_found(format!(
            "node `{}` in lab `{}`",
            params.node,
            record.name()
        )));
    }
    if crate::devices::node_profile(
        &record
            .spec
            .nodes
            .iter()
            .find(|node| node.name == params.node.trim())
            .map(|node| node.model.clone())
            .unwrap_or_default(),
    )
    .is_none()
    {
        return Err(AppError::invalid(format!(
            "`{}` has no command line, so there is no console to expose",
            params.node
        )));
    }

    let settings = model::settings(app);
    let bind = params.bind.unwrap_or(settings.expose_bind);
    let address: IpAddr = bind.trim().parse().map_err(|_| {
        AppError::invalid(format!(
            "`{bind}` is not an IP address to listen on; use 127.0.0.1 or 0.0.0.0"
        ))
    })?;
    if !address.is_loopback() {
        app.require_local_access("exposing a lab console to the network")?;
    }
    let socket = SocketAddr::new(address, params.port.unwrap_or(0));

    let opener = app
        .state::<ExposeState>()
        .opener
        .get()
        .cloned()
        .ok_or_else(|| AppError::unavailable("the lab simulator is not available"))?;
    let handler = Arc::new(ConsoleHandler {
        opener,
        lab: LabRef {
            lab_id: record.id.clone(),
            node: params.node.trim().to_string(),
        },
    });
    let handle = match protocol.as_str() {
        "ssh" => serve_ssh(
            SshServerConfig {
                bind: socket,
                // A lab console has no login of its own: the simulated device
                // asks for one if its configuration says so.
                users: Vec::new(),
                host_key: None,
                banner: None,
            },
            handler,
        )
        .await
        .map_err(AppError::from)?,
        _ => serve_telnet(socket, handler)
            .await
            .map_err(AppError::from)?,
    };

    let row = ExposedRow {
        id: new_id(),
        lab_id: record.id.clone(),
        lab: record.name().to_string(),
        node: params.node.trim().to_string(),
        protocol: protocol.clone(),
        address: handle.local_addr().to_string(),
        opened_at: nocturne_app::util::now_ms(),
    };
    app.state::<ExposeState>().servers.lock().insert(
        row.id.clone(),
        Exposure {
            row: row.clone(),
            handle: Some(handle),
        },
    );
    app.audit(
        "labs.expose",
        None,
        format!(
            "Exposed the console of `{}/{}` over {protocol} on {}",
            record.name(),
            row.node,
            row.address
        ),
        json!({"lab_id": record.id, "node": row.node, "address": row.address}),
    );
    Ok(row)
}

pub fn init(router: &mut Router) {
    router.add("labs.expose", |app: App, params: ExposeParams| async move {
        expose(&app, params).await
    });
    router.add(
        "labs.exposed",
        |app: App, _: nocturne_app::NoParams| async move {
            let mut rows: Vec<ExposedRow> = app
                .state::<ExposeState>()
                .servers
                .lock()
                .values()
                .map(|exposure| exposure.row.clone())
                .collect();
            rows.sort_by_key(|row| row.opened_at);
            Ok(rows)
        },
    );
    router.add(
        "labs.unexpose",
        |app: App, params: UnexposeParams| async move {
            let exposure = app.state::<ExposeState>().servers.lock().remove(&params.id);
            let Some(mut exposure) = exposure else {
                return Err(AppError::not_found(format!(
                    "exposed console `{}`",
                    params.id
                )));
            };
            if let Some(handle) = exposure.handle.take() {
                handle.shutdown().await;
            }
            app.audit(
                "labs.unexpose",
                None,
                format!(
                    "Stopped serving the console of `{}/{}` on {}",
                    exposure.row.lab, exposure.row.node, exposure.row.address
                ),
                json!({"lab_id": exposure.row.lab_id, "node": exposure.row.node}),
            );
            Ok(json!({"closed": true}))
        },
    );
}
