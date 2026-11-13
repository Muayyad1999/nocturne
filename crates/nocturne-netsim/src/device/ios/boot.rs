//! Booting: a device comes up by feeding its startup configuration through
//! the same command path a human would use.

use crate::api::NodeId;
use crate::device::ios::{IosShell, Mode};
use crate::sim::World;
use crate::sim::fx::Fx;
use crate::sim::node::Node;

/// Apply configuration text line by line in global configuration mode.
/// Returns the lines that were rejected, with the device's response.
pub(crate) fn apply_lines(
    shell: &mut IosShell,
    node: &mut Node,
    fx: &mut Fx,
    text: &str,
) -> Vec<(String, String)> {
    let mut rejected = Vec::new();
    for line in text.lines() {
        let reply = shell.execute(node, fx, line);
        if reply.error {
            rejected.push((line.to_string(), reply.out));
        }
        // `end` leaves configuration mode; anything after it is not configuration.
        if !shell.in_config() {
            break;
        }
    }
    rejected
}

/// Load the node's startup configuration into its (factory-fresh) running one.
pub(crate) fn apply_startup(world: &mut World, id: NodeId) {
    world.with_node(id, |node, fx| {
        if let Some(text) = node.startup.clone() {
            let mut shell = IosShell::automation(Mode::Config);
            node.boot_errors = apply_lines(&mut shell, node, fx, &text);
        } else {
            node.boot_errors.clear();
        }
        node.unsaved = false;
        node.last_config_change = None;
        // Nothing done while loading the configuration is an event of its own.
        fx.events.clear();
        node.log.clear();
    });
}
