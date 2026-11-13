//! What changed: snapshots of the physical graph, stored whenever its shape
//! changes, and the difference between two of them.

use std::collections::BTreeMap;

use nocturne_app::App;
use nocturne_app::error::AppResult;
use nocturne_app::settings;
use nocturne_app::util::{new_id, now_ms};
use serde::{Deserialize, Serialize};

use crate::graph::{self, Options};
use crate::model::{Graph, Layer, SETTINGS_SECTION, Settings};

const COLLECTION: &str = "topology.snapshots";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SnapshotNode {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    pub kind: String,
    pub unmanaged: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SnapshotLink {
    /// The link's identity, independent of which end is `a`.
    pub signature: String,
    pub a_node: String,
    pub a_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a_interface: Option<String>,
    pub b_node: String,
    pub b_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub b_interface: Option<String>,
    pub kind: String,
    pub confidence: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    pub id: String,
    pub at: i64,
    pub nodes: Vec<SnapshotNode>,
    pub links: Vec<SnapshotLink>,
}

impl Snapshot {
    /// What the shape of the graph is, for "has anything changed".
    fn shape(&self) -> String {
        let mut nodes: Vec<&str> = self.nodes.iter().map(|node| node.id.as_str()).collect();
        nodes.sort_unstable();
        let mut links: Vec<&str> = self
            .links
            .iter()
            .map(|link| link.signature.as_str())
            .collect();
        links.sort_unstable();
        format!("{}|{}", nodes.join(","), links.join(","))
    }
}

fn of(graph: &Graph) -> Snapshot {
    let names: BTreeMap<&str, &str> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.name.as_str()))
        .collect();
    let name_of = |id: &str| names.get(id).copied().unwrap_or(id).to_string();
    Snapshot {
        id: String::new(),
        at: graph.generated_at,
        nodes: graph
            .nodes
            .iter()
            .map(|node| SnapshotNode {
                id: node.id.clone(),
                name: node.name.clone(),
                device_id: node.device_id.clone(),
                kind: node.kind.as_str().to_string(),
                unmanaged: node.unmanaged,
            })
            .collect(),
        links: graph
            .links
            .iter()
            .map(|link| SnapshotLink {
                signature: link.signature(),
                a_node: link.a.node.clone(),
                a_name: name_of(&link.a.node),
                a_interface: link.a.interface.clone(),
                b_node: link.b.node.clone(),
                b_name: name_of(&link.b.node),
                b_interface: link.b.interface.clone(),
                kind: link.kind.as_str().to_string(),
                confidence: link.confidence.as_str().to_string(),
            })
            .collect(),
    }
}

/// Every stored snapshot, oldest first.
pub fn snapshots(app: &App) -> AppResult<Vec<Snapshot>> {
    let mut stored: Vec<Snapshot> = app.store().list(COLLECTION)?;
    stored.sort_by(|a, b| a.at.cmp(&b.at).then(a.id.cmp(&b.id)));
    Ok(stored)
}

/// Store a snapshot of the physical graph if its shape has changed since the
/// last one. Returns whether anything was stored.
pub async fn record(app: &App) -> AppResult<bool> {
    let config: Settings = settings::get(app, SETTINGS_SECTION)?;
    let graph = graph::build(
        app,
        &Options {
            layer: Layer::Physical,
            ..Options::default()
        },
    )
    .await?;
    let mut snapshot = of(&graph);
    let stored = snapshots(app)?;
    if stored
        .last()
        .is_some_and(|previous| previous.shape() == snapshot.shape())
    {
        return Ok(false);
    }
    snapshot.id = new_id();
    // Two snapshots in the same millisecond cannot be told apart by `since`,
    // so the clock is nudged forward rather than allowed to collide.
    snapshot.at = match stored.last() {
        Some(previous) => now_ms().max(previous.at + 1),
        None => now_ms(),
    };
    app.store().put(COLLECTION, &snapshot.id, &snapshot)?;
    let keep = config.max_snapshots.max(2) as usize;
    if stored.len() + 1 > keep {
        for old in &stored[..stored.len() + 1 - keep] {
            app.store().delete(COLLECTION, &old.id)?;
        }
    }
    Ok(true)
}

#[derive(Debug, Serialize)]
pub struct Changes {
    pub since: i64,
    /// The snapshot the comparison started from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_at: Option<i64>,
    pub snapshots: usize,
    pub nodes_added: Vec<SnapshotNode>,
    pub nodes_removed: Vec<SnapshotNode>,
    pub links_added: Vec<SnapshotLink>,
    pub links_removed: Vec<SnapshotLink>,
    /// What the comparison could and could not say.
    pub notes: Vec<String>,
}

/// What appeared and disappeared since a moment in time.
pub fn changes(app: &App, since: i64) -> AppResult<Changes> {
    let stored = snapshots(app)?;
    let mut changes = Changes {
        since,
        from_at: None,
        to_at: None,
        snapshots: stored.len(),
        nodes_added: Vec::new(),
        nodes_removed: Vec::new(),
        links_added: Vec::new(),
        links_removed: Vec::new(),
        notes: Vec::new(),
    };
    let Some(latest) = stored.last() else {
        changes.notes.push(
            "no topology snapshot has been stored yet; one is stored the first time the graph \
             changes after a collection"
                .to_string(),
        );
        return Ok(changes);
    };
    changes.to_at = Some(latest.at);
    let earlier = stored.iter().rev().find(|snapshot| snapshot.at <= since);
    let from = match earlier {
        Some(from) => from,
        None => {
            changes.notes.push(format!(
                "no snapshot is as old as the time asked for; the oldest one kept is from {}",
                crate::export::timestamp(stored[0].at)
            ));
            &stored[0]
        }
    };
    changes.from_at = Some(from.at);
    if from.at == latest.at {
        changes
            .notes
            .push("the graph has not changed since then".to_string());
        return Ok(changes);
    }

    let before: BTreeMap<&str, &SnapshotNode> = from
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let after: BTreeMap<&str, &SnapshotNode> = latest
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    changes.nodes_added = after
        .iter()
        .filter(|(id, _)| !before.contains_key(*id))
        .map(|(_, node)| (*node).clone())
        .collect();
    changes.nodes_removed = before
        .iter()
        .filter(|(id, _)| !after.contains_key(*id))
        .map(|(_, node)| (*node).clone())
        .collect();

    let before_links: BTreeMap<&str, &SnapshotLink> = from
        .links
        .iter()
        .map(|link| (link.signature.as_str(), link))
        .collect();
    let after_links: BTreeMap<&str, &SnapshotLink> = latest
        .links
        .iter()
        .map(|link| (link.signature.as_str(), link))
        .collect();
    changes.links_added = after_links
        .iter()
        .filter(|(signature, _)| !before_links.contains_key(*signature))
        .map(|(_, link)| (*link).clone())
        .collect();
    changes.links_removed = before_links
        .iter()
        .filter(|(signature, _)| !after_links.contains_key(*signature))
        .map(|(_, link)| (*link).clone())
        .collect();
    Ok(changes)
}
