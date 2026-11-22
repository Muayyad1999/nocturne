//! Topology graph from collected data, layout, path trace, host location and diagram export.
//!
//! Specification: `docs/dev/features/topology.md` (conventions: `docs/dev/features/README.md`).
//!
//! The network drawn from what the devices themselves report. Everything here
//! is pure computation over `nocturne_app::collect` and the inventory: this
//! crate never opens a connection to a device, so a "Refresh" in the UI is a
//! `collect.run` and then a redraw.
//!
//! What it is honest about:
//!
//! * A link is `confirmed` when both ends reported it, `one_sided` when only
//!   one did, and `inferred` when it was derived from addresses or MAC tables.
//! * A neighbour that is not in the inventory becomes an `unmanaged` node
//!   rather than being dropped or invented into a device.
//! * Data older than the staleness setting is drawn, counted and named in
//!   `warnings`; it is never silently hidden.
//! * `topology.path` stops and says which device it stopped at and what is
//!   missing instead of guessing the next hop.

#![forbid(unsafe_code)]

mod api;
mod data;
mod endpoints;
mod export;
mod graph;
mod history;
mod l3;
mod layout;
mod locate;
mod manual;
mod model;
mod paged;
mod path;
mod physical;
mod rates;
#[cfg(test)]
mod scenarios;
mod views;

pub use export::Format;
pub use graph::Options;
pub use model::{
    Algorithm, Confidence, DataAge, End, Filter, Graph, Group, GroupKind, Layer, Link, LinkKind,
    Node, NodeKind, Position, Settings, Stats, Status, View,
};

/// Register this feature's settings, state, RPC methods and background tasks.
pub fn init(builder: &mut nocturne_app::AppBuilder) {
    api::init(builder);
}

/// A short, stable id derived from a link's identity, so that the same link
/// keeps the same id across rebuilds and across machines.
fn id_of(signature: &str) -> String {
    // FNV-1a: tiny, deterministic, and good enough to name a link.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in signature.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("l{hash:016x}")
}

/// Sort names the way a person reads them: `SW2` before `SW10`.
fn graph_sort_key(name: &str) -> Vec<(String, u64)> {
    let mut parts = Vec::new();
    let mut letters = String::new();
    let mut digits = String::new();
    for character in name.to_lowercase().chars() {
        if character.is_ascii_digit() {
            digits.push(character);
        } else {
            if !digits.is_empty() {
                parts.push((std::mem::take(&mut letters), digits.parse().unwrap_or(0)));
                digits.clear();
            }
            letters.push(character);
        }
    }
    parts.push((letters, digits.parse().unwrap_or(0)));
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_ids_are_stable_and_distinct() {
        assert_eq!(id_of("a::b"), id_of("a::b"));
        assert_ne!(id_of("a::b"), id_of("a::c"));
        assert_eq!(id_of("a::b").len(), 17);
    }

    #[test]
    fn names_sort_naturally() {
        let mut names = vec!["SW10", "SW2", "SW1"];
        names.sort_by_key(|name| graph_sort_key(name));
        assert_eq!(names, ["SW1", "SW2", "SW10"]);
    }
}
