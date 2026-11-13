//! NAT (placeholder until tier 2: no translation is performed and the CLI does
//! not offer the commands yet).

use std::net::Ipv4Addr;

use crate::ifname::IfId;
use crate::sim::frame::Packet;
use crate::sim::fx::Fx;
use crate::sim::node::Node;

#[derive(Clone, Debug, Default)]
pub(crate) struct Nat {}

impl Node {
    pub(crate) fn nat_inbound(&mut self, _fx: &mut Fx, _packet: &mut Packet) -> Option<String> {
        None
    }

    pub(crate) fn nat_outbound(&mut self, _fx: &mut Fx, _packet: &mut Packet, _out: IfId) -> Result<Option<String>, String> {
        Ok(None)
    }

    pub(crate) fn nat_owns_global(&self, _iface: IfId, _ip: Ipv4Addr) -> bool {
        false
    }

    pub(crate) fn nat_rows(&self, _now: u64) -> Vec<crate::api::NatRow> {
        Vec::new()
    }
    pub(crate) fn nat_sync(&mut self) {}
    pub(crate) fn nat_clear(&mut self) {}
}
