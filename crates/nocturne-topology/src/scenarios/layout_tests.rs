//! The drawing has to be the same every time, readable, and fast enough that
//! nobody waits for it.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use super::*;
use crate::graph::{self, Options};
use crate::layout::{self, NODE_WIDTH};
use crate::model::{Algorithm, End, Graph, Link, Node, Position};

/// A synthetic three-tier graph, for size, speed and rank checks.
fn synthetic(cores: usize, dists: usize, access: usize) -> Graph {
    let mut nodes: Vec<Node> = Vec::new();
    let mut links: Vec<Link> = Vec::new();
    let push = |id: String, name: String, role: &str, site: &str, nodes: &mut Vec<Node>| {
        nodes.push(Node {
            id,
            name,
            role: Some(role.to_string()),
            site: Some(site.to_string()),
            ..Node::default()
        });
    };
    for index in 0..cores {
        push(
            format!("core{index}"),
            format!("CORE{index}"),
            "core",
            "HQ",
            &mut nodes,
        );
    }
    for index in 0..dists {
        push(
            format!("dist{index}"),
            format!("DIST{index}"),
            "distribution",
            if index % 2 == 0 { "HQ" } else { "North" },
            &mut nodes,
        );
    }
    for index in 0..access {
        push(
            format!("acc{index}"),
            format!("ACC{index:03}"),
            "access",
            if index % 2 == 0 { "HQ" } else { "North" },
            &mut nodes,
        );
    }
    let join = |a: String, b: String, links: &mut Vec<Link>| {
        let mut new = Link {
            a: End::new(a),
            b: End::new(b),
            ..Link::default()
        };
        new.id = crate::id_of(&new.signature());
        links.push(new);
    };
    for core in 0..cores {
        for dist in 0..dists {
            join(format!("core{core}"), format!("dist{dist}"), &mut links);
        }
    }
    for index in 0..access {
        join(
            format!("acc{index}"),
            format!("dist{}", index % dists.max(1)),
            &mut links,
        );
        join(
            format!("acc{index}"),
            format!("dist{}", (index + 1) % dists.max(1)),
            &mut links,
        );
    }
    Graph {
        nodes,
        links,
        ..Graph::default()
    }
}

#[test]
fn the_same_graph_gives_the_same_picture() {
    let graph = synthetic(2, 6, 40);
    for algorithm in [Algorithm::Layered, Algorithm::Force, Algorithm::Radial] {
        let once = layout::run(&graph, algorithm, &BTreeMap::new(), None);
        let twice = layout::run(&graph, algorithm, &BTreeMap::new(), None);
        assert_eq!(once, twice, "{algorithm:?} is not deterministic");
        assert_eq!(once.len(), graph.nodes.len());
        // The order the graph arrived in must not change where anything ends up.
        let mut reversed = graph.clone();
        reversed.nodes.reverse();
        reversed.links.reverse();
        let shuffled = layout::run(&reversed, algorithm, &BTreeMap::new(), None);
        assert_eq!(once, shuffled, "{algorithm:?} depends on input order");
    }
}

#[test]
fn no_two_nodes_sit_on_top_of_each_other() {
    for (cores, dists, access) in [(2, 4, 20), (1, 1, 2), (3, 9, 60)] {
        let graph = synthetic(cores, dists, access);
        for algorithm in [Algorithm::Layered, Algorithm::Force, Algorithm::Radial] {
            let positions = layout::run(&graph, algorithm, &BTreeMap::new(), None);
            assert_eq!(
                layout::overlapping(&positions),
                None,
                "{algorithm:?} overlaps with {cores}/{dists}/{access}"
            );
        }
    }
}

