//! Generate a design, boot it and report everything that went wrong.

use std::env;
use std::net::Ipv4Addr;
use std::time::Instant;

use nocturne_design::{Expect, Requirements, Site, document, examples, generate};
use nocturne_netsim::Lab;

const SECOND: u64 = 1_000_000;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let which = args.first().map(String::as_str).unwrap_or("branch");
    let (name, req) = match which {
        "branch" => (
            "Acme branch".to_string(),
            Requirements {
                name: "Acme branch".into(),
                sites: vec![Site {
                    name: "Branch".into(),
                    users: 120,
                    voice: true,
                    guest_wifi: true,
                    servers: 2,
                    printers: 3,
                    vlans: Vec::new(),
                }],
                redundancy: nocturne_design::Redundancy::Uplinks,
                hosts_per_vlan: 2,
                ..Requirements::default()
            },
        ),
        "big" => {
            let n: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2000);
            let r = match args.get(2).map(String::as_str) {
                Some("uplinks") => nocturne_design::Redundancy::Uplinks,
                Some("full") => nocturne_design::Redundancy::Full,
                _ => nocturne_design::Redundancy::None,
            };
            (
                format!("{n} users, {r:?}"),
                Requirements {
                    name: "Big".into(),
                    sites: vec![Site {
                        name: "Tower".into(),
                        users: n,
                        voice: true,
                        guest_wifi: true,
                        servers: 4,
                        printers: 10,
                        vlans: Vec::new(),
                    }],
                    redundancy: r,
                    growth_pct: 0,
                    addressing: nocturne_design::Addressing {
                        base: "10.0.0.0/12".into(),
                        ..Default::default()
                    },
                    ..Requirements::default()
                },
            )
        }
        n => {
            let i: usize = n.parse().unwrap_or(0);
            let e = examples();
            let (label, req) = e.get(i).cloned().unwrap_or_else(|| e[0].clone());
            (label, req)
        }
    };
    println!("=== {name}");
    let t0 = Instant::now();
    let design = match generate(&req) {
        Ok(d) => d,
        Err(e) => {
            println!("FAILED: {e}");
            return;
        }
    };
    println!("generated in {:?}", t0.elapsed());
    println!(
        "{} nodes, {} cables, {} checks",
        design.lab.nodes.len(),
        design.lab.links.len(),
        design.plan.checks.len()
    );
    for w in &design.warnings {
        println!("WARN  {w}");
    }
    let t1 = Instant::now();
    let mut lab = match Lab::from_spec(&design.lab) {
        Ok(l) => l,
        Err(e) => {
            println!("BOOT FAILED: {e}");
            if env::var("SHOWCFG").is_ok() {
                for n in &design.lab.nodes {
                    if let Some(c) = &n.startup_config {
                        println!("---- {}\n{c}", n.name);
                    }
                }
            }
            return;
        }
    };
    let mut rejected = 0;
    for node in lab.snapshot().nodes {
        for (line, why) in lab.startup_errors(node.id) {
            rejected += 1;
            if rejected < 25 {
                println!("REJECT {} : {line}  -> {}", node.name, why.trim());
            }
        }
    }
    lab.settle(240 * SECOND);
    println!(
        "booted+settled in {:?} (sim {}s), {rejected} rejected lines",
        t1.elapsed(),
        lab.now() / SECOND
    );
    let mut bad = 0;
    for check in &design.plan.checks {
        let Some(from) = lab.node_by_name(&check.from) else {
            println!("CHECK no node {}", check.from);
            bad += 1;
            continue;
        };
        let Ok(to) = check.to.parse::<Ipv4Addr>() else {
            println!("CHECK bad address {}", check.to);
            bad += 1;
            continue;
        };
        let result = match check.port {
            Some(port) => lab.probe_tcp(from, to, port),
            None => lab.probe_ping(from, to),
        };
        let want = check.expect == Expect::Reachable;
        if result.ok != want {
            bad += 1;
            println!(
                "CHECK FAIL {} -> {} ({}) {:?} expected {:?}: {}",
                check.from, check.to, check.to_name, check.port, check.expect, result.reason
            );
            for hop in &result.hops {
                println!(
                    "        {} in={} out={} : {}",
                    hop.name, hop.in_if, hop.out_if, hop.note
                );
            }
        }
    }
    println!("{bad} failing checks of {}", design.plan.checks.len());
    if env::var("SHOWCHECKS").is_ok() {
        for check in &design.plan.checks {
            let from = lab.node_by_name(&check.from).unwrap();
            let to: Ipv4Addr = check.to.parse().unwrap();
            let r = match check.port {
                Some(p) => lab.probe_tcp(from, to, p),
                None => lab.probe_ping(from, to),
            };
            let want = check.expect == Expect::Reachable;
            println!(
                "| {} | {} ({}) | {} | {} | {} |",
                check.from,
                check.to,
                check.to_name,
                match check.port {
                    Some(p) => format!("tcp/{p}"),
                    None => "ping".into(),
                },
                if want { "reachable" } else { "blocked" },
                if r.ok == want { "PASS" } else { "FAIL" }
            );
        }
    }
    if env::var("SHOWCFG").is_ok() {
        for n in &design.lab.nodes {
            if let Some(c) = &n.startup_config {
                println!("---- {}\n{c}", n.name);
            }
        }
    }
    if env::var("SHOWDOC").is_ok() {
        println!("{}", document(&design));
    }
}
