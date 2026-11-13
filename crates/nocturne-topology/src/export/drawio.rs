//! draw.io / diagrams.net XML: editable shapes, not a picture of them.

use std::fmt::Write as _;

use nocturne_drivers::ifname;

use super::{clip, node_ids, xml_attribute};
use crate::layout::{NODE_HEIGHT, NODE_WIDTH};
use crate::model::{Confidence, Graph, GroupKind, Layer, Node, NodeKind, Status};

fn shape(kind: NodeKind) -> &'static str {
    // mxgraph's own network stencils, so the shapes stay editable in the app.
    match kind {
        NodeKind::Router => "shape=mxgraph.networks.router;",
        NodeKind::Switch => "shape=mxgraph.networks.switch;",
        NodeKind::L3Switch => "shape=mxgraph.networks.switch;",
        NodeKind::Firewall => "shape=mxgraph.networks.firewall;",
        NodeKind::AccessPoint => "shape=mxgraph.networks.wireless_hub;",
        NodeKind::Server => "shape=mxgraph.networks.server;",
        NodeKind::Host => "shape=mxgraph.networks.pc;",
        NodeKind::Cloud => "shape=mxgraph.networks.cloud;",
        NodeKind::Unknown => "",
    }
}

fn fill(node: &Node) -> &'static str {
    if node.unmanaged {
        return "fillColor=#f5f5f5;strokeColor=#999999;dashed=1;";
    }
    match node.status {
        Status::Down => "fillColor=#f8cecc;strokeColor=#b85450;",
        Status::Up => "fillColor=#d5e8d4;strokeColor=#82b366;",
        Status::Unknown => "fillColor=#ffffff;strokeColor=#9eb6c9;",
    }
}

