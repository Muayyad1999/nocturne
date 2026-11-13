//! CDP and LLDP (placeholder until tier 2).

use crate::ifname::IfId;
use crate::sim::frame::{Advert, Frame};
use crate::sim::fx::Fx;
use crate::sim::node::Node;

#[derive(Clone, Debug, Default)]
pub(crate) struct Neighbors {}

impl Node {
    pub(crate) fn advert_rx(&mut self, _fx: &mut Fx, _port: IfId, _advert: &Advert, _frame: &Frame) {}
    pub(crate) fn neighbors_sync(&mut self, _fx: &mut Fx, _came_up: &[IfId]) {}
    pub(crate) fn advert_timer(&mut self, _fx: &mut Fx) {}
}
