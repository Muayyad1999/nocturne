//! The RPC surface and the `topology.changed` event.

use std::collections::BTreeMap;
use std::time::Duration;

use nocturne_app::error::{AppError, AppResult};
use nocturne_app::{App, AppBuilder, NoParams, settings};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::export::{self, Format};
use crate::graph::{self, Options};
use crate::model::{
    Algorithm, Confidence, DEFAULT_VIEW, Filter, Graph, Layer, LinkKind, Node, Position,
    SETTINGS_SECTION, Settings, Status, View,
};
use crate::{history, layout, locate, manual, path, views};

/// How long to wait for the dust to settle before redrawing.
const DEBOUNCE: Duration = Duration::from_secs(1);

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GetParams {
    pub view: Option<String>,
    pub layer: Option<Layer>,
    pub filter: Option<Filter>,
    pub include_endpoints: Option<bool>,
}

impl GetParams {
    /// A saved view gives the defaults; anything stated in the call wins.
    fn options(&self, app: &App) -> AppResult<Options> {
        let id = self
            .view
            .clone()
            .filter(|view| !view.is_empty())
            .unwrap_or_else(|| DEFAULT_VIEW.to_string());
        let view = views::get(app, &id)?;
        let mut options = Options::from_view(&view);
        if let Some(layer) = self.layer {
            options.layer = layer;
        }
        if let Some(filter) = &self.filter {
            options.filter = filter.clone();
        }
        if let Some(include) = self.include_endpoints {
            options.include_endpoints = include;
        }
        Ok(options)
    }
}

#[derive(Debug, Deserialize)]
pub struct ViewSaveParams {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub layer: Layer,
    #[serde(default)]
    pub filter: Filter,
    #[serde(default)]
    pub include_endpoints: bool,
}

