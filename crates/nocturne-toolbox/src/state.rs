//! The crate's runtime state: the two servers, when one is running.
//!
//! Nothing else needs state. The calculators are pure, and a probe's state is
//! its job and its history record. A server, on the other hand, holds a socket
//! and has to be findable again — to report on it, and to stop it.

use parking_lot::Mutex;

use crate::servers::{syslog, tftp};

#[derive(Default)]
pub struct State {
    pub tftp: Mutex<Option<tftp::Running>>,
    pub syslog: Mutex<Option<syslog::Running>>,
}
