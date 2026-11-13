//! A standalone SVG drawing: something that can go into a change document.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use nocturne_drivers::ifname;

use super::{clip, timestamp, xml_text};
use crate::layout::Y_GAP;
use crate::model::{Confidence, Graph, GroupKind, Layer, Link, LinkKind, Node, NodeKind, Status};

const CARD_WIDTH: f64 = 190.0;
const CARD_HEIGHT: f64 = 52.0;
const PAD: f64 = 48.0;
const HEADER: f64 = 86.0;
/// Interface names are only worth drawing while they can still be read.
const LABEL_LIMIT: usize = 70;

const FONT: &str = "Inter, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif";

/// The accent colour of a node kind.
fn accent(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Router => "#b45309",
        NodeKind::Switch => "#0f766e",
        NodeKind::L3Switch => "#1d4ed8",
        NodeKind::Firewall => "#b91c1c",
        NodeKind::AccessPoint => "#7c3aed",
        NodeKind::Server => "#0369a1",
        NodeKind::Host => "#475569",
        NodeKind::Cloud => "#64748b",
        NodeKind::Unknown => "#94a3b8",
    }
}

fn kind_label(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Router => "Router",
        NodeKind::Switch => "Switch",
        NodeKind::L3Switch => "Layer 3 switch",
        NodeKind::Firewall => "Firewall",
        NodeKind::AccessPoint => "Access point",
        NodeKind::Server => "Server",
        NodeKind::Host => "Host",
        NodeKind::Cloud => "External",
        NodeKind::Unknown => "Unknown",
    }
}

/// A 22×22 glyph with its top-left corner at `(x, y)`.
fn glyph(kind: NodeKind, x: f64, y: f64, colour: &str) -> String {
    let (cx, cy) = (x + 11.0, y + 11.0);
    match kind {
        NodeKind::Router => format!(
            "<circle cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"9.5\" fill=\"none\" stroke=\"{colour}\" stroke-width=\"1.6\"/>\
             <path d=\"M{:.1} {:.1} h9 m-3 -3 l3 3 l-3 3 M{:.1} {:.1} h-9 m3 -3 l-3 3 l3 3\" fill=\"none\" stroke=\"{colour}\" stroke-width=\"1.4\"/>",
            cx - 4.5,
            cy - 3.0,
            cx + 4.5,
            cy + 3.0
        ),
        NodeKind::Switch | NodeKind::L3Switch => {
            let inner = if kind == NodeKind::L3Switch {
                format!(
                    "<circle cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"3.4\" fill=\"none\" stroke=\"{colour}\" stroke-width=\"1.5\"/>"
                )
            } else {
                format!(
                    "<path d=\"M{:.1} {:.1} h8 m-2.5 -2.5 l2.5 2.5 l-2.5 2.5 M{:.1} {:.1} h-8 m2.5 -2.5 l-2.5 2.5 l2.5 2.5\" fill=\"none\" stroke=\"{colour}\" stroke-width=\"1.3\"/>",
                    cx - 4.0,
                    cy - 2.6,
                    cx + 4.0,
                    cy + 2.6
                )
            };
            format!(
                "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"19\" height=\"14\" rx=\"2.5\" fill=\"none\" stroke=\"{colour}\" stroke-width=\"1.6\"/>{inner}",
                cx - 9.5,
                cy - 7.0
            )
        }
        NodeKind::Firewall => format!(
            "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"19\" height=\"15\" rx=\"2\" fill=\"none\" stroke=\"{colour}\" stroke-width=\"1.6\"/>\
             <path d=\"M{:.1} {:.1} h19 M{:.1} {:.1} h19 M{:.1} {:.1} v5 M{:.1} {:.1} v5 M{:.1} {:.1} v5\" stroke=\"{colour}\" stroke-width=\"1.1\" fill=\"none\"/>",
            cx - 9.5,
            cy - 7.5,
            cx - 9.5,
            cy - 2.5,
            cx - 9.5,
            cy + 2.5,
            cx - 3.0,
            cy - 7.5,
            cx + 3.0,
            cy - 2.5,
            cx - 3.0,
            cy + 2.5
        ),
        NodeKind::AccessPoint => format!(
            "<circle cx=\"{cx:.1}\" cy=\"{:.1}\" r=\"2.6\" fill=\"{colour}\"/>\
             <path d=\"M{:.1} {:.1} a7 7 0 0 1 10 0 M{:.1} {:.1} a11 11 0 0 1 16.4 0\" fill=\"none\" stroke=\"{colour}\" stroke-width=\"1.5\"/>",
            cy + 6.0,
            cx - 5.0,
            cy + 2.0,
            cx - 8.2,
            cy - 2.0
        ),
        NodeKind::Server => format!(
            "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"15\" height=\"19\" rx=\"2\" fill=\"none\" stroke=\"{colour}\" stroke-width=\"1.6\"/>\
             <path d=\"M{:.1} {:.1} h9 M{:.1} {:.1} h9 M{:.1} {:.1} h9\" stroke=\"{colour}\" stroke-width=\"1.4\"/>",
            cx - 7.5,
            cy - 9.5,
            cx - 4.5,
            cy - 4.5,
            cx - 4.5,
            cy,
            cx - 4.5,
            cy + 4.5
        ),
        NodeKind::Host => format!(
            "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"19\" height=\"13\" rx=\"1.8\" fill=\"none\" stroke=\"{colour}\" stroke-width=\"1.6\"/>\
             <path d=\"M{:.1} {:.1} h9 M{cx:.1} {:.1} v3\" stroke=\"{colour}\" stroke-width=\"1.5\"/>",
            cx - 9.5,
            cy - 8.0,
            cx - 4.5,
            cy + 8.5,
            cy + 5.0
        ),
        NodeKind::Cloud => format!(
            "<path d=\"M{:.1} {:.1} a4.6 4.6 0 0 1 4.3 -6 a5.6 5.6 0 0 1 10.4 -1.4 a4.3 4.3 0 0 1 1.1 7.4 z\" fill=\"none\" stroke=\"{colour}\" stroke-width=\"1.6\"/>",
            cx - 8.5,
            cy + 5.5
        ),
        NodeKind::Unknown => format!(
            "<circle cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"9.5\" fill=\"none\" stroke=\"{colour}\" stroke-width=\"1.5\" stroke-dasharray=\"3 2\"/>\
             <text x=\"{cx:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-size=\"12\" font-weight=\"600\" fill=\"{colour}\">?</text>",
            cy + 4.2
        ),
    }
}

