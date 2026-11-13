//! RIP (placeholder until tier 3).

use std::net::Ipv4Addr;

use crate::ifname::IfId;
use crate::sim::frame::RipMsg;
use crate::sim::fx::Fx;
use crate::sim::node::Node;

#[derive(Clone, Debug, Default)]
pub(crate) struct Rip {}

pub(crate) fn show_protocols(_node: &Node, _now: u64) -> String {
    String::new()
}

impl Node {
    pub(crate) fn rip_sync(&mut self, _fx: &mut Fx) {}
    pub(crate) fn rip_rx(&mut self, _fx: &mut Fx, _iface: IfId, _src: Ipv4Addr, _msg: &RipMsg) {}
    pub(crate) fn rip_update_timer(&mut self, _fx: &mut Fx, _token: u32) {}
    pub(crate) fn rip_triggered(&mut self, _fx: &mut Fx) {}
    pub(crate) fn rip_table_changed(&mut self, _fx: &mut Fx) {}
}