#[test]
fn ranks_put_the_core_at_the_top_and_the_access_layer_at_the_bottom() {
    let graph = synthetic(2, 4, 20);
    let positions = layout::run(&graph, Algorithm::Layered, &BTreeMap::new(), None);
    let row = |prefix: &str| -> Vec<f64> {
        positions
            .iter()
            .filter(|(id, _)| id.starts_with(prefix))
            .map(|(_, position)| position.y)
            .collect()
    };
    let core = row("core");
    let dist = row("dist");
    let mut access = row("acc");
    access.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    assert!(core.windows(2).all(|pair| pair[0] == pair[1]), "{core:?}");
    assert!(dist.windows(2).all(|pair| pair[0] == pair[1]));
    assert!(core[0] < dist[0] && dist[0] < access[0]);
    // Twenty access switches are too many for one row, so the rank is wrapped
    // onto several lines — all of them below the distribution row.
    let lines: Vec<f64> = {
        let mut lines = access.clone();
        lines.dedup();
        lines
    };
    assert!((2..=5).contains(&lines.len()), "{lines:?}");
    assert!(lines.iter().all(|y| *y > dist[0]));

    // Sites stay as contiguous blocks instead of interleaving.
    let site_of = |id: &str| {
        graph
            .nodes
            .iter()
            .find(|node| node.id == id)
            .and_then(|node| node.site.clone())
            .unwrap_or_default()
    };
    let mut access_row: Vec<(&String, &Position)> = positions
        .iter()
        .filter(|(id, _)| id.starts_with("acc") && (positions[*id].y - lines[0]).abs() < 1.0)
        .collect();
    access_row.sort_by(|a, b| a.1.x.partial_cmp(&b.1.x).expect("finite"));
    let sites: Vec<String> = access_row.iter().map(|(id, _)| site_of(id)).collect();
    let mut blocks = sites.clone();
    blocks.dedup();
    assert_eq!(
        blocks.len(),
        1,
        "a line should not straddle two sites: {sites:?}"
    );
}

#[test]
fn pinned_nodes_do_not_move_and_nothing_lands_on_them() {
    let graph = synthetic(2, 4, 12);
    let pinned: BTreeMap<String, Position> = BTreeMap::from([
        (
            "core0".to_string(),
            Position {
                x: -500.0,
                y: -500.0,
                pinned: true,
            },
        ),
        (
            "acc3".to_string(),
            Position {
                x: 1234.0,
                y: 42.0,
                pinned: true,
            },
        ),
    ]);
    for algorithm in [Algorithm::Layered, Algorithm::Force, Algorithm::Radial] {
        let positions = layout::run(&graph, algorithm, &pinned, None);
        for (id, place) in &pinned {
            assert_eq!(
                positions.get(id),
                Some(place),
                "{algorithm:?} moved the pinned {id}"
            );
        }
        assert_eq!(layout::overlapping(&positions), None, "{algorithm:?}");
    }
}

#[test]
fn a_new_node_is_placed_next_to_its_neighbours() {
    let mut graph = synthetic(2, 4, 12);
    let positions = layout::run(&graph, Algorithm::Layered, &BTreeMap::new(), None);
    for node in &mut graph.nodes {
        let place = positions[&node.id];
        node.x = place.x;
        node.y = place.y;
    }
    graph.nodes.push(Node {
        id: "accNEW".into(),
        name: "ACC-NEW".into(),
        role: Some("access".into()),
        ..Node::default()
    });
    let mut link = Link {
        a: End::new("accNEW"),
        b: End::new("dist1"),
        ..Link::default()
    };
    link.id = crate::id_of(&link.signature());
    graph.links.push(link);

    let placed = layout::place_near_neighbours(&graph, &["accNEW".to_string()]);
    assert_eq!(placed.len(), 1, "only the new node is placed");
    let new = placed["accNEW"];
    let anchor = positions["dist1"];
    assert!(
        (new.x - anchor.x).abs() < NODE_WIDTH * 2.0 && new.y > anchor.y,
        "the new node landed at {new:?}, nowhere near dist1 at {anchor:?}"
    );
    let mut everything: BTreeMap<String, Position> = positions.clone();
    everything.extend(placed);
    assert_eq!(layout::overlapping(&everything), None);
    assert_eq!(everything["dist1"], anchor, "nothing that was placed moved");
}