fn status_colour(status: Status) -> &'static str {
    match status {
        Status::Up => "#16a34a",
        Status::Down => "#dc2626",
        Status::Unknown => "#cbd5e1",
    }
}

/// How a link is drawn: colour, dash pattern and width.
fn link_style(link: &Link) -> (&'static str, &'static str, f64) {
    let colour = match (link.status, link.confidence) {
        (Status::Down, _) => "#dc2626",
        (_, Confidence::Inferred) => "#94a3b8",
        _ => "#64748b",
    };
    let dash = match link.confidence {
        Confidence::Confirmed => "none",
        Confidence::OneSided => "7 4",
        Confidence::Inferred => "2 3",
    };
    let width = if link.aggregate.is_some() { 3.0 } else { 1.6 };
    (colour, dash, width)
}

/// Where a line from the centre of a card towards `(tx, ty)` leaves the card.
fn exit(cx: f64, cy: f64, tx: f64, ty: f64) -> (f64, f64) {
    let (dx, dy) = (tx - cx, ty - cy);
    if dx.abs() < 1e-6 && dy.abs() < 1e-6 {
        return (cx, cy);
    }
    let (half_w, half_h) = (CARD_WIDTH / 2.0 + 2.0, CARD_HEIGHT / 2.0 + 2.0);
    let scale_x = if dx.abs() > 1e-6 {
        half_w / dx.abs()
    } else {
        f64::INFINITY
    };
    let scale_y = if dy.abs() > 1e-6 {
        half_h / dy.abs()
    } else {
        f64::INFINITY
    };
    let scale = scale_x.min(scale_y);
    (cx + dx * scale, cy + dy * scale)
}

fn subtitle(node: &Node) -> String {
    let mut parts: Vec<String> = Vec::new();
    if node.unmanaged && node.device_id.is_none() && node.kind != NodeKind::Host {
        parts.push("not in inventory".to_string());
    }
    if let Some(role) = &node.role {
        parts.push(role.clone());
    }
    if let Some(model) = &node.model {
        parts.push(model.clone());
    } else if let Some(address) = &node.address {
        parts.push(address.clone());
    }
    if node.lab {
        parts.push("lab".to_string());
    }
    if node.stale {
        parts.push("stale".to_string());
    }
    if parts.is_empty() {
        parts.push(kind_label(node.kind).to_lowercase());
    }
    clip(&parts.join(" · "), 27)
}

