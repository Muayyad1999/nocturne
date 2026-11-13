//! OSPF (placeholder until tier 2).

use std::net::Ipv4Addr;

use crate::ifname::IfId;
use crate::sim::World;
use crate::sim::frame::OspfHello;
use crate::sim::fx::Fx;
use crate::sim::node::Node;

#[derive(Clone, Debug, Default)]
pub(crate) struct Ospf {}

pub(crate) fn recompute(_world: &mut World) {}

impl Node {
    pub(crate) fn ospf_sync(&mut self, _fx: &mut Fx) {}
    pub(crate) fn ospf_rx(&mut self, _fx: &mut Fx, _iface: IfId, _src: Ipv4Addr, _hello: &OspfHello) {}
    pub(crate) fn ospf_hello_timer(&mut self, _fx: &mut Fx, _iface: IfId, _token: u32) {}
    pub(crate) fn ospf_neighbor_timer(&mut self, _fx: &mut Fx, _iface: IfId, _router_id: Ipv4Addr, _token: u32) {}
    pub(crate) fn ospf_wait_timer(&mut self, _fx: &mut Fx, _iface: IfId, _token: u32) {}
}
