//! Deterministic network simulator: IOS-style CLI devices on a simulated L2/L3 dataplane.
//!
//! The crate is pure and synchronous: no threads, no wall clock, no IO. The host
//! owns a [`Lab`], feeds it console bytes and API calls, and drives simulated time
//! with [`Lab::advance`]. Identical inputs produce byte-identical outputs.

#![forbid(unsafe_code)]

mod api;
mod catalog;
pub mod demo;
mod device;
mod ifname;
mod lab;
mod sim;
mod util;
mod views;

pub use api::*;
pub use catalog::catalog;
pub use demo::{demo_campus, examples};
pub use lab::Lab;