#[derive(Debug, Deserialize)]
pub struct IdParams {
    pub id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LayoutSaveParams {
    pub view: Option<String>,
    pub positions: BTreeMap<String, Position>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AutoLayoutParams {
    pub view: Option<String>,
    pub algorithm: Option<Algorithm>,
    pub layer: Option<Layer>,
    pub filter: Option<Filter>,
    pub include_endpoints: Option<bool>,
    /// For `radial`: the node to put in the middle.
    pub root: Option<String>,
    /// Store the result as the view's positions (the default).
    pub save: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct NeighborsParams {
    #[serde(alias = "id")]
    pub device_id: String,
    #[serde(default)]
    pub include_endpoints: bool,
}

#[derive(Debug, Deserialize)]
pub struct ChangesParams {
    pub since: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExportParams {
    pub view: Option<String>,
    pub layer: Option<Layer>,
    pub filter: Option<Filter>,
    pub include_endpoints: Option<bool>,
    pub format: Format,
}

/// One row of a device's "connected to" table.
#[derive(Debug, Serialize)]
pub struct NeighborRow {
    pub link_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_interface: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_ip: Option<String>,
    pub peer: Node,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_interface: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_ip: Option<String>,
    pub kind: LinkKind,
    pub confidence: Confidence,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_bps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub util_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vlans: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub protocols: Vec<String>,
    pub members: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Neighbors {
    pub device_id: String,
    pub device: String,
    pub rows: Vec<NeighborRow>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Exported {
    pub format: String,
    pub filename: String,
    pub text: String,
}

fn view_id(view: &Option<String>) -> String {
    view.clone()
        .filter(|view| !view.is_empty())
        .unwrap_or_else(|| DEFAULT_VIEW.to_string())
}

/// The device's links, with the node at the other end resolved.
fn neighbors(graph: &Graph, device_id: &str) -> Vec<NeighborRow> {
    let mut rows = Vec::new();
    for link in &graph.links {
        let (near, far) = if link.a.node == device_id {
            (&link.a, &link.b)
        } else if link.b.node == device_id {
            (&link.b, &link.a)
        } else {
            continue;
        };
        let Some(peer) = graph.node(&far.node) else {
            continue;
        };
        rows.push(NeighborRow {
            link_id: link.id.clone(),
            local_interface: near.interface.clone(),
            local_ip: near.ip.clone(),
            peer: peer.clone(),
            peer_interface: far.interface.clone(),
            peer_ip: far.ip.clone(),
            kind: link.kind,
            confidence: link.confidence,
            status: link.status,
            speed_bps: link.speed_bps,
            util_pct: link.util_pct,
            vlans: link.vlans.clone(),
            aggregate: link.aggregate.clone(),
            label: link.label.clone(),
            protocols: link.protocols.clone(),
            members: link.members.clone(),
        });
    }
    rows.sort_by_key(|row| {
        (
            row.local_interface
                .as_deref()
                .map(nocturne_drivers::ifname::sort_key),
            row.peer.name.to_lowercase(),
        )
    });
    rows
}

/// Redraw and announce, once the events have stopped arriving.
///
/// The receiver is made when the app is built, not when this task first runs,
/// so nothing published in between is missed.
async fn watcher(app: App, mut events: tokio::sync::broadcast::Receiver<nocturne_app::Event>) {
    use tokio::sync::broadcast::error::RecvError;
    // What a topic means for the graph; the most telling one wins when several
    // arrive in the same window.
    fn reason_of(topic: &str) -> Option<(u8, &'static str)> {
        match topic {
            "collect.updated" => Some((3, "collect")),
            "labs.changed" => Some((2, "lab")),
            "devices.changed" => Some((1, "inventory")),
            _ => None,
        }
    }

    loop {
        let mut reason = loop {
            match events.recv().await {
                Ok(event) => match reason_of(&event.topic) {
                    Some(reason) => break reason,
                    None => continue,
                },
                Err(RecvError::Lagged(_)) => break (0, "events"),
                Err(RecvError::Closed) => return,
            }
        };
        // Everything that arrives while the dust settles is part of the same change.
        let settle = tokio::time::sleep(DEBOUNCE);
        tokio::pin!(settle);
        loop {
            tokio::select! {
                () = &mut settle => break,
                received = events.recv() => match received {
                    Ok(event) => {
                        if let Some(other) = reason_of(&event.topic)
                            && other.0 > reason.0
                        {
                            reason = other;
                        }
                        continue;
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return,
                },
            }
        }
        let reason = reason.1;
        match history::record(&app).await {
            Ok(_) => app.publish("topology.changed", &json!({"reason": reason})),
            Err(err) => tracing::debug!(%err, "topology: the graph could not be rebuilt"),
        }
    }
}

pub fn init(builder: &mut AppBuilder) {
    settings::declare(builder, SETTINGS_SECTION, &Settings::default());
    let router = &mut builder.router;

    router.add("topology.get", |app: App, params: GetParams| async move {
        let options = params.options(&app)?;
        graph::build(&app, &options).await
    });

    router.add("topology.views", |app: App, _: NoParams| async move {
        views::list(&app)
    });
    router.add(
        "topology.view_save",
        |app: App, params: ViewSaveParams| async move {
            let view = views::save(
                &app,
                View {
                    id: params.id.unwrap_or_default(),
                    name: params.name,
                    layer: params.layer,
                    filter: params.filter,
                    include_endpoints: params.include_endpoints,
                    created_at: 0,
                    updated_at: 0,
                },
            )?;
            app.audit(
                "topology.view_save",
                None,
                format!("Saved topology view `{}`", view.name),
                json!({"view": view.id, "layer": view.layer}),
            );
            app.publish("topology.changed", &json!({"reason": "view"}));
            Ok(view)
        },
    );
    router.add(
        "topology.view_delete",
        |app: App, params: IdParams| async move {
            let name = views::get(&app, &params.id)
                .map(|view| view.name)
                .unwrap_or_else(|_| params.id.clone());
            let removed = views::delete(&app, &params.id)?;
            if removed {
                app.audit(
                    "topology.view_delete",
                    None,
                    format!("Deleted topology view `{name}`"),
                    json!({"view": params.id}),
                );
                app.publish("topology.changed", &json!({"reason": "view"}));
            }
            Ok(json!({"deleted": removed}))
        },
    );

    router.add(
        "topology.layout.save",
        |app: App, params: LayoutSaveParams| async move {
            let view = view_id(&params.view);
            if params.positions.is_empty() {
                return Err(AppError::invalid("no positions were given to save"));
            }
            let stored = views::save_positions(&app, &view, params.positions)?;
            app.publish(
                "topology.changed",
                &json!({"reason": "layout", "view": view}),
            );
            Ok(json!({"view": view, "positions": stored}))
        },
    );
    router.add(
        "topology.autolayout",
        |app: App, params: AutoLayoutParams| async move {
            let view = view_id(&params.view);
            let config: Settings = settings::get(&app, SETTINGS_SECTION)?;
            let options = GetParams {
                view: Some(view.clone()),
                layer: params.layer,
                filter: params.filter.clone(),
                include_endpoints: params.include_endpoints,
            }
            .options(&app)?;
            let graph = graph::build(&app, &options).await?;
            let algorithm = params.algorithm.unwrap_or(config.default_algorithm);
            // Pinned nodes stay exactly where the person put them.
            let pinned: BTreeMap<String, Position> = views::positions(&app, &view)?
                .into_iter()
                .filter(|(_, position)| position.pinned)
                .collect();
            let root = params.root.as_deref().and_then(|root| {
                graph
                    .nodes
                    .iter()
                    .find(|node| {
                        node.id == root
                            || node.name.eq_ignore_ascii_case(root)
                            || node.device_id.as_deref() == Some(root)
                    })
                    .map(|node| node.id.clone())
            });
            if params.root.is_some() && root.is_none() {
                return Err(AppError::not_found(format!(
                    "node `{}` is not in this view",
                    params.root.unwrap_or_default()
                )));
            }
            let positions = layout::run(&graph, algorithm, &pinned, root.as_deref());
            if params.save.unwrap_or(true) {
                views::clear_positions(&app, &view)?;
                views::save_positions(&app, &view, positions.clone())?;
                app.publish(
                    "topology.changed",
                    &json!({"reason": "layout", "view": view}),
                );
            }
            Ok(json!({
                "view": view,
                "algorithm": algorithm,
                "positions": positions,
            }))
        },
    );

    router.add(
        "topology.neighbors",
        |app: App, params: NeighborsParams| async move {
            let device = nocturne_app::inventory::find(&app, &params.device_id)?;
            let graph = graph::build(
                &app,
                &Options {
                    layer: Layer::Physical,
                    include_endpoints: params.include_endpoints,
                    ..Options::default()
                },
            )
            .await?;
            let rows = neighbors(&graph, &device.id);
            let warnings = graph
                .warnings
                .into_iter()
                .filter(|warning| warning.contains(&device.name))
                .collect();
            Ok(Neighbors {
                device_id: device.id,
                device: device.name,
                rows,
                warnings,
            })
        },
    );

    router.add(
        "topology.path",
        |app: App, params: path::Params| async move { path::trace(&app, &params) },
    );
    router.add(
        "locate.host",
        |app: App, params: locate::Params| async move { locate::locate(&app, &params) },
    );
    router.add(
        "topology.changes",
        |app: App, params: ChangesParams| async move { history::changes(&app, params.since) },
    );

    router.add(
        "topology.export",
        |app: App, params: ExportParams| async move {
            let options = GetParams {
                view: params.view.clone(),
                layer: params.layer,
                filter: params.filter.clone(),
                include_endpoints: params.include_endpoints,
            }
            .options(&app)?;
            let graph = graph::build(&app, &options).await?;
            let text = export::render(&graph, params.format)?;
            Ok(Exported {
                format: params.format.as_str().to_string(),
                filename: format!(
                    "topology-{}.{}",
                    graph.layer.as_str(),
                    params.format.extension()
                ),
                text,
            })
        },
    );

    router.add("topology.manual.list", |app: App, _: NoParams| async move {
        manual::list(&app)
    });
    router.add(
        "topology.manual.save",
        |app: App, params: manual::ManualLink| async move {
            let link = manual::save(&app, params)?;
            app.audit(
                "topology.manual.save",
                None,
                "Saved a manual topology link".to_string(),
                json!({"link": link.id}),
            );
            app.publish("topology.changed", &json!({"reason": "manual"}));
            Ok(link)
        },
    );
    router.add(
        "topology.manual.delete",
        |app: App, params: IdParams| async move {
            let link = manual::delete(&app, &params.id)?;
            app.audit(
                "topology.manual.delete",
                None,
                "Deleted a manual topology link".to_string(),
                json!({"link": link.id}),
            );
            app.publish("topology.changed", &json!({"reason": "manual"}));
            Ok(json!({"deleted": true}))
        },
    );

    builder.on_start(|app| {
        let events = app.subscribe();
        app.spawn("topology.watcher", watcher(app.clone(), events));
    });
}
