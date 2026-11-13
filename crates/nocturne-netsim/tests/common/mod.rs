//! Helpers shared by the scenario tests.
#![allow(dead_code)]

use nocturne_netsim::{CableKind, ConsoleId, HostConfig, Lab, NodeId};

pub const SECOND: u64 = 1_000_000;

/// Static addressing for an end host.
pub fn host(ip: &str, mask: &str, gateway: Option<&str>) -> HostConfig {
    HostConfig {
        ip: Some(ip.to_string()),
        mask: Some(mask.to_string()),
        gateway: gateway.map(str::to_string),
        ..HostConfig::default()
    }
}

/// Add a node, panicking with context on failure.
pub fn node(lab: &mut Lab, model: &str, name: &str) -> NodeId {
    lab.add_node(model, Some(name), 0.0, 0.0).unwrap_or_else(|e| panic!("add {name}: {e}"))
}

/// Add an end host with a static address.
pub fn pc(lab: &mut Lab, name: &str, ip: &str, gateway: Option<&str>) -> NodeId {
    let id = node(lab, "pc", name);
    lab.set_host_config(id, host(ip, "255.255.255.0", gateway)).unwrap();
    id
}

pub fn link(lab: &mut Lab, a: NodeId, a_if: &str, b: NodeId, b_if: &str) -> u32 {
    lab.add_link(a, a_if, b, b_if, CableKind::Auto).unwrap_or_else(|e| panic!("link {a_if} - {b_if}: {e}"))
}

/// Apply configuration lines and require that every one is accepted.
pub fn cfg(lab: &mut Lab, node: NodeId, text: &str) {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let outcome = lab.configure(node, &lines).unwrap();
    let rejected: Vec<_> = outcome.lines.iter().filter(|l| l.error).collect();
    assert!(rejected.is_empty(), "rejected configuration lines: {rejected:#?}");
}

pub fn exec(lab: &mut Lab, node: NodeId, command: &str) -> String {
    lab.exec(node, command).unwrap_or_else(|e| panic!("{command}: {e}"))
}

/// Type a line on a console and return what the console printed.
pub fn typed(lab: &mut Lab, console: ConsoleId, text: &str) -> String {
    lab.console_input(console, text.as_bytes());
    lab.console_input(console, b"\r");
    output(lab, console)
}

pub fn output(lab: &mut Lab, console: ConsoleId) -> String {
    String::from_utf8_lossy(&lab.console_output(console)).into_owned()
}

/// The success marks of an IOS ping (`.!!!!`).
pub fn ping_marks(output: &str) -> String {
    output.lines().find(|l| !l.is_empty() && l.chars().all(|c| matches!(c, '!' | '.' | 'U'))).unwrap_or("").to_string()
}

/// Count of answered echoes in a host (Windows-style) ping.
pub fn replies(output: &str) -> usize {
    output.lines().filter(|l| l.starts_with("Reply from") && l.contains("bytes=")).count()
}