pub fn render(graph: &Graph) -> String {
    let places: BTreeMap<&str, (f64, f64)> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), (node.x, node.y)))
        .collect();
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for (x, y) in places.values() {
        min_x = min_x.min(x - CARD_WIDTH / 2.0);
        max_x = max_x.max(x + CARD_WIDTH / 2.0);
        min_y = min_y.min(y - CARD_HEIGHT / 2.0);
        max_y = max_y.max(y + CARD_HEIGHT / 2.0);
    }
    if !min_x.is_finite() {
        (min_x, min_y, max_x, max_y) = (0.0, 0.0, 480.0, 120.0);
    }

    let kinds_used: Vec<NodeKind> = {
        let mut kinds: Vec<NodeKind> = graph.nodes.iter().map(|node| node.kind).collect();
        kinds.sort();
        kinds.dedup();
        kinds
    };
    let confidences: Vec<Confidence> = {
        let mut all: Vec<Confidence> = graph.links.iter().map(|link| link.confidence).collect();
        all.sort();
        all.dedup();
        all
    };
    let legend_rows = kinds_used.len().max(confidences.len());
    let legend_height = 34.0 + legend_rows as f64 * 20.0;

    let origin_x = min_x - PAD;
    let origin_y = min_y - PAD - HEADER;
    let width = (max_x - min_x) + PAD * 2.0;
    let height = (max_y - min_y) + PAD * 2.0 + HEADER + legend_height;
    let width = width.max(620.0);

    let mut out = String::with_capacity(4096 + graph.nodes.len() * 512);
    let _ = writeln!(
        out,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\" \
         width=\"{width:.0}\" height=\"{height:.0}\" \
         viewBox=\"{origin_x:.1} {origin_y:.1} {width:.1} {height:.1}\" \
         font-family=\"{FONT}\">"
    );
    let _ = writeln!(
        out,
        "<rect x=\"{origin_x:.1}\" y=\"{origin_y:.1}\" width=\"{width:.1}\" height=\"{height:.1}\" fill=\"#ffffff\"/>"
    );

    // Title.
    let title = match graph.layer {
        Layer::Physical => "Physical topology",
        Layer::L3 => "Layer 3 topology",
    };
    let mut counts = format!(
        "{} device{} · {} link{}",
        graph.stats.devices,
        if graph.stats.devices == 1 { "" } else { "s" },
        graph.stats.links,
        if graph.stats.links == 1 { "" } else { "s" }
    );
    if graph.stats.unmanaged > 0 {
        let _ = writeln!(counts, " · {} not in inventory", graph.stats.unmanaged);
    }
    if graph.stats.endpoints > 0 {
        let _ = writeln!(
            counts,
            " · {} host{}",
            graph.stats.endpoints,
            if graph.stats.endpoints == 1 { "" } else { "s" }
        );
    }
    if let (Some(oldest), Some(newest)) = (graph.data_age.oldest_at, graph.data_age.newest_at) {
        let _ = writeln!(
            counts,
            " · collected {}",
            if oldest == newest {
                timestamp(newest)
            } else {
                format!("{} to {}", timestamp(oldest), timestamp(newest))
            }
        );
    }
    let _ = writeln!(
        out,
        "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"19\" font-weight=\"600\" fill=\"#0f172a\">{}</text>\n\
         <text x=\"{:.1}\" y=\"{:.1}\" font-size=\"11.5\" fill=\"#64748b\">{}</text>",
        min_x - PAD / 2.0,
        origin_y + 34.0,
        xml_text(title),
        min_x - PAD / 2.0,
        origin_y + 54.0,
        xml_text(&counts)
    );

    // Group outlines behind everything.
    // One kind of outline per layer: the cables are grouped by site, the
    // addresses by subnet. Drawing both at once is a thicket.
    let wanted = match graph.layer {
        Layer::Physical => [GroupKind::Site, GroupKind::Lab].as_slice(),
        Layer::L3 => [GroupKind::Subnet].as_slice(),
    };
    let mut boxes: Vec<(String, f64, f64, f64, f64, GroupKind)> = Vec::new();
    for group in graph
        .groups
        .iter()
        .filter(|group| wanted.contains(&group.kind))
    {
        let members: Vec<&(f64, f64)> = group
            .nodes
            .iter()
            .filter_map(|id| places.get(id.as_str()))
            .collect();
        if members.len() < 2 {
            continue;
        }
        let left = members
            .iter()
            .map(|(x, _)| x)
            .fold(f64::INFINITY, |a, b| a.min(*b))
            - CARD_WIDTH / 2.0
            - 18.0;
        let right = members
            .iter()
            .map(|(x, _)| x)
            .fold(f64::NEG_INFINITY, |a, b| a.max(*b))
            + CARD_WIDTH / 2.0
            + 18.0;
        let top = members
            .iter()
            .map(|(_, y)| y)
            .fold(f64::INFINITY, |a, b| a.min(*b))
            - CARD_HEIGHT / 2.0
            - 26.0;
        let bottom = members
            .iter()
            .map(|(_, y)| y)
            .fold(f64::NEG_INFINITY, |a, b| a.max(*b))
            + CARD_HEIGHT / 2.0
            + 18.0;
        boxes.push((
            group.label.clone(),
            left,
            top,
            right - left,
            bottom - top,
            group.kind,
        ));
    }
    // Overlapping outlines make a drawing worse, not better: the smallest
    // ones are kept and anything that would cut across them is dropped.
    boxes.sort_by(|one, other| {
        (one.3 * one.4)
            .partial_cmp(&(other.3 * other.4))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(one.0.cmp(&other.0))
    });
    let boxes_wanted = boxes.len();
    let mut kept: Vec<(String, f64, f64, f64, f64, GroupKind)> = Vec::new();
    for candidate in boxes {
        let clashes = kept.iter().any(|other| {
            candidate.1 < other.1 + other.3
                && other.1 < candidate.1 + candidate.3
                && candidate.2 < other.2 + other.4
                && other.2 < candidate.2 + candidate.4
        });
        if !clashes {
            kept.push(candidate);
        }
    }
    // Sites and labs divide the devices between them, so drawing some of the
    // outlines and not others would say something untrue about the ones left
    // out. Subnets genuinely overlap, so there the ones that fit are drawn.
    if wanted[0] != GroupKind::Subnet && kept.len() != boxes_wanted {
        kept.clear();
    }
    {
        for (label, x, y, box_width, box_height, kind) in &kept {
            let dash = if *kind == GroupKind::Subnet {
                "6 4"
            } else {
                "none"
            };
            let _ = writeln!(
                out,
                "<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{box_width:.1}\" height=\"{box_height:.1}\" rx=\"12\" \
                 fill=\"#f8fafc\" stroke=\"#cbd5e1\" stroke-width=\"1\" stroke-dasharray=\"{dash}\"/>\n\
                 <text x=\"{:.1}\" y=\"{:.1}\" font-size=\"11\" font-weight=\"600\" fill=\"#64748b\">{}</text>",
                x + 12.0,
                y + 16.0,
                xml_text(&clip(label, 42))
            );
        }
    }

    // Links.
    let show_labels = graph.links.len() <= LABEL_LIMIT;
    // A node with a dozen links would have a dozen labels stacked on the same
    // few pixels, which is worse than no labels at all.
    let mut degree: BTreeMap<&str, usize> = BTreeMap::new();
    for link in &graph.links {
        *degree.entry(link.a.node.as_str()).or_default() += 1;
        *degree.entry(link.b.node.as_str()).or_default() += 1;
    }
    let roomy = |node: &str| degree.get(node).copied().unwrap_or(0) <= 6;
    let mut link_labels = String::new();
    for link in &graph.links {
        let (Some(&(ax, ay)), Some(&(bx, by))) = (
            places.get(link.a.node.as_str()),
            places.get(link.b.node.as_str()),
        ) else {
            continue;
        };
        let (colour, dash, stroke) = link_style(link);
        if link.a.node == link.b.node {
            // A device that is its own neighbour: a stack cable, drawn as a loop.
            let _ = writeln!(
                out,
                "<path d=\"M{:.1} {:.1} C{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}\" fill=\"none\" \
                 stroke=\"{colour}\" stroke-width=\"{stroke}\" stroke-dasharray=\"{dash}\"/>",
                ax - 16.0,
                ay - CARD_HEIGHT / 2.0,
                ax - 34.0,
                ay - CARD_HEIGHT / 2.0 - 44.0,
                ax + 34.0,
                ay - CARD_HEIGHT / 2.0 - 44.0,
                ax + 16.0,
                ay - CARD_HEIGHT / 2.0
            );
            continue;
        }
        let (sx, sy) = exit(ax, ay, bx, by);
        let (tx, ty) = exit(bx, by, ax, ay);
        // A link that skips a row would pass straight through the cards in
        // between and look as though it ended at one of them, so it is bowed
        // out to one side where a reader can follow it.
        let length = ((tx - sx).powi(2) + (ty - sy).powi(2)).sqrt().max(1.0);
        let bowed = (ty - sy).abs() > Y_GAP * 1.4;
        if bowed {
            // A quadratic curve only reaches half way to its control point,
            // so the control offset is twice the clearance wanted — enough to
            // get round a card that is directly in the way.
            let bow = 2.0 * (length * 0.10).clamp(CARD_WIDTH * 0.62, 190.0);
            let (mut nx, mut ny) = (-(ty - sy) / length, (tx - sx) / length);
            // Bend away from whatever is closest to the middle of the link, so
            // the curve goes round the cards in between rather than through
            // one of them.
            let (mx, my) = ((sx + tx) / 2.0, (sy + ty) / 2.0);
            let nearest = graph
                .nodes
                .iter()
                .filter(|node| node.id != link.a.node && node.id != link.b.node)
                .map(|node| (node.x - mx, node.y - my))
                .min_by(|one, other| {
                    (one.0 * one.0 + one.1 * one.1)
                        .partial_cmp(&(other.0 * other.0 + other.1 * other.1))
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            if let Some((dx, dy)) = nearest
                && nx * dx + ny * dy > 0.0
            {
                nx = -nx;
                ny = -ny;
            }
            let _ = writeln!(
                out,
                "<path d=\"M{sx:.1} {sy:.1} Q{:.1} {:.1} {tx:.1} {ty:.1}\" fill=\"none\" \
                 stroke=\"{colour}\" stroke-width=\"{stroke}\" stroke-dasharray=\"{dash}\" \
                 stroke-linecap=\"round\"/>",
                (sx + tx) / 2.0 + nx * bow,
                (sy + ty) / 2.0 + ny * bow
            );
        } else {
            let _ = writeln!(
                out,
                "<line x1=\"{sx:.1}\" y1=\"{sy:.1}\" x2=\"{tx:.1}\" y2=\"{ty:.1}\" \
                 stroke=\"{colour}\" stroke-width=\"{stroke}\" stroke-dasharray=\"{dash}\" stroke-linecap=\"round\"/>"
            );
        }
        // A label is placed along the straight line between the ends, which is
        // not where a bowed link goes; and two labels on a link shorter than
        // they are would sit on top of each other.
        if !show_labels || bowed || length < 130.0 {
            continue;
        }
        for (end, px, py, qx, qy) in [(&link.a, sx, sy, tx, ty), (&link.b, tx, ty, sx, sy)] {
            let Some(name) = &end.interface.as_ref().filter(|_| roomy(&end.node)) else {
                continue;
            };
            let short = ifname::short(name);
            let (lx, ly) = (px + (qx - px) * 0.18, py + (qy - py) * 0.18);
            let _ = writeln!(
                link_labels,
                "<text x=\"{lx:.1}\" y=\"{ly:.1}\" font-size=\"9.5\" fill=\"#475569\" \
                 text-anchor=\"middle\" paint-order=\"stroke\" stroke=\"#ffffff\" stroke-width=\"3\" \
                 stroke-linejoin=\"round\">{}</text>",
                xml_text(&clip(&short, 14))
            );
        }
        if let Some(aggregate) = &link.aggregate {
            let _ = writeln!(
                link_labels,
                "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"9.5\" font-weight=\"600\" fill=\"#1d4ed8\" \
                 text-anchor=\"middle\" paint-order=\"stroke\" stroke=\"#ffffff\" stroke-width=\"3\" \
                 stroke-linejoin=\"round\">{}</text>",
                (sx + tx) / 2.0,
                (sy + ty) / 2.0 - 4.0,
                xml_text(&clip(aggregate, 24))
            );
        } else if let Some(label) = &link
            .label
            .clone()
            .filter(|_| link.kind == LinkKind::Subnet && length > 150.0)
        {
            let _ = writeln!(
                link_labels,
                "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"9.5\" fill=\"#475569\" \
                 text-anchor=\"middle\" paint-order=\"stroke\" stroke=\"#ffffff\" stroke-width=\"3\" \
                 stroke-linejoin=\"round\">{}</text>",
                (sx + tx) / 2.0,
                (sy + ty) / 2.0 - 4.0,
                xml_text(&clip(label, 20))
            );
        }
    }
    out.push_str(&link_labels);

    // Nodes.
    for node in &graph.nodes {
        let (x, y) = (node.x, node.y);
        let (left, top) = (x - CARD_WIDTH / 2.0, y - CARD_HEIGHT / 2.0);
        let colour = accent(node.kind);
        let border = if node.unmanaged { "6 4" } else { "none" };
        let _ = writeln!(
            out,
            "<g>\n<rect x=\"{left:.1}\" y=\"{top:.1}\" width=\"{CARD_WIDTH}\" height=\"{CARD_HEIGHT}\" rx=\"8\" \
             fill=\"#ffffff\" stroke=\"{}\" stroke-width=\"1.3\" stroke-dasharray=\"{border}\"/>",
            if node.unmanaged { "#94a3b8" } else { "#cbd5e1" }
        );
        let _ = writeln!(
            out,
            "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"4\" height=\"{:.1}\" rx=\"2\" fill=\"{colour}\"/>",
            left + 6.0,
            top + 9.0,
            CARD_HEIGHT - 18.0
        );
        out.push_str(&glyph(node.kind, left + 16.0, y - 11.0, colour));
        let _ = writeln!(
            out,
            "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"12.5\" font-weight=\"600\" fill=\"#0f172a\">{}</text>\n\
             <text x=\"{:.1}\" y=\"{:.1}\" font-size=\"9.5\" fill=\"#64748b\">{}</text>",
            left + 44.0,
            y - 2.0,
            xml_text(&clip(&node.name, 20)),
            left + 44.0,
            y + 12.0,
            xml_text(&subtitle(node))
        );
        let _ = writeln!(
            out,
            "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"3.4\" fill=\"{}\"/>\n</g>",
            left + CARD_WIDTH - 10.0,
            top + 10.0,
            status_colour(node.status)
        );
    }

    // Legend.
    let legend_x = min_x - PAD / 2.0;
    let legend_y = max_y + PAD / 2.0 + 8.0;
    let _ = writeln!(
        out,
        "<rect x=\"{legend_x:.1}\" y=\"{legend_y:.1}\" width=\"{:.1}\" height=\"{legend_height:.1}\" rx=\"8\" \
         fill=\"#f8fafc\" stroke=\"#e2e8f0\"/>\n\
         <text x=\"{:.1}\" y=\"{:.1}\" font-size=\"10.5\" font-weight=\"700\" fill=\"#334155\" letter-spacing=\"0.6\">LEGEND</text>",
        (width - PAD).max(380.0),
        legend_x + 14.0,
        legend_y + 20.0
    );
    for (row, kind) in kinds_used.iter().enumerate() {
        let row_y = legend_y + 36.0 + row as f64 * 20.0;
        out.push_str(&glyph(*kind, legend_x + 14.0, row_y - 11.0, accent(*kind)));
        let _ = writeln!(
            out,
            "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"10.5\" fill=\"#334155\">{}</text>",
            legend_x + 42.0,
            row_y + 3.5,
            xml_text(kind_label(*kind))
        );
    }
    for (row, confidence) in confidences.iter().enumerate() {
        let row_y = legend_y + 36.0 + row as f64 * 20.0;
        let (dash, text) = match confidence {
            Confidence::Confirmed => ("none", "confirmed — both ends report the link"),
            Confidence::OneSided => ("7 4", "one-sided — only one end reports it"),
            Confidence::Inferred => ("2 3", "inferred — derived from addresses or MAC tables"),
        };
        let _ = writeln!(
            out,
            "<line x1=\"{:.1}\" y1=\"{row_y:.1}\" x2=\"{:.1}\" y2=\"{row_y:.1}\" stroke=\"#64748b\" \
             stroke-width=\"1.8\" stroke-dasharray=\"{dash}\"/>\n\
             <text x=\"{:.1}\" y=\"{:.1}\" font-size=\"10.5\" fill=\"#334155\">{}</text>",
            legend_x + 196.0,
            legend_x + 236.0,
            legend_x + 246.0,
            row_y + 3.5,
            xml_text(text)
        );
    }
    out.push_str("</svg>\n");
    out
}
