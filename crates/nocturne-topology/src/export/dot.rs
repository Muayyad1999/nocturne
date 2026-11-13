//! Graphviz DOT, for anyone who would rather lay it out themselves.

use std::fmt::Write as _;

use nocturne_drivers::ifname;

use super::{clip, node_ids};
use crate::model::{Confidence, Graph, NodeKind, Status};

/// A DOT double-quoted string. Backslash and quote are the only characters
/// that can end one, and a real newline inside a string is not allowed.
fn quoted(text: &str) -> String {
    quoted_lines(&[text])
}

/// The same, with each part on its own line of the label.
fn quoted_lines(parts: &[&str]) -> String {
    let text = parts
        .iter()
        .map(|part| clip(part, 64))
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = String::with_capacity(text.len() + 8);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            // Graphviz reads a two-character `\n` inside a string as a line
            // break; a real newline there is a syntax error.
            '\n' => out.push_str("\\n"),
            '\r' | '\t' => out.push(' '),
            character if (character as u32) < 0x20 => out.push(' '),
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

fn shape(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Router | NodeKind::L3Switch => "ellipse",
        NodeKind::Firewall => "hexagon",
        NodeKind::Cloud => "ellipse",
        NodeKind::Host | NodeKind::Server => "note",
        NodeKind::AccessPoint => "octagon",
        _ => "box",
    }
}

pub fn render(graph: &Graph) -> String {
    let ids = node_ids(graph);
    let mut out = String::with_capacity(512 + graph.nodes.len() * 128);
    out.push_str("graph topology {\n");
    out.push_str("  graph [rankdir=TB, splines=true, overlap=false, fontname=\"Helvetica\"];\n");
    out.push_str(
        "  node [fontname=\"Helvetica\", fontsize=10, style=filled, fillcolor=\"#ffffff\"];\n",
    );
    out.push_str("  edge [fontname=\"Helvetica\", fontsize=8, color=\"#64748b\"];\n");
    for node in &graph.nodes {
        let label = match &node.address {
            Some(address) => quoted_lines(&[&node.name, address]),
            None => quoted_lines(&[&node.name]),
        };
        let colour = match node.status {
            Status::Down => "#fee2e2",
            _ if node.unmanaged => "#f8fafc",
            _ => "#ffffff",
        };
        let style = if node.unmanaged {
            "filled,dashed"
        } else {
            "filled"
        };
        let _ = writeln!(
            out,
            "  {} [label={}, shape={}, fillcolor=\"{colour}\", style=\"{style}\"];",
            ids[&node.id],
            label,
            shape(node.kind)
        );
    }
    for link in &graph.links {
        let (Some(a), Some(b)) = (ids.get(&link.a.node), ids.get(&link.b.node)) else {
            continue;
        };
        let style = match link.confidence {
            Confidence::Confirmed => "solid",
            Confidence::OneSided => "dashed",
            Confidence::Inferred => "dotted",
        };
        let colour = if link.status == Status::Down {
            "#dc2626"
        } else {
            "#64748b"
        };
        let mut attributes = format!("style={style}, color=\"{colour}\"");
        if let Some(aggregate) = &link.aggregate {
            let _ = write!(attributes, ", label={}, penwidth=2.5", quoted(aggregate));
        }
        if let Some(interface) = &link.a.interface {
            let _ = write!(
                attributes,
                ", taillabel={}",
                quoted(&ifname::short(interface))
            );
        }
        if let Some(interface) = &link.b.interface {
            let _ = write!(
                attributes,
                ", headlabel={}",
                quoted(&ifname::short(interface))
            );
        }
        let _ = writeln!(out, "  {a} -- {b} [{attributes}];");
    }
    out.push_str("}\n");
    out
}
