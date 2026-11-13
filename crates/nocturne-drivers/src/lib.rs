//! Everything Nocturne needs to talk to a network device's CLI and make sense
//! of what comes back.
//!
//! * [`profile`] — device types are **data**: a YAML profile describes a
//!   platform's prompts, paging, config mode, safety policy and how to turn
//!   its `show` output into the normalised [`model`]. Adding a device type
//!   means adding a file, not code.
//! * [`term`] — cleans raw terminal bytes (escape sequences, backspaces,
//!   pager redraws) into plain text.
//! * [`hier`] — parses configuration into a tree and computes the exact
//!   commands that turn one configuration into another.
//! * [`policy`] — classifies a command as read-only, state-changing or
//!   dangerous before anything is sent.

pub mod expr;
pub mod hier;
pub mod ifname;
pub mod model;
pub mod parse;
pub mod policy;
pub mod profile;
pub mod session;
pub mod term;
