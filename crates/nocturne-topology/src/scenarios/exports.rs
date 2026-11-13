//! Exports have to parse, carry every node, and be inert: a device name is
//! whatever a box on the network chose to send.

use nocturne_drivers::model::Kind;

use super::*;
use crate::export::{self, Format};
use crate::graph::{self, Options};
use crate::model::{Graph, Layer};

/// Text that tries to break out of every format at once.
const HOSTILE: &str = "<script>alert(1)</script> \"q\" 'a' & ]]> --> ${x} |pipe| `tick`";

// ----------------------------------------------------------------- XML check

/// `&` is only allowed as one of these.
fn is_entity(rest: &[char]) -> bool {
    for entity in ["&amp;", "&lt;", "&gt;", "&quot;", "&apos;"] {
        if rest.len() >= entity.len() && rest[..entity.len()].iter().collect::<String>() == *entity
        {
            return true;
        }
    }
    if rest.len() > 3 && rest[1] == '#' {
        let digits: Vec<char> = rest[2..]
            .iter()
            .copied()
            .take_while(|c| *c != ';')
            .collect();
        return !digits.is_empty()
            && digits
                .iter()
                .all(|c| c.is_ascii_hexdigit() || *c == 'x' || *c == 'X');
    }
    false
}

/// No markup may reach character data or an attribute value.
fn check_data(data: &[char], what: &str) -> Result<(), String> {
    for (index, character) in data.iter().enumerate() {
        match character {
            '<' => return Err(format!("raw `<` in {what} at {index}")),
            '&' if !is_entity(&data[index..]) => {
                return Err(format!("unescaped `&` in {what} at {index}"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Find `needle` in `haystack` starting at `from`, in characters.
fn find_from(haystack: &[char], from: usize, needle: &str) -> Option<usize> {
    let needle: Vec<char> = needle.chars().collect();
    (from..haystack.len().saturating_sub(needle.len() - 1))
        .find(|start| haystack[*start..*start + needle.len()] == needle[..])
}

/// A deliberately strict XML well-formedness check: tags balance, attribute
/// values are double-quoted and hold no markup, and character data holds no
/// `<` and no `&` that is not a known entity. Returns the text nodes.
fn parse_xml(document: &str) -> Result<Vec<String>, String> {
    let chars: Vec<char> = document.chars().collect();
    let mut at = 0;
    let mut stack: Vec<String> = Vec::new();
    let mut text: Vec<String> = Vec::new();
    let mut root_seen = false;

    while at < chars.len() {
        if chars[at] != '<' {
            let start = at;
            while at < chars.len() && chars[at] != '<' {
                at += 1;
            }
            check_data(&chars[start..at], "character data")?;
            let data: String = chars[start..at].iter().collect();
            if !data.trim().is_empty() {
                text.push(data.trim().to_string());
            }
            continue;
        }
        let next = chars.get(at + 1).copied();
        if next == Some('?') {
            at = find_from(&chars, at, "?>").ok_or("unterminated processing instruction")? + 2;
            continue;
        }
        if next == Some('!') {
            at = find_from(&chars, at, "-->").ok_or("unterminated comment")? + 3;
            continue;
        }
        at += 1;
        let closing = chars.get(at) == Some(&'/');
        if closing {
            at += 1;
        }
        let name_start = at;
        while at < chars.len() && !chars[at].is_whitespace() && chars[at] != '>' && chars[at] != '/'
        {
            at += 1;
        }
        let name: String = chars[name_start..at].iter().collect();
        if name.is_empty() {
            return Err("a tag with no name".to_string());
        }
        let mut self_closing = false;
        loop {
            while at < chars.len() && chars[at].is_whitespace() {
                at += 1;
            }
            match chars.get(at) {
                None => return Err(format!("unterminated tag `{name}`")),
                Some('>') => {
                    at += 1;
                    break;
                }
                Some('/') => {
                    self_closing = true;
                    at += 1;
                }
                Some(_) => {
                    let start = at;
                    while at < chars.len() && chars[at] != '=' && !chars[at].is_whitespace() {
                        at += 1;
                    }
                    let attribute: String = chars[start..at].iter().collect();
                    while at < chars.len() && chars[at].is_whitespace() {
                        at += 1;
                    }
                    if chars.get(at) != Some(&'=') {
                        return Err(format!("attribute `{attribute}` of `{name}` has no value"));
                    }
                    at += 1;
                    while at < chars.len() && chars[at].is_whitespace() {
                        at += 1;
                    }
                    if chars.get(at) != Some(&'"') {
                        return Err(format!(
                            "attribute `{attribute}` of `{name}` is not double-quoted"
                        ));
                    }
                    at += 1;
                    let value_start = at;
                    while at < chars.len() && chars[at] != '"' {
                        at += 1;
                    }
                    if at >= chars.len() {
                        return Err(format!("unterminated attribute value in `{name}`"));
                    }
                    check_data(
                        &chars[value_start..at],
                        &format!("attribute `{attribute}` of `{name}`"),
                    )?;
                    at += 1;
                }
            }
        }
        if closing {
            match stack.pop() {
                Some(open) if open == name => {}
                Some(open) => return Err(format!("`</{name}>` closes `<{open}>`")),
                None => return Err(format!("`</{name}>` closes nothing")),
            }
        } else {
            if stack.is_empty() {
                if root_seen {
                    return Err("two root elements".to_string());
                }
                root_seen = true;
            }
            if !self_closing {
                stack.push(name);
            }
        }
    }
    if let Some(open) = stack.last() {
        return Err(format!("`<{open}>` is never closed"));
    }
    if !root_seen {
        return Err("no elements at all".to_string());
    }
    Ok(text)
}

// --------------------------------------------------------- Mermaid and DOT

/// Mermaid by a simple grammar: a header, then node, edge and style
/// statements, with every label quoted and free of markup.
fn check_mermaid(source: &str) -> Result<usize, String> {
    let mut lines = source.lines();
    if lines.next() != Some("graph TD") {
        return Err("the first line is not `graph TD`".to_string());
    }
    let mut nodes = 0;
    for (number, line) in lines.enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let inert = |text: &str| -> Result<(), String> {
            if text.contains('<') || text.contains('>') || text.contains('\\') {
                return Err(format!("line {number} has markup: {text}"));
            }
            if !text.matches('"').count().is_multiple_of(2) {
                return Err(format!("line {number} has an unbalanced label: {text}"));
            }
            Ok(())
        };
        inert(line)?;
        if line.starts_with("classDef ") || line.starts_with("class ") {
            continue;
        }
        if line.starts_with("linkStyle ") {
            continue;
        }
        if line.contains("---") {
            if line.split("---").count() != 2 {
                return Err(format!("line {number} is not one edge: {line}"));
            }
            continue;
        }
        let identifier: String = line.chars().take_while(|c| c.is_alphanumeric()).collect();
        if !identifier.starts_with('n') || identifier.len() < 2 {
            return Err(format!("line {number} is not a statement: {line}"));
        }
        if line.matches('"').count() != 2 {
            return Err(format!("line {number} has an unquoted label: {line}"));
        }
        nodes += 1;
    }
    Ok(nodes)
}

/// DOT by a simple grammar: one `graph name { … }` of `;`-terminated
/// statements, with every double-quoted string properly closed.
fn check_dot(source: &str) -> Result<usize, String> {
    let mut lines = source.lines();
    let first = lines.next().unwrap_or_default();
    if !first.starts_with("graph ") || !first.ends_with('{') {
        return Err(format!("the first line is not a graph header: {first}"));
    }
    let mut statements = 0;
    let mut closed = false;
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "}" {
            closed = true;
            continue;
        }
        if closed {
            return Err(format!("`{line}` comes after the closing brace"));
        }
        if !line.ends_with(';') {
            return Err(format!("statement does not end with `;`: {line}"));
        }
        let mut inside = false;
        let mut escaped = false;
        for character in line.chars() {
            match character {
                '\\' if inside => escaped = !escaped,
                '"' if inside && !escaped => inside = false,
                '"' if !inside => inside = true,
                '{' | '}' if !inside => return Err(format!("a brace outside a string: {line}")),
                _ => escaped = false,
            }
        }
        if inside {
            return Err(format!("unterminated string: {line}"));
        }
        statements += 1;
    }
    if !closed {
        return Err("the graph is never closed".to_string());
    }
    Ok(statements)
}

// ------------------------------------------------------------------- tests

async fn built(layer: Layer, endpoints: bool) -> Graph {
    let campus = campus();
    graph::build(
        &campus.app,
        &Options {
            layer,
            include_endpoints: endpoints,
            ..Options::default()
        },
    )
    .await
    .expect("the graph is built")
}

#[test]
fn the_xml_checker_catches_what_it_is_for() {
    assert!(parse_xml("<a><b x=\"1\"/>text</a>").is_ok());
    assert!(parse_xml("<a>&amp;&lt;&#10;</a>").is_ok());
    assert!(parse_xml("<?xml version=\"1.0\"?><!-- hi --><a/>").is_ok());
    for bad in [
        "<a><b></a>",
        "<a>raw < text</a>",
        "<a x=1/>",
        "<a x=\"<b>\"/>",
        "<a>bare & ampersand</a>",
        "<a>",
        "no elements",
        "<a/><b/>",
    ] {
        assert!(parse_xml(bad).is_err(), "`{bad}` should be rejected");
    }
}

#[test]
fn the_mermaid_and_dot_checkers_catch_what_they_are_for() {
    assert!(check_mermaid("graph TD\n  n0[\"a\"]").is_ok());
    for bad in [
        "flowchart TD\n  n0[\"a\"]",
        "graph TD\n  n0[\"a<b>\"]",
        "graph TD\n  n0[a]",
        "graph TD\n  junk",
    ] {
        assert!(check_mermaid(bad).is_err(), "`{bad}` should be rejected");
    }
    assert!(check_dot("graph t {\n  n0 [label=\"a\"];\n}").is_ok());
    for bad in [
        "digraph t {\n}",
        "graph t {\n  n0 [label=\"a\"]\n}",
        "graph t {\n  n0 [label=\"a];\n}",
        "graph t {\n  n0;",
    ] {
        assert!(check_dot(bad).is_err(), "`{bad}` should be rejected");
    }
}

#[tokio::test]
async fn every_format_carries_every_node() {
    let graph = built(Layer::Physical, true).await;
    assert!(graph.nodes.len() > 25);
    for format in [
        Format::Svg,
        Format::Drawio,
        Format::Mermaid,
        Format::Dot,
        Format::Json,
    ] {
        let text = export::render(&graph, format).expect("the export is written");
        for node in &graph.nodes {
            assert!(
                text.contains(&node.name),
                "{} is missing from the {} export",
                node.name,
                format.as_str()
            );
        }
        assert!(
            text.len() > 500,
            "the {} export is suspiciously short",
            format.as_str()
        );
    }
}

#[tokio::test]
async fn the_svg_is_well_formed_and_carries_a_legend() {
    let graph = built(Layer::Physical, false).await;
    let svg = export::render(&graph, Format::Svg).unwrap();
    let text = parse_xml(&svg).expect("the SVG parses");
    assert!(svg.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(svg.contains("xmlns=\"http://www.w3.org/2000/svg\""));
    assert!(text.iter().any(|item| item == "Physical topology"));
    assert!(text.iter().any(|item| item == "LEGEND"));
    assert!(
        text.iter()
            .any(|item| item.contains("confirmed — both ends report the link"))
    );
    assert!(
        text.iter().any(|item| item.contains("26 devices")),
        "{text:?}"
    );
    assert!(
        text.iter().any(|item| item.contains("not in inventory")),
        "the unmanaged access point is labelled: {text:?}"
    );

    let l3 = export::render(&built(Layer::L3, false).await, Format::Svg).unwrap();
    let text = parse_xml(&l3).expect("the layer 3 SVG parses");
    assert!(text.iter().any(|item| item == "Layer 3 topology"));
}

#[tokio::test]
async fn the_drawio_file_is_well_formed_and_has_a_cell_per_node() {
    let graph = built(Layer::Physical, false).await;
    let xml = export::render(&graph, Format::Drawio).unwrap();
    parse_xml(&xml).expect("the draw.io file parses");
    assert!(xml.contains("<mxfile") && xml.contains("<mxGraphModel"));
    let cells = xml.matches("<mxCell ").count();
    assert!(
        cells >= 2 + graph.nodes.len() + graph.links.len(),
        "{cells} cells for {} nodes and {} links",
        graph.nodes.len(),
        graph.links.len()
    );
    assert!(xml.contains("shape=mxgraph.networks.switch;"));
}

#[tokio::test]
async fn mermaid_and_dot_parse_by_their_own_grammar() {
    let graph = built(Layer::Physical, true).await;
    let mermaid = export::render(&graph, Format::Mermaid).unwrap();
    assert_eq!(
        check_mermaid(&mermaid).expect("the mermaid source parses"),
        graph.nodes.len()
    );
    let dot = export::render(&graph, Format::Dot).unwrap();
    let statements = check_dot(&dot).expect("the DOT source parses");
    assert!(statements >= graph.nodes.len() + graph.links.len());
    assert!(dot.contains("graph topology {"));
}

#[tokio::test]
async fn the_json_export_is_the_graph() {
    let graph = built(Layer::Physical, false).await;
    let text = export::render(&graph, Format::Json).unwrap();
    let parsed: Graph = serde_json::from_str(&text).expect("the JSON is a graph");
    assert_eq!(parsed.nodes.len(), graph.nodes.len());
    assert_eq!(parsed.links.len(), graph.links.len());
    assert_eq!(parsed.stats, graph.stats);
    assert_eq!(parsed.layer, Layer::Physical);
}

#[tokio::test]
async fn a_hostile_device_name_cannot_escape_any_format() {
    let app = app();
    let switch = device(&app, HOSTILE, "10.0.0.1", "core", HOSTILE);
    let other = device(&app, "SW2", "10.0.0.2", "access", "HQ");
    store(
        &app,
        &switch,
        vec![(
            Kind::Neighbors,
            vec![
                neighbor("Gi0/1", "SW2", "Gi0/1", "lldp"),
                neighbor_full(
                    "Gi0/2",
                    "</text><script>bad()</script>",
                    "]]>",
                    "cdp",
                    "",
                    "<platform>",
                    "Switch",
                ),
            ],
        )],
    );
    store(
        &app,
        &other,
        vec![(
            Kind::Neighbors,
            vec![neighbor("Gi0/1", HOSTILE, "Gi0/1", "lldp")],
        )],
    );

    let graph = graph::build(&app, &Options::default()).await.unwrap();
    assert_eq!(graph.nodes.len(), 3, "{:#?}", graph.nodes);

    let svg = export::render(&graph, Format::Svg).unwrap();
    parse_xml(&svg).expect("the SVG still parses");
    assert!(!svg.contains("<script"), "the SVG carries live markup");
    assert!(!svg.contains("]]>"));
    assert!(svg.contains("&lt;script&gt;"), "the name is there, escaped");

    let drawio = export::render(&graph, Format::Drawio).unwrap();
    parse_xml(&drawio).expect("the draw.io file still parses");
    assert!(!drawio.contains("<script"));
    assert!(!drawio.contains("]]>"));

    let mermaid = export::render(&graph, Format::Mermaid).unwrap();
    check_mermaid(&mermaid).expect("the mermaid source still parses");
    assert!(!mermaid.contains("<script"));
    assert!(!mermaid.contains("]]>"));
    // Mermaid renders labels as HTML, so nothing may reach it unescaped.
    for line in mermaid.lines().skip(1) {
        for label in line.split('"').skip(1).step_by(2) {
            assert!(
                !label.contains('<') && !label.contains('>') && !label.contains('"'),
                "mermaid label `{label}` is not inert"
            );
        }
    }

    let dot = export::render(&graph, Format::Dot).unwrap();
    check_dot(&dot).expect("the DOT source still parses");
    assert!(dot.contains("\\\""), "the quote in the name is escaped");

    let json = export::render(&graph, Format::Json).unwrap();
    let parsed: Graph = serde_json::from_str(&json).expect("the JSON is still a graph");
    assert!(
        parsed.nodes.iter().any(|node| node.name == HOSTILE),
        "JSON carries the name as it is, which is what JSON is for"
    );
}

#[tokio::test]
async fn an_empty_graph_still_exports() {
    let app = app();
    let graph = graph::build(&app, &Options::default()).await.unwrap();
    assert!(graph.nodes.is_empty());
    parse_xml(&export::render(&graph, Format::Svg).unwrap()).expect("an empty SVG is an SVG");
    parse_xml(&export::render(&graph, Format::Drawio).unwrap()).expect("empty draw.io");
    check_mermaid(&export::render(&graph, Format::Mermaid).unwrap()).expect("empty mermaid");
    check_dot(&export::render(&graph, Format::Dot).unwrap()).expect("empty dot");
}

/// Writes the campus drawings somewhere a person can open them. Not part of
/// the suite: a drawing is judged by looking at it, which a test cannot do.
///
/// `NOCTURNE_TOPOLOGY_OUT=/some/dir cargo test -p nocturne-topology -- --ignored --nocapture`
#[tokio::test]
#[ignore = "writes files for a visual check"]
async fn write_the_campus_drawings() {
    let directory = std::env::var("NOCTURNE_TOPOLOGY_OUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    std::fs::create_dir_all(&directory).expect("the output directory");
    for (layer, endpoints, name) in [
        (Layer::Physical, false, "campus-physical"),
        (Layer::Physical, true, "campus-endpoints"),
        (Layer::L3, false, "campus-l3"),
    ] {
        let graph = built(layer, endpoints).await;
        for format in [Format::Svg, Format::Drawio, Format::Mermaid, Format::Dot] {
            let path = directory.join(format!("{name}.{}", format.extension()));
            std::fs::write(&path, export::render(&graph, format).unwrap()).expect("written");
            println!("wrote {}", path.display());
        }
    }
}
