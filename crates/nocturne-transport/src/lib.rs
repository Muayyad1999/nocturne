//! Interactive device transports: SSH, Telnet, serial console and local shell.
//!
//! Every interactive session — a human typing in a terminal tab, a driver
//! scraping `show` output, a change being pushed — runs over one abstraction:
//! a [`Connection`], a duplex byte stream with resize and break
//! side-channels.

#![allow(dead_code)] // TEMP while the crate is being assembled

mod connection;
mod error;
mod net;
mod pump;
mod relay;

pub use connection::{
    Backend, Connection, ConnectionInfo, ConnectionReader, ConnectionWriter, Input, Output,
    TransportKind,
};
pub use error::TransportError;
