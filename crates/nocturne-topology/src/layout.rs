//! Putting the graph on paper.
//!
//! Every algorithm here is a pure function of the graph: the same graph gives
//! the same picture, on any machine, in any order. Nothing random, nothing
//! that depends on a hash map's iteration order.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::model::{Algorithm, Graph, Node, NodeKind, Position};

/// How much room one node takes, for spacing and overlap checks.
pub const NODE_WIDTH: f64 = 190.0;
pub const NODE_HEIGHT: f64 = 64.0;
/// Smallest gap left between two nodes side by side.
pub const X_GAP: f64 = 48.0;
/// Distance between ranks, centre to centre.
pub const Y_GAP: f64 = 150.0;
/// Extra space between two sites in the same rank.
pub const SITE_GAP: f64 = 90.0;
/// Distance between the lines a rank too wide to fit is wrapped onto.
pub const SUB_ROW_GAP: f64 = 100.0;
const MARGIN: f64 = 80.0;

type Positions = BTreeMap<String, Position>;

/// Lay the graph out. `fixed` positions are kept exactly as they are and are
/// treated as obstacles; `root` is only used by `radial`.
pub fn run(
    graph: &Graph,
    algorithm: Algorithm,
    fixed: &Positions,
    root: Option<&str>,
) -> Positions {
    let adjacency = Adjacency::of(graph);
    let mut positions = match algorithm {
        Algorithm::Layered => layered(graph, &adjacency),
        Algorithm::Force => force(graph, &adjacency),
        Algorithm::Radial => radial(graph, &adjacency, root),
    };
    for (id, position) in fixed {
        if positions.contains_key(id) {
            positions.insert(id.clone(), *position);
        }
    }
    separate(&mut positions, fixed);
    normalise(&mut positions, fixed);
    positions
}

/// Place nodes that have no position yet next to the neighbours that do,
/// without moving anything already placed.
pub fn place_near_neighbours(graph: &Graph, missing: &[String]) -> Positions {
    let adjacency = Adjacency::of(graph);
    let placed: BTreeMap<String, Position> = graph
        .nodes
        .iter()
        .filter(|node| !missing.contains(&node.id))
        .map(|node| {
            (
                node.id.clone(),
                Position {
                    x: node.x,
                    y: node.y,
                    pinned: node.pinned,
                },
            )
        })
        .collect();
    let bottom = placed
        .values()
        .map(|position| position.y)
        .fold(f64::NEG_INFINITY, f64::max);
    let left = placed
        .values()
        .map(|position| position.x)
        .fold(f64::INFINITY, f64::min);
    let mut new: Positions = BTreeMap::new();
    let mut orphans = 0;
    for id in missing {
        let known: Vec<&Position> = adjacency
            .neighbours(id)
            .iter()
            .filter_map(|other| placed.get(other).or_else(|| new.get(other)))
            .collect();
        let position = if known.is_empty() {
            let position = Position {
                x: if left.is_finite() { left } else { MARGIN }
                    + f64::from(orphans) * (NODE_WIDTH + X_GAP),
                y: if bottom.is_finite() {
                    bottom + Y_GAP
                } else {
                    MARGIN
                },
                pinned: false,
            };
            orphans += 1;
            position
        } else {
            let count = known.len() as f64;
            Position {
                x: known.iter().map(|position| position.x).sum::<f64>() / count,
                y: known.iter().map(|position| position.y).sum::<f64>() / count + Y_GAP,
                pinned: false,
            }
        };
        // The spot a node wants is often taken, so look outwards from it for
        // the nearest free one rather than sliding it away across the drawing.
        let free = nearest_free(position, &placed, &new);
        new.insert(id.clone(), free);
    }
    new
}

