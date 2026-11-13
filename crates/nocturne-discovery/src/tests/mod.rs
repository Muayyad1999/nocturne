//! This crate's tests. Everything is real except the packets: the core, the
//! job surface, the profile registry, the inventory and the store are the
//! ones that ship, and `support::FakeProbes` stands in for the network where
//! a test needs to decide what answers (`live` uses the real thing).

mod adding;
mod crawling;
mod live;
mod records;
mod support;
mod sweeping;
mod targets;