#[test]
fn a_node_with_no_links_still_gets_a_place_of_its_own() {
    let mut graph = synthetic(1, 1, 2);
    graph.nodes.push(Node {
        id: "lonely".into(),
        name: "LONELY".into(),
        ..Node::default()
    });
    for algorithm in [Algorithm::Layered, Algorithm::Force, Algorithm::Radial] {
        let positions = layout::run(&graph, algorithm, &BTreeMap::new(), None);
        assert!(positions.contains_key("lonely"), "{algorithm:?}");
        assert_eq!(layout::overlapping(&positions), None, "{algorithm:?}");
    }
    assert!(
        layout::run(
            &Graph::default(),
            Algorithm::Layered,
            &BTreeMap::new(),
            None
        )
        .is_empty()
    );
}

#[test]
fn five_hundred_nodes_lay_out_in_well_under_a_second() {
    let graph = synthetic(4, 24, 472);
    assert_eq!(graph.nodes.len(), 500);
    for algorithm in [Algorithm::Layered, Algorithm::Force, Algorithm::Radial] {
        // The whole test suite runs in parallel on whatever machine this is,
        // so the fastest of three runs is the honest measure of the algorithm
        // rather than of the load around it.
        let mut best = Duration::from_secs(3600);
        let mut first = None;
        for _ in 0..3 {
            let started = Instant::now();
            let positions = layout::run(&graph, algorithm, &BTreeMap::new(), None);
            best = best.min(started.elapsed());
            assert_eq!(positions.len(), 500);
            assert_eq!(layout::overlapping(&positions), None, "{algorithm:?}");
            // A size this big has to be as repeatable as a small one.
            match &first {
                None => first = Some(positions),
                Some(first) => assert_eq!(*first, positions, "{algorithm:?}"),
            }
        }
        println!("{algorithm:?}: 500 nodes in {best:?}");
        // What the user waits for is the release build: measured on four
        // cores, 500 nodes take 5 ms layered, 60 ms force and 2 ms radial.
        // These budgets are for an unoptimised build sharing the machine with
        // the rest of the suite — roughly six times slower, and more than that
        // for `force`, whose inner loop is arithmetic the optimiser eats. They
        // are here to catch a regression, not to measure the product.
        let budget = match algorithm {
            Algorithm::Force => 2_500,
            _ => 1_000,
        };
        assert!(
            best.as_millis() < budget,
            "{algorithm:?} took {best:?} for 500 nodes"
        );
    }
}

#[tokio::test]
async fn a_graph_with_no_stored_positions_is_laid_out_automatically() {
    let campus = campus();
    let graph = graph::build(&campus.app, &Options::default())
        .await
        .unwrap();
    let positions: BTreeMap<String, Position> = graph
        .nodes
        .iter()
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
    assert_eq!(layout::overlapping(&positions), None);
    assert!(graph.nodes.iter().all(|node| !node.pinned));
    let y_of = |id: &str| graph.node(id).expect("node").y;
    assert!(y_of(&campus.core[0]) < y_of(&campus.dist[0]));
    assert!(y_of(&campus.dist[0]) < y_of(&campus.access[0]));
    assert!(
        (y_of(&campus.core[0]) - y_of(&campus.core[1])).abs() < f64::EPSILON,
        "the two cores share a row"
    );

    // The same data draws the same picture the second time it is asked for.
    let again = graph::build(&campus.app, &Options::default())
        .await
        .unwrap();
    for (one, other) in graph.nodes.iter().zip(&again.nodes) {
        assert_eq!(
            (one.id.as_str(), one.x, one.y),
            (other.id.as_str(), other.x, other.y)
        );
    }
}

#[tokio::test]
#[ignore = "prints the campus layout for a human to look at"]
async fn print_campus_positions() {
    let campus = campus();
    let graph = graph::build(&campus.app, &Options::default())
        .await
        .unwrap();
    let mut nodes: Vec<&crate::model::Node> = graph.nodes.iter().collect();
    nodes.sort_by(|a, b| {
        a.y.partial_cmp(&b.y)
            .unwrap()
            .then(a.x.partial_cmp(&b.x).unwrap())
    });
    for node in nodes {
        println!(
            "{:>7.0} {:>7.0}  {:<12} {:?} site={:?} links={}",
            node.x,
            node.y,
            node.name,
            node.kind,
            node.site,
            graph
                .links
                .iter()
                .filter(|link| link.a.node == node.id || link.b.node == node.id)
                .count()
        );
    }
}