/// The free place closest to `wanted`, searched outwards on a lattice so the
/// answer depends only on the drawing and not on the order of a hash map.
fn nearest_free(wanted: Position, placed: &Positions, new: &Positions) -> Position {
    let step_x = NODE_WIDTH + X_GAP;
    let step_y = NODE_HEIGHT + 36.0;
    let clear = |x: f64, y: f64| {
        placed
            .values()
            .chain(new.values())
            .all(|other| (other.x - x).abs() >= step_x * 0.82 || (other.y - y).abs() >= step_y)
    };
    for ring in 0..24_i32 {
        let mut candidates: Vec<(i32, i32)> = Vec::new();
        for dy in -ring..=ring {
            for dx in -ring..=ring {
                if dx.abs().max(dy.abs()) == ring {
                    candidates.push((dx, dy));
                }
            }
        }
        // Below first, then above, then sideways: a new switch usually hangs
        // off the one it is connected to.
        candidates.sort_by_key(|(dx, dy)| (dx.abs() + dy.abs(), -dy, dx.abs(), *dx));
        for (dx, dy) in candidates {
            let (x, y) = (
                wanted.x + f64::from(dx) * step_x,
                wanted.y + f64::from(dy) * step_y,
            );
            if clear(x, y) {
                return Position {
                    x,
                    y,
                    pinned: false,
                };
            }
        }
    }
    wanted
}

/// Neighbour lists, built once, without self-loops.
struct Adjacency {
    order: Vec<String>,
    neighbours: BTreeMap<String, Vec<String>>,
}

impl Adjacency {
    fn of(graph: &Graph) -> Adjacency {
        let order: Vec<String> = graph.nodes.iter().map(|node| node.id.clone()).collect();
        let known: BTreeSet<&String> = order.iter().collect();
        let mut neighbours: BTreeMap<String, Vec<String>> =
            order.iter().map(|id| (id.clone(), Vec::new())).collect();
        for link in &graph.links {
            if link.a.node == link.b.node
                || !known.contains(&link.a.node)
                || !known.contains(&link.b.node)
            {
                continue;
            }
            neighbours
                .entry(link.a.node.clone())
                .or_default()
                .push(link.b.node.clone());
            neighbours
                .entry(link.b.node.clone())
                .or_default()
                .push(link.a.node.clone());
        }
        for list in neighbours.values_mut() {
            list.sort();
            list.dedup();
        }
        Adjacency { order, neighbours }
    }

