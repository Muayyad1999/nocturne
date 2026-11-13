//! DHCP (placeholder until tier 2).

use crate::ifname::IfId;
use crate::sim::frame::{DhcpMsg, Meta, Packet};
use crate::sim::fx::Fx;
use crate::sim::node::Node;
use crate::util::mac::Mac;
use crate::util::net::IfAddr;

#[derive(Clone, Debug)]
pub(crate) struct Lease {
    pub(crate) addr: IfAddr,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DhcpClient {
    pub(crate) lease: Option<Lease>,
}

impl DhcpClient {
    pub(crate) fn release(&mut self) {
        self.lease = None;
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DhcpServer {}

impl Node {
    pub(crate) fn dhcp_client_sync(&mut self, _fx: &mut Fx) {}
    pub(crate) fn dhcp_client_timer(&mut self, _fx: &mut Fx, _iface: IfId, _token: u32) {}
    pub(crate) fn dhcp_server_sync(&mut self) {}
    pub(crate) fn dhcp_rx(&mut self, _fx: &mut Fx, _iface: IfId, _packet: &Packet, _msg: &DhcpMsg, _meta: &Meta, _l2: (Mac, Mac)) {}
}
