//! Links a person drew: the unmanaged switch in a riser, a WAN circuit, the
//! fibre that no protocol crosses.

use std::collections::BTreeSet;

use nocturne_app::error::{AppError, AppResult};
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, inventory};
use nocturne_drivers::ifname;
use serde::{Deserialize, Serialize};

use crate::data::Collected;
use crate::model::{Confidence, End, Link, LinkKind, Node, NodeKind, Status};

const COLLECTION: &str = "topology.manual";

/// One end of a drawn link: a device, or a name for something that is not one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManualEnd {
    /// A device id, name or address; resolved when the link is saved.
    pub device: Option<String>,
    /// For an end that is not a device: what to call it.
    pub name: Option<String>,
    pub interface: Option<String>,
    /// What to draw a non-device end as; `cloud` by default.
    pub kind: Option<NodeKind>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManualLink {
    pub id: String,
    pub a: ManualEnd,
    pub b: ManualEnd,
    pub label: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub fn list(app: &App) -> AppResult<Vec<ManualLink>> {
    let mut links: Vec<ManualLink> = app.store().list(COLLECTION)?;
    links.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    Ok(links)
}

/// Check both ends and store the link.
pub fn save(app: &App, mut link: ManualLink) -> AppResult<ManualLink> {
    for (side, end) in [("a", &mut link.a), ("b", &mut link.b)] {
        match &end.device {
            Some(reference) if !reference.trim().is_empty() => {
                let device = inventory::find(app, reference.trim())?;
                end.device = Some(device.id);
                end.name = None;
            }
            _ => {
                end.device = None;
                let name = end.name.clone().unwrap_or_default().trim().to_string();
                if name.is_empty() {
                    return Err(AppError::invalid(format!(
                        "end `{side}` of a manual link needs either a device or a name"
                    )));
                }
                end.name = Some(name);
            }
        }
        end.interface = end
            .interface
            .as_ref()
            .map(|name| name.trim())
            .filter(|name| !name.is_empty())
            .map(ifname::long);
    }
    if link.a.device.is_some()
        && link.a.device == link.b.device
        && link.a.interface == link.b.interface
    {
        return Err(AppError::invalid("a manual link needs two different ends"));
    }
    let now = now_ms();
    if link.id.is_empty() {
        link.id = new_id();
        link.created_at = now;
    } else if let Some(existing) = app.store().get::<ManualLink>(COLLECTION, &link.id)? {
        link.created_at = existing.created_at;
    } else {
        link.created_at = now;
    }
    link.updated_at = now;
    app.store().put(COLLECTION, &link.id, &link)?;
    Ok(link)
}

pub fn delete(app: &App, id: &str) -> AppResult<ManualLink> {
    let link: ManualLink = app.store().require(COLLECTION, id, "manual link")?;
    app.store().delete(COLLECTION, id)?;
    Ok(link)
}

/// A node id for an end that is not a device.
fn node_id(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();
    format!("m:{}", slug.trim_matches('-'))
}

/// The stored manual links as graph links, plus the nodes they invent.
pub fn links(
    app: &App,
    collected: &Collected,
    scope: &BTreeSet<String>,
) -> AppResult<(Vec<Link>, Vec<Node>)> {
    let mut graph_links = Vec::new();
    let mut nodes: Vec<Node> = Vec::new();
    for manual in list(app)? {
        let mut ends = Vec::new();
        let mut in_scope = true;
        for end in [&manual.a, &manual.b] {
            match &end.device {
                Some(device_id) => {
                    if !scope.contains(device_id) || collected.device(device_id).is_none() {
                        in_scope = false;
                        break;
                    }
                    ends.push(End::with_interface(
                        device_id.clone(),
                        end.interface.clone(),
                    ));
                }
                None => {
                    let name = end.name.clone().unwrap_or_default();
                    let id = node_id(&name);
                    if !nodes.iter().any(|node| node.id == id) {
                        nodes.push(Node {
                            id: id.clone(),
                            kind: end.kind.unwrap_or(NodeKind::Cloud),
                            name,
                            status: Status::Unknown,
                            unmanaged: true,
                            ..Node::default()
                        });
                    }
                    ends.push(End::with_interface(id, end.interface.clone()));
                }
            }
        }
        if !in_scope || ends.len() != 2 {
            continue;
        }
        ends.sort_by(|a, b| {
            (&a.node, a.interface.as_deref().unwrap_or(""))
                .cmp(&(&b.node, b.interface.as_deref().unwrap_or("")))
        });
        let mut link = Link {
            a: ends[0].clone(),
            b: ends[1].clone(),
            kind: LinkKind::Manual,
            // A person drew it; that is as good as both ends reporting it.
            confidence: Confidence::Confirmed,
            status: Status::Unknown,
            label: manual.label.clone(),
            ..Link::default()
        };
        link.id = crate::id_of(&format!("manual:{}", manual.id));
        graph_links.push(link);
    }
    // Only keep invented nodes that a link that survived actually uses.
    let used: BTreeSet<String> = graph_links
        .iter()
        .flat_map(|link| [link.a.node.clone(), link.b.node.clone()])
        .collect();
    nodes.retain(|node| used.contains(&node.id));
    Ok((graph_links, nodes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_stable_node_ids() {
        assert_eq!(node_id("Riser switch 3"), "m:riser-switch-3");
        assert_eq!(node_id("WAN / MPLS"), "m:wan---mpls");
    }
}