    fn neighbours(&self, id: &str) -> &[String] {
        self.neighbours
            .get(id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn degree(&self, id: &str) -> usize {
        self.neighbours(id).len()
    }

    /// The best starting point: most connections, ties broken by id.
    fn busiest(&self) -> Option<String> {
        self.order
            .iter()
            .max_by_key(|id| (self.degree(id), std::cmp::Reverse((*id).clone())))
            .cloned()
    }
}

/// Where a role puts a node: 0 is the top of the drawing.
fn role_rank(role: Option<&str>) -> Option<usize> {
    let role = role?.to_ascii_lowercase();
    match role.as_str() {
        "core" | "edge" | "wan" | "internet" | "isp" | "border" => Some(0),
        "firewall" | "security" | "router" | "edge-router" => Some(0),
        "distribution" | "dist" | "aggregation" | "agg" => Some(1),
        "access" => Some(2),
        "wireless" | "ap" | "access-point" | "wifi" | "server" | "hypervisor" | "host"
        | "endpoint" | "printer" | "camera" => Some(3),
        _ => None,
    }
}

/// Where a node's kind puts it when it has no role.
fn kind_rank(node: &Node) -> Option<usize> {
    match node.kind {
        NodeKind::Cloud => Some(0),
        NodeKind::Firewall | NodeKind::Router => Some(0),
        NodeKind::AccessPoint | NodeKind::Server | NodeKind::Host => Some(3),
        _ => None,
    }
}

/// Rank every node: by role where there is one, by distance from the busiest
/// node where there is not.
fn ranks(graph: &Graph, adjacency: &Adjacency) -> BTreeMap<String, usize> {
    let mut known: BTreeMap<String, usize> = BTreeMap::new();
    let mut roles = 0;
    for node in &graph.nodes {
        if let Some(rank) = role_rank(node.role.as_deref()) {
            roles += 1;
            known.insert(node.id.clone(), rank);
        } else if let Some(rank) = kind_rank(node) {
            known.insert(node.id.clone(), rank);
        }
    }
    // Without roles the hierarchy has to come from the graph itself.
    if roles == 0
        && let Some(start) = adjacency.busiest()
    {
        let mut distance: BTreeMap<String, usize> = BTreeMap::new();
        let mut queue = VecDeque::from([start.clone()]);
        distance.insert(start, 0);
        while let Some(current) = queue.pop_front() {
            let step = distance[&current] + 1;
            for next in adjacency.neighbours(&current) {
                if !distance.contains_key(next) {
                    distance.insert(next.clone(), step);
                    queue.push_back(next.clone());
                }
            }
        }
        let deepest = distance.values().copied().max().unwrap_or(0);
        for node in &graph.nodes {
            let rank = distance
                .get(&node.id)
                .copied()
                .unwrap_or(deepest + 1)
                .max(known.get(&node.id).copied().unwrap_or(0));
            known.insert(node.id.clone(), rank);
        }
    }

    // Anything still unranked sits one below its highest ranked neighbour.
    let mut assigned = known;
    for _ in 0..8 {
        let mut changed = false;
        for node in &graph.nodes {
            if assigned.contains_key(&node.id) {
                continue;
            }
            let above = adjacency
                .neighbours(&node.id)
                .iter()
                .filter_map(|other| assigned.get(other))
                .min()
                .copied();
            if let Some(rank) = above {
                assigned.insert(node.id.clone(), rank + 1);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let deepest = assigned.values().copied().max().unwrap_or(0);
    for node in &graph.nodes {
        assigned.entry(node.id.clone()).or_insert(deepest);
    }

    // Close the gaps so there are no empty rows.
    let used: BTreeSet<usize> = assigned.values().copied().collect();
    let compacted: BTreeMap<usize, usize> = used
        .into_iter()
        .enumerate()
        .map(|(index, rank)| (rank, index))
        .collect();
    assigned
        .into_iter()
        .map(|(id, rank)| (id, compacted[&rank]))
        .collect()
}

/// Ranked rows, ordered to cross as little as possible, sites kept together.
fn layered(graph: &Graph, adjacency: &Adjacency) -> Positions {
    let ranks = ranks(graph, adjacency);
    let by_id: BTreeMap<&str, &Node> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let depth = ranks.values().copied().max().unwrap_or(0) + 1;

    let sites: Vec<String> = {
        let mut sites: BTreeSet<String> = graph
            .nodes
            .iter()
            .filter_map(|node| node.site.clone())
            .collect();
        sites.insert(String::new());
        sites.into_iter().collect()
    };
    let site_rank = |node: &Node| -> usize {
        let site = node.site.clone().unwrap_or_default();
        sites.iter().position(|known| *known == site).unwrap_or(0)
    };

    let mut rows: Vec<Vec<String>> = vec![Vec::new(); depth];
    for node in &graph.nodes {
        rows[ranks[&node.id]].push(node.id.clone());
    }
    for row in &mut rows {
        row.sort_by_key(|id| {
            let node = by_id[id.as_str()];
            (
                site_rank(node),
                crate::graph_sort_key(&node.name),
                id.clone(),
            )
        });
    }

    // Barycentre sweeps: a node drifts towards the average position of what it
    // is connected to in the rank above, then the rank below, and so on.
    for sweep in 0..8 {
        let places: BTreeMap<String, f64> = rows
            .iter()
            .flat_map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(index, id)| (id.clone(), index as f64))
            })
            .collect();
        let downwards = sweep % 2 == 0;
        let order: Vec<usize> = if downwards {
            (0..depth).collect()
        } else {
            (0..depth).rev().collect()
        };
        for rank in order {
            let reference = if downwards {
                rank.checked_sub(1)
            } else {
                (rank + 1 < depth).then_some(rank + 1)
            };
            let Some(reference) = reference else { continue };
            let in_reference: BTreeSet<&str> = rows[reference].iter().map(String::as_str).collect();
            let mut keyed: Vec<(usize, f64, Vec<(String, u64)>, String)> = rows[rank]
                .iter()
                .enumerate()
                .map(|(index, id)| {
                    let node = by_id[id.as_str()];
                    let anchors: Vec<f64> = adjacency
                        .neighbours(id)
                        .iter()
                        .filter(|other| in_reference.contains(other.as_str()))
                        .filter_map(|other| places.get(other).copied())
                        .collect();
                    let barycentre = if anchors.is_empty() {
                        index as f64
                    } else {
                        anchors.iter().sum::<f64>() / anchors.len() as f64
                    };
                    (
                        site_rank(node),
                        barycentre,
                        crate::graph_sort_key(&node.name),
                        id.clone(),
                    )
                })
                .collect();
            // Ties are broken by name, so `ACC01` comes before `ACC02` rather
            // than in whatever order their opaque ids happen to sort in.
            keyed.sort_by(|a, b| {
                a.0.cmp(&b.0)
                    .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                    .then(a.2.cmp(&b.2))
                    .then(a.3.cmp(&b.3))
            });
            rows[rank] = keyed.into_iter().map(|(.., id)| id).collect();
        }
    }

    // Rows wider than the drawing can usefully be are wrapped onto several
    // lines, the way a campus diagram draws forty access switches: a single
    // row of them is a strip nobody can read.
    let step = NODE_WIDTH + X_GAP;
    let per_line = ((graph.nodes.len() as f64) * 1.8).sqrt().ceil().max(6.0) as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut top = 0.0;
    for (rank, row) in rows.iter().enumerate() {
        if row.is_empty() {
            top += Y_GAP;
            continue;
        }
        // Wrap at site boundaries where it can: a line that is all one site
        // reads as a block, one that straddles two does not.
        let mut blocks: Vec<Vec<String>> = Vec::new();
        for id in row {
            let site = site_rank(by_id[id.as_str()]);
            match blocks.last_mut() {
                Some(block) if site_rank(by_id[block[0].as_str()]) == site => {
                    block.push(id.clone());
                }
                _ => blocks.push(vec![id.clone()]),
            }
        }
        let mut wrapped: Vec<Vec<String>> = Vec::new();
        let mut current: Vec<String> = Vec::new();
        for block in blocks {
            let parts = block.len().div_ceil(per_line).max(1);
            let each = block.len().div_ceil(parts);
            for chunk in block.chunks(each) {
                if !current.is_empty() && current.len() + chunk.len() > per_line {
                    wrapped.push(std::mem::take(&mut current));
                }
                current.extend_from_slice(chunk);
            }
        }
        if !current.is_empty() {
            wrapped.push(current);
        }
        let count = wrapped.len().max(1);
        for (index, chunk) in wrapped.iter().enumerate() {
            let mut width = 0.0;
            let mut offsets = Vec::with_capacity(chunk.len());
            let mut previous_site: Option<usize> = None;
            for id in chunk {
                let site = site_rank(by_id[id.as_str()]);
                if previous_site.is_some_and(|previous| previous != site) {
                    width += SITE_GAP;
                }
                previous_site = Some(site);
                offsets.push(width);
                width += step;
            }
            lines.push(Line {
                rank,
                y: top + index as f64 * SUB_ROW_GAP,
                width: width - X_GAP,
                nodes: chunk.to_vec(),
                offsets,
            });
        }
        top += Y_GAP + (count - 1) as f64 * SUB_ROW_GAP;
    }

    // Each line is centred, then slid under whatever it is connected to above
    // and below — far enough to sit over its own children, never so far that
    // it leaves the drawing.
    let widest = lines.iter().map(|line| line.width).fold(0.0_f64, f64::max);
    let mut positions: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    // `offsets` are left edges; a node's position is its centre.
    let half = NODE_WIDTH / 2.0;
    for line in &lines {
        let start = (widest - line.width) / 2.0;
        for (id, offset) in line.nodes.iter().zip(&line.offsets) {
            positions.insert(id.clone(), (start + offset + half, line.y));
        }
    }
    for pass in 0..2 {
        let order: Vec<usize> = if pass == 0 {
            (0..lines.len()).collect()
        } else {
            (0..lines.len()).rev().collect()
        };
        for index in order {
            let line = &lines[index];
            let anchors: Vec<f64> = line
                .nodes
                .iter()
                .filter_map(|id| {
                    let near: Vec<f64> = adjacency
                        .neighbours(id)
                        .iter()
                        .filter(|other| ranks.get(other.as_str()).copied() != Some(line.rank))
                        .filter_map(|other| positions.get(other).map(|(x, _)| *x))
                        .collect();
                    (!near.is_empty()).then(|| near.iter().sum::<f64>() / near.len() as f64)
                })
                .collect();
            if anchors.is_empty() {
                continue;
            }
            let middle = anchors.iter().sum::<f64>() / anchors.len() as f64;
            let start = (middle - line.width / 2.0).clamp(0.0, (widest - line.width).max(0.0));
            for (id, offset) in line.nodes.iter().zip(&line.offsets) {
                if let Some(place) = positions.get_mut(id) {
                    place.0 = start + offset + half;
                }
            }
        }
    }

    positions
        .into_iter()
        .map(|(id, (x, y))| {
            (
                id,
                Position {
                    x,
                    y,
                    pinned: false,
                },
            )
        })
        .collect()
}

/// One row of the drawing: a rank, or part of one that was too wide.
struct Line {
    rank: usize,
    y: f64,
    width: f64,
    nodes: Vec<String>,
    offsets: Vec<f64>,
}

/// A spring embedder for meshes. Deterministic: the starting ring is the
/// node order, and nothing is random.
fn force(graph: &Graph, adjacency: &Adjacency) -> Positions {
    let count = graph.nodes.len();
    if count == 0 {
        return Positions::new();
    }
    // Sorted, so the starting ring (and therefore the whole layout) depends on
    // the graph and not on the order its nodes happened to arrive in.
    let ids: Vec<String> = {
        let mut ids: Vec<String> = graph.nodes.iter().map(|node| node.id.clone()).collect();
        ids.sort();
        ids
    };
    let index: BTreeMap<&str, usize> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect();
    let spread = (NODE_WIDTH + X_GAP) * (count as f64).sqrt();
    // A sunflower spiral, not a ring: it fills a disc at an even density, so
    // nothing starts on top of anything else however many nodes there are.
    // The golden angle is what makes the turns never line up.
    const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653;
    let step = NODE_WIDTH + X_GAP;
    let mut x: Vec<f64> = Vec::with_capacity(count);
    let mut y: Vec<f64> = Vec::with_capacity(count);
    for position in 0..count {
        let radius = step * 0.62 * (position as f64).sqrt();
        let angle = GOLDEN_ANGLE * position as f64;
        x.push(radius * angle.cos());
        y.push(radius * angle.sin());
    }
    let edges: Vec<(usize, usize)> = {
        let mut edges: Vec<(usize, usize)> = graph
            .links
            .iter()
            .filter_map(|link| {
                let a = *index.get(link.a.node.as_str())?;
                let b = *index.get(link.b.node.as_str())?;
                (a != b).then_some(if a <= b { (a, b) } else { (b, a) })
            })
            .collect();
        // Summing the pulls in a fixed order keeps the arithmetic identical.
        edges.sort_unstable();
        edges
    };
    let ideal = (spread * spread / count as f64)
        .sqrt()
        .max(NODE_WIDTH + X_GAP);
    // Every pair repels every other pair, so the work per iteration grows with
    // the square of the graph. The number of iterations comes down to match: a
    // spring embedder has settled long before iteration 300 at that size, and
    // the separation pass below guarantees the result is readable either way.
    let iterations = (6_000.0 / (count as f64).max(1.0)).clamp(16.0, 300.0) as usize;
    let mut temperature = spread / 6.0;
    let cooling = temperature / (iterations as f64 + 1.0);
    let mut push_x = vec![0.0; count];
    let mut push_y = vec![0.0; count];
    for _ in 0..iterations {
        push_x.iter_mut().for_each(|value| *value = 0.0);
        push_y.iter_mut().for_each(|value| *value = 0.0);
        for one in 0..count {
            for other in (one + 1)..count {
                let (dx, dy) = (x[one] - x[other], y[one] - y[other]);
                let distance = (dx * dx + dy * dy).sqrt().max(0.01);
                let repulsion = ideal * ideal / distance;
                let (ux, uy) = (dx / distance, dy / distance);
                push_x[one] += ux * repulsion;
                push_y[one] += uy * repulsion;
                push_x[other] -= ux * repulsion;
                push_y[other] -= uy * repulsion;
            }
        }
        for &(one, other) in &edges {
            let (dx, dy) = (x[one] - x[other], y[one] - y[other]);
            let distance = (dx * dx + dy * dy).sqrt().max(0.01);
            let attraction = distance * distance / ideal;
            let (ux, uy) = (dx / distance, dy / distance);
            push_x[one] -= ux * attraction;
            push_y[one] -= uy * attraction;
            push_x[other] += ux * attraction;
            push_y[other] += uy * attraction;
        }
        for node in 0..count {
            let length = (push_x[node] * push_x[node] + push_y[node] * push_y[node])
                .sqrt()
                .max(0.01);
            let step = length.min(temperature);
            x[node] += push_x[node] / length * step;
            y[node] += push_y[node] / length * step;
        }
        temperature = (temperature - cooling).max(0.01);
    }
    let _ = adjacency;
    // The embedder treats nodes as points, but a node is a wide, short card.
    // Stretching the result horizontally gives the labels the room they need
    // without changing the shape of the graph.
    let stretch = (NODE_WIDTH + X_GAP) / (NODE_HEIGHT + X_GAP);
    ids.into_iter()
        .enumerate()
        .map(|(position, id)| {
            (
                id,
                Position {
                    x: x[position] * stretch,
                    y: y[position],
                    pinned: false,
                },
            )
        })
        .collect()
}

/// Rings around a root.
fn radial(graph: &Graph, adjacency: &Adjacency, root: Option<&str>) -> Positions {
    let Some(start) = root
        .map(str::to_string)
        .filter(|id| graph.node(id).is_some())
        .or_else(|| adjacency.busiest())
    else {
        return Positions::new();
    };
    let mut ring: BTreeMap<String, usize> = BTreeMap::from([(start.clone(), 0)]);
    let mut parent: BTreeMap<String, String> = BTreeMap::new();
    let mut queue = VecDeque::from([start.clone()]);
    while let Some(current) = queue.pop_front() {
        let next_ring = ring[&current] + 1;
        for other in adjacency.neighbours(&current) {
            if !ring.contains_key(other) {
                ring.insert(other.clone(), next_ring);
                parent.insert(other.clone(), current.clone());
                queue.push_back(other.clone());
            }
        }
    }
    let outer = ring.values().copied().max().unwrap_or(0) + 1;
    for node in &graph.nodes {
        ring.entry(node.id.clone()).or_insert(outer);
    }

    let mut rings: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (id, index) in &ring {
        rings.entry(*index).or_default().push(id.clone());
    }
    let mut positions = Positions::new();
    let mut angles: BTreeMap<String, f64> = BTreeMap::new();
    for (index, mut members) in rings {
        if index == 0 {
            for id in members {
                angles.insert(id.clone(), 0.0);
                positions.insert(
                    id,
                    Position {
                        x: 0.0,
                        y: 0.0,
                        pinned: false,
                    },
                );
            }
            continue;
        }
        members.sort_by(|a, b| {
            let angle = |id: &String| {
                parent
                    .get(id)
                    .and_then(|parent| angles.get(parent))
                    .copied()
                    .unwrap_or(0.0)
            };
            angle(a)
                .partial_cmp(&angle(b))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| crate::graph_sort_key(a).cmp(&crate::graph_sort_key(b)))
        });
        let count = members.len() as f64;
        let circumference = count * (NODE_WIDTH + X_GAP);
        let radius =
            (index as f64 * (NODE_HEIGHT + Y_GAP)).max(circumference / std::f64::consts::TAU);
        for (place, id) in members.into_iter().enumerate() {
            let angle = std::f64::consts::TAU * place as f64 / count;
            angles.insert(id.clone(), angle);
            positions.insert(
                id,
                Position {
                    x: radius * angle.cos(),
                    y: radius * angle.sin(),
                    pinned: false,
                },
            );
        }
    }
    positions
}

/// Push overlapping nodes apart. Nodes in `fixed` never move.
fn separate(positions: &mut Positions, fixed: &Positions) {
    let min_x = NODE_WIDTH + 12.0;
    let min_y = NODE_HEIGHT + 12.0;
    let cell = min_x.max(min_y);
    // Work on plain arrays: the ids are only needed to read and write back.
    let ids: Vec<&String> = positions.keys().collect();
    let mut xs: Vec<f64> = ids.iter().map(|id| positions[*id].x).collect();
    let mut ys: Vec<f64> = ids.iter().map(|id| positions[*id].y).collect();
    let locked: Vec<bool> = ids.iter().map(|id| fixed.contains_key(*id)).collect();
    let count = ids.len();
    let mut buckets: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    let mut pairs: Vec<(usize, usize)> = Vec::with_capacity(count * 8);
    // Half a stencil of neighbouring cells, so every pair comes up once.
    const STENCIL: [(i64, i64); 5] = [(0, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];

    for _ in 0..400 {
        buckets.clear();
        for node in 0..count {
            buckets
                .entry(((xs[node] / cell) as i64, (ys[node] / cell) as i64))
                .or_default()
                .push(node);
        }
        pairs.clear();
        for ((bucket_x, bucket_y), members) in &buckets {
            for (dx, dy) in STENCIL {
                let Some(others) = buckets.get(&(bucket_x + dx, bucket_y + dy)) else {
                    continue;
                };
                for &one in members {
                    for &other in others {
                        if (dx, dy) != (0, 0) || one < other {
                            pairs.push(if one <= other {
                                (one, other)
                            } else {
                                (other, one)
                            });
                        }
                    }
                }
            }
        }
        pairs.sort_unstable();
        let mut moved = false;
        // Each sweep works from the positions it started with, so the order
        // the pairs come in cannot change the outcome.
        let (was_x, was_y) = (xs.clone(), ys.clone());
        for &(one, other) in &pairs {
            if locked[one] && locked[other] {
                continue;
            }
            let dx = was_x[other] - was_x[one];
            let dy = was_y[other] - was_y[one];
            if dx.abs() >= min_x || dy.abs() >= min_y {
                continue;
            }
            let overlap_x = min_x - dx.abs();
            let overlap_y = min_y - dy.abs();
            let (shift_x, shift_y) = if overlap_x <= overlap_y {
                (if dx >= 0.0 { overlap_x } else { -overlap_x }, 0.0)
            } else {
                (0.0, if dy >= 0.0 { overlap_y } else { -overlap_y })
            };
            let (one_share, other_share) = match (locked[one], locked[other]) {
                (true, false) => (0.0, 1.0),
                (false, true) => (1.0, 0.0),
                _ => (0.5, 0.5),
            };
            xs[one] -= shift_x * one_share;
            ys[one] -= shift_y * one_share;
            xs[other] += shift_x * other_share;
            ys[other] += shift_y * other_share;
            moved = true;
        }
        if !moved {
            break;
        }
    }

    // The relaxation above settles nearly every drawing. A very dense one can
    // still have a few nodes touching when the sweeps run out, and "no two
    // nodes on top of each other" is a promise, so the stragglers are slid to
    // the right until they are clear. Each node only ever moves one way, so
    // this always finishes.
    // The furthest right of everything this node still sits on top of.
    let blocked_until = |node: usize, xs: &[f64], ys: &[f64]| -> Option<f64> {
        (0..count)
            .filter(|other| {
                *other != node
                    && (xs[*other] - xs[node]).abs() < min_x
                    && (ys[*other] - ys[node]).abs() < min_y
            })
            .map(|other| xs[other] + min_x)
            .fold(None, |furthest: Option<f64>, edge| {
                Some(furthest.map_or(edge, |known| known.max(edge)))
            })
    };
    for node in 0..count {
        if locked[node] {
            continue;
        }
        let mut steps = 0;
        while let Some(edge) = blocked_until(node, &xs, &ys) {
            // Never backwards: that is what makes this finish.
            xs[node] = edge.max(xs[node] + 1.0);
            steps += 1;
            if steps > count {
                break;
            }
        }
    }

    let placed: Vec<(String, f64, f64)> = ids
        .into_iter()
        .enumerate()
        .map(|(node, id)| (id.clone(), xs[node], ys[node]))
        .collect();
    for (id, x, y) in placed {
        if let Some(position) = positions.get_mut(&id) {
            position.x = x;
            position.y = y;
        }
    }
}

/// Move the drawing so it starts at the margin, unless something is pinned
/// (then the pinned nodes decide where the drawing is).
fn normalise(positions: &mut Positions, fixed: &Positions) {
    if !fixed.is_empty() || positions.is_empty() {
        return;
    }
    let left = positions
        .values()
        .map(|position| position.x)
        .fold(f64::INFINITY, f64::min);
    let top = positions
        .values()
        .map(|position| position.y)
        .fold(f64::INFINITY, f64::min);
    for position in positions.values_mut() {
        position.x = (position.x - left + MARGIN).round();
        position.y = (position.y - top + MARGIN).round();
    }
}

/// Whether any two nodes sit on top of each other. The layout promises this
/// is always `None`, and the tests hold it to that.
#[cfg(test)]
pub fn overlapping(positions: &Positions) -> Option<(String, String)> {
    let nodes: Vec<(&String, &Position)> = positions.iter().collect();
    for one in 0..nodes.len() {
        for other in (one + 1)..nodes.len() {
            let (dx, dy) = (
                nodes[one].1.x - nodes[other].1.x,
                nodes[one].1.y - nodes[other].1.y,
            );
            if dx.abs() < NODE_WIDTH && dy.abs() < NODE_HEIGHT {
                return Some((nodes[one].0.clone(), nodes[other].0.clone()));
            }
        }
    }
    None
}
