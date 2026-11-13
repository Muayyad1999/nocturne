//! Mermaid `graph` source, for a wiki or a pull request.

use std::fmt::Write as _;

use nocturne_drivers::ifname;

use super::{clip, node_ids};
use crate::model::{Confidence, Graph, NodeKind};

/// Mermaid renders labels as HTML, so a label is only safe once every
/// character that means something — to Mermaid or to the browser — is an
/// entity. Node ids are generated, never taken from device data.
fn label(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for character in clip(text, 40).chars() {
        match character {
            '"' => out.push_str("#quot;"),
            '#' => out.push_str("#35;"),
            '<' => out.push_str("#lt;"),
            '>' => out.push_str("#gt;"),
            '&' => out.push_str("#amp;"),
            '\'' => out.push_str("#39;"),
            '(' => out.push_str("#40;"),
            ')' => out.push_str("#41;"),
            '[' => out.push_str("#91;"),
            ']' => out.push_str("#93;"),
            '{' => out.push_str("#123;"),
            '}' => out.push_str("#125;"),
            ';' => out.push_str("#59;"),
            '|' => out.push_str("#124;"),
            '\\' => out.push_str("#92;"),
            '`' => out.push_str("#96;"),
            character if (character as u32) < 0x20 => out.push(' '),
            character => out.push(character),
        }
    }
    out
}

/// The bracket pair that gives a node its shape.
fn shape(kind: NodeKind) -> (&'static str, &'static str) {
    match kind {
        NodeKind::Router | NodeKind::L3Switch => ("((", "))"),
        NodeKind::Firewall => ("{{", "}}"),
        NodeKind::Cloud => ("([", "])"),
        NodeKind::Host | NodeKind::Server => ("[/", "/]"),
        _ => ("[", "]"),
    }
}

pub fn render(graph: &Graph) -> String {
    let ids = node_ids(graph);
    let mut out = String::with_capacity(512 + graph.nodes.len() * 96);
    out.push_str("graph TD\n");
    for node in &graph.nodes {
        let (open, close) = shape(node.kind);
        let _ = writeln!(
            out,
            "  {}{open}\"{}\"{close}",
            ids[&node.id],
            label(&node.name)
        );
    }
    let mut dashed: Vec<(usize, &'static str)> = Vec::new();
    let mut edge = 0;
    for link in &graph.links {
        let (Some(a), Some(b)) = (ids.get(&link.a.node), ids.get(&link.b.node)) else {
            continue;
        };
        let mut text = Vec::new();
        if let Some(aggregate) = &link.aggregate {
            text.push(aggregate.clone());
        } else {
            if let Some(interface) = &link.a.interface {
                text.push(ifname::short(interface));
            }
            if let Some(interface) = &link.b.interface {
                text.push(ifname::short(interface));
            }
        }
        // One link form only (`---`, optionally with a label); the confidence
        // is a `linkStyle`, which cannot be confused with the graph itself.
        if text.is_empty() {
            let _ = writeln!(out, "  {a} --- {b}");
        } else {
            let _ = writeln!(out, "  {a} ---|\"{}\"| {b}", label(&text.join(" <-> ")));
        }
        match link.confidence {
            Confidence::Confirmed => {}
            Confidence::OneSided => dashed.push((edge, "8 4")),
            Confidence::Inferred => dashed.push((edge, "2 3")),
        }
        edge += 1;
    }
    for (index, pattern) in dashed {
        let _ = writeln!(out, "  linkStyle {index} stroke-dasharray:{pattern};");
    }

    // Styling by status, after the graph so the source still reads as a list.
    let down: Vec<&str> = graph
        .nodes
        .iter()
        .filter(|node| node.status == crate::model::Status::Down)
        .map(|node| ids[&node.id].as_str())
        .collect();
    if !down.is_empty() {
        let _ = writeln!(
            out,
            "  classDef down fill:#fee2e2,stroke:#b91c1c;\n  class {} down;",
            down.join(",")
        );
    }
    let unmanaged: Vec<&str> = graph
        .nodes
        .iter()
        .filter(|node| node.unmanaged)
        .map(|node| ids[&node.id].as_str())
        .collect();
    if !unmanaged.is_empty() {
        let _ = writeln!(
            out,
            "  classDef unmanaged fill:#f8fafc,stroke:#94a3b8,stroke-dasharray:4 3;\n  class {} unmanaged;",
            unmanaged.join(",")
        );
    }
    out
}
