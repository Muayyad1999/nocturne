//! HSRP (placeholder until tier 3).

use std::net::Ipv4Addr;

use crate::ifname::IfId;
use crate::sim::frame::HsrpMsg;
use crate::sim::fx::Fx;
use crate::sim::node::Node;
use crate::util::mac::Mac;

#[derive(Clone, Debug, Default)]
pub(crate) struct Hsrp {}

impl Node {
    pub(crate) fn hsrp_sync(&mut self, _fx: &mut Fx) {}
    pub(crate) fn hsrp_rx(&mut self, _fx: &mut Fx, _iface: IfId, _src: Ipv4Addr, _msg: HsrpMsg) {}
    pub(crate) fn hsrp_timer(&mut self, _fx: &mut Fx, _iface: IfId, _group: u16, _token: u32) {}
    pub(crate) fn hsrp_owns_mac(&self, _iface: IfId, _mac: Mac) -> bool {
        false
    }
    pub(crate) fn hsrp_owns_ip(&self, _ip: Ipv4Addr) -> bool {
        false
    }
    pub(crate) fn hsrp_virtual_mac(&self, _iface: IfId, _ip: Ipv4Addr) -> Option<Mac> {
        None
    }
}
