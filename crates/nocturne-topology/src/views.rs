//! Saved pictures and where the nodes sit in them.

use std::collections::BTreeMap;

use nocturne_app::App;
use nocturne_app::error::{AppError, AppResult};
use nocturne_app::util::{new_id, now_ms};

use crate::layout;
use crate::model::{DEFAULT_VIEW, Graph, Position, View};

const VIEWS: &str = "topology.views";
const POSITIONS: &str = "topology.positions";

/// Every saved view, the built-in default first.
pub fn list(app: &App) -> AppResult<Vec<View>> {
    let mut saved: Vec<View> = app.store().list(VIEWS)?;
    saved.sort_by_key(|view| view.name.to_lowercase());
    if !saved.iter().any(|view| view.id == DEFAULT_VIEW) {
        saved.insert(0, View::default());
    }
    Ok(saved)
}

/// A view by id. The default view exists whether or not it was ever saved.
pub fn get(app: &App, id: &str) -> AppResult<View> {
    if let Some(view) = app.store().get::<View>(VIEWS, id)? {
        return Ok(view);
    }
    if id == DEFAULT_VIEW {
        return Ok(View::default());
    }
    Err(AppError::not_found(format!("view `{id}`")))
}

pub fn save(app: &App, mut view: View) -> AppResult<View> {
    view.name = view.name.trim().to_string();
    if view.name.is_empty() {
        return Err(AppError::invalid("a view needs a name"));
    }
    if view.id.is_empty() {
        view.id = new_id();
    }
    let now = now_ms();
    let existing = app.store().get::<View>(VIEWS, &view.id)?;
    view.created_at = existing.map_or(now, |existing| existing.created_at);
    view.updated_at = now;
    app.store().put(VIEWS, &view.id, &view)?;
    Ok(view)
}

pub fn delete(app: &App, id: &str) -> AppResult<bool> {
    if id == DEFAULT_VIEW {
        return Err(AppError::invalid(
            "the default view cannot be deleted; save over it instead",
        ));
    }
    let removed = app.store().delete(VIEWS, id)?;
    app.store().delete(POSITIONS, id)?;
    Ok(removed)
}

/// The stored positions of one view.
pub fn positions(app: &App, view: &str) -> AppResult<BTreeMap<String, Position>> {
    Ok(app.store().get(POSITIONS, view)?.unwrap_or_default())
}

/// Store positions, merging with what is already there.
pub fn save_positions(
    app: &App,
    view: &str,
    changes: BTreeMap<String, Position>,
) -> AppResult<BTreeMap<String, Position>> {
    let mut stored = positions(app, view)?;
    for (node, position) in changes {
        stored.insert(node, position);
    }
    app.store().put(POSITIONS, view, &stored)?;
    Ok(stored)
}

/// Forget the positions of one view.
pub fn clear_positions(app: &App, view: &str) -> AppResult<()> {
    app.store().delete(POSITIONS, view)?;
    Ok(())
}

/// Put every node somewhere sensible: stored positions where there are any,
/// a full automatic layout where there are none, and new nodes next to their
/// neighbours without moving anything that is already placed.
pub fn apply_positions(app: &App, view: &str, graph: &mut Graph) -> AppResult<()> {
    let stored = positions(app, view)?;
    if stored.is_empty() {
        let config: crate::model::Settings =
            nocturne_app::settings::get(app, crate::model::SETTINGS_SECTION)?;
        let placed = layout::run(graph, config.default_algorithm, &BTreeMap::new(), None);
        apply(graph, &placed);
        return Ok(());
    }
    apply(graph, &stored);
    let missing: Vec<String> = graph
        .nodes
        .iter()
        .filter(|node| !stored.contains_key(&node.id))
        .map(|node| node.id.clone())
        .collect();
    if !missing.is_empty() {
        let placed = layout::place_near_neighbours(graph, &missing);
        apply(graph, &placed);
    }
    Ok(())
}

fn apply(graph: &mut Graph, positions: &BTreeMap<String, Position>) {
    for node in &mut graph.nodes {
        if let Some(position) = positions.get(&node.id) {
            node.x = position.x;
            node.y = position.y;
            node.pinned = position.pinned;
        }
    }
}