pub fn render(graph: &Graph) -> String {
    let ids = node_ids(graph);
    let mut out = String::with_capacity(2048 + graph.nodes.len() * 320);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        out,
        "<mxfile host=\"nocturne\" type=\"device\">\n  <diagram id=\"topology\" name=\"{}\">",
        xml_attribute(&clip(&format!("{} topology", graph.layer.as_str()), 40))
    );
    out.push_str(
        "    <mxGraphModel dx=\"1400\" dy=\"900\" grid=\"1\" gridSize=\"10\" guides=\"1\" \
         tooltips=\"1\" connect=\"1\" arrows=\"0\" fold=\"1\" page=\"1\" pageScale=\"1\" \
         pageWidth=\"1654\" pageHeight=\"1169\" math=\"0\" shadow=\"0\">\n      <root>\n        \
         <mxCell id=\"0\"/>\n        <mxCell id=\"1\" parent=\"0\"/>\n",
    );

    // One kind of container per layer, and never two that overlap: draw.io
    // nests whatever a shape sits inside, and crossed containers make a mess
    // of that.
    let wanted: &[GroupKind] = match graph.layer {
        Layer::Physical => &[GroupKind::Site, GroupKind::Lab],
        Layer::L3 => &[GroupKind::Subnet],
    };
    let candidates: Vec<&crate::model::Group> = graph
        .groups
        .iter()
        .filter(|group| {
            wanted.contains(&group.kind)
                && group
                    .nodes
                    .iter()
                    .filter(|id| graph.node(id).is_some())
                    .count()
                    >= 2
        })
        .collect();
    let mut drawn: Vec<(f64, f64, f64, f64)> = Vec::new();
    let mut skipped = false;
    let mut outlines: Vec<(&crate::model::Group, f64, f64, f64, f64)> = Vec::new();
    for group in candidates {
        let members: Vec<&Node> = group.nodes.iter().filter_map(|id| graph.node(id)).collect();
        let left = members
            .iter()
            .map(|node| node.x - NODE_WIDTH / 2.0)
            .fold(f64::INFINITY, f64::min)
            - 20.0;
        let top = members
            .iter()
            .map(|node| node.y - NODE_HEIGHT / 2.0)
            .fold(f64::INFINITY, f64::min)
            - 32.0;
        let right = members
            .iter()
            .map(|node| node.x + NODE_WIDTH / 2.0)
            .fold(f64::NEG_INFINITY, f64::max)
            + 20.0;
        let bottom = members
            .iter()
            .map(|node| node.y + NODE_HEIGHT / 2.0)
            .fold(f64::NEG_INFINITY, f64::max)
            + 20.0;
        if drawn.iter().any(|other| {
            left < other.0 + other.2
                && other.0 < right
                && top < other.1 + other.3
                && other.1 < bottom
        }) {
            skipped = true;
            continue;
        }
        drawn.push((left, top, right - left, bottom - top));
        outlines.push((group, left, top, right, bottom));
    }
    // Sites and labs divide the devices between them, so drawing some of the
    // containers and not others would say something untrue about the rest.
    if skipped && wanted[0] != GroupKind::Subnet {
        outlines.clear();
    }
    for (group, left, top, right, bottom) in outlines {
        let dashed = if group.kind == GroupKind::Subnet {
            1
        } else {
            0
        };
        let _ = writeln!(
            out,
            "        <mxCell id=\"g{}\" value=\"{}\" style=\"rounded=1;whiteSpace=wrap;html=0;\
             fillColor=none;strokeColor=#b1b1b1;dashed={dashed};verticalAlign=top;\" vertex=\"1\" parent=\"1\">\n          \
             <mxGeometry x=\"{left:.0}\" y=\"{top:.0}\" width=\"{:.0}\" height=\"{:.0}\" as=\"geometry\"/>\n        </mxCell>",
            xml_attribute(&group.id),
            xml_attribute(&clip(&group.label, 48)),
            right - left,
            bottom - top
        );
    }

    for node in &graph.nodes {
        let id = &ids[&node.id];
        let mut label = clip(&node.name, 28);
        if let Some(address) = &node.address {
            label.push('\n');
            label.push_str(&clip(address, 24));
        }
        let _ = writeln!(
            out,
            "        <mxCell id=\"{id}\" value=\"{}\" style=\"{}{}verticalLabelPosition=bottom;\
             verticalAlign=top;html=0;whiteSpace=wrap;rounded=1;\" vertex=\"1\" parent=\"1\">\n          \
             <mxGeometry x=\"{:.0}\" y=\"{:.0}\" width=\"{:.0}\" height=\"{:.0}\" as=\"geometry\"/>\n        </mxCell>",
            xml_attribute(&label),
            shape(node.kind),
            fill(node),
            node.x - NODE_WIDTH / 2.0,
            node.y - NODE_HEIGHT / 2.0,
            NODE_WIDTH,
            NODE_HEIGHT
        );
    }

    for (index, link) in graph.links.iter().enumerate() {
        let (Some(source), Some(target)) = (ids.get(&link.a.node), ids.get(&link.b.node)) else {
            continue;
        };
        let dashed = match link.confidence {
            Confidence::Confirmed => "dashed=0;",
            Confidence::OneSided => "dashed=1;dashPattern=8 4;",
            Confidence::Inferred => "dashed=1;dashPattern=1 3;",
        };
        let colour = match link.status {
            Status::Down => "strokeColor=#b85450;",
            _ => "strokeColor=#6c8ebf;",
        };
        let width = if link.aggregate.is_some() {
            "strokeWidth=3;"
        } else {
            "strokeWidth=1;"
        };
        let label = link
            .aggregate
            .clone()
            .or_else(|| link.label.clone())
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "        <mxCell id=\"e{index}\" value=\"{}\" style=\"endArrow=none;html=0;{dashed}{colour}{width}\" \
             edge=\"1\" parent=\"1\" source=\"{source}\" target=\"{target}\">\n          \
             <mxGeometry relative=\"1\" as=\"geometry\"/>\n        </mxCell>",
            xml_attribute(&clip(&label, 32))
        );
        // Edge labels are cells of their own, parented to the edge.
        for (end, position, suffix) in [(&link.a, "-1", "a"), (&link.b, "1", "b")] {
            let Some(interface) = &end.interface else {
                continue;
            };
            let _ = writeln!(
                out,
                "        <mxCell id=\"e{index}{suffix}\" value=\"{}\" style=\"edgeLabel;html=0;\
                 align=center;verticalAlign=middle;fontSize=9;\" vertex=\"1\" connectable=\"0\" parent=\"e{index}\">\n          \
                 <mxGeometry x=\"{position}\" relative=\"1\" as=\"geometry\"><mxPoint as=\"offset\"/></mxGeometry>\n        </mxCell>",
                xml_attribute(&clip(&ifname::short(interface), 16))
            );
        }
    }

    out.push_str("      </root>\n    </mxGraphModel>\n  </diagram>\n</mxfile>\n");
    out
}
