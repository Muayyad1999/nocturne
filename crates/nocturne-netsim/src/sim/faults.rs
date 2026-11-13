//! Fault injection: what each `Fault` does to the world and how it is undone.

use crate::api::{Fault, FaultId, LabError, LinkId, NodeId, SimTime};
use crate::ifname::IfId;
use crate::sim::{Event, World};

impl World {
    pub(crate) fn inject(&mut self, fault: Fault) -> Result<FaultId, LabError> {
        match &fault {
            Fault::LinkDown { link }
            | Fault::PacketLoss { link, .. }
            | Fault::Flapping { link, .. }
            | Fault::DuplexMismatch { link } => {
                if !self.links.contains_key(link) {
                    return Err(LabError::UnknownLink(*link));
                }
            }
            Fault::InterfaceErrors {
                node, interface, ..
            } => {
                let n = self.nodes.get(node).ok_or(LabError::UnknownNode(*node))?;
                let known = IfId::parse(interface).is_some_and(|id| n.ifs.contains_key(&id));
                if !known {
                    return Err(LabError::UnknownInterface {
                        node: n.name.clone(),
                        interface: interface.clone(),
                    });
                }
            }
            Fault::HighCpu { node, .. } | Fault::PowerOff { node } => {
                if !self.nodes.contains_key(node) {
                    return Err(LabError::UnknownNode(*node));
                }
            }
        }
        let id = self.next_fault;
        self.next_fault += 1;
        self.faults.insert(id, fault.clone());
        match fault {
            Fault::Flapping { link, .. } => {
                if let Some(l) = self.links.get_mut(&link) {
                    l.flap_down = true;
                }
                self.schedule_flap(id);
                self.refresh_link_faults(link);
            }
            Fault::LinkDown { link }
            | Fault::PacketLoss { link, .. }
            | Fault::DuplexMismatch { link } => {
                self.refresh_link_faults(link);
            }
            Fault::PowerOff { node } => self.set_power(node, false),
            Fault::InterfaceErrors { .. } => {
                self.fault_clock.insert(id, self.now);
            }
            Fault::HighCpu { node, .. } => self.refresh_cpu_fault(node),
        }
        self.touch();
        self.reconcile();
        Ok(id)
    }

    pub(crate) fn clear_fault(&mut self, id: FaultId) {
        self.account_error_faults();
        let Some(fault) = self.faults.remove(&id) else {
            return;
        };
        self.fault_clock.remove(&id);
        match fault {
            Fault::LinkDown { link }
            | Fault::PacketLoss { link, .. }
            | Fault::DuplexMismatch { link }
            | Fault::Flapping { link, .. } => self.refresh_link_faults(link),
            Fault::PowerOff { node } => {
                let still_off = self
                    .faults
                    .values()
                    .any(|f| matches!(f, Fault::PowerOff { node: n } if *n == node));
                if !still_off {
                    self.set_power(node, true);
                }
            }
            Fault::HighCpu { node, .. } => self.refresh_cpu_fault(node),
            Fault::InterfaceErrors { .. } => {}
        }
        self.touch();
        self.reconcile();
    }

    /// The strongest active `HighCpu` fault on a node decides its extra load.
    fn refresh_cpu_fault(&mut self, node: NodeId) {
        let percent = self
            .faults
            .values()
            .filter_map(|f| match f {
                Fault::HighCpu { node: n, percent } if *n == node => Some((*percent).min(100)),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        if let Some(n) = self.nodes.get_mut(&node) {
            n.cpu_fault = percent;
        }
    }

    /// Recompute a link's fault flags from the faults that are still active.
    fn refresh_link_faults(&mut self, link: LinkId) {
        let mut down = false;
        let mut loss = 0u8;
        let mut duplex = false;
        let mut flapping = false;
        for fault in self.faults.values() {
            match fault {
                Fault::LinkDown { link: l } if *l == link => down = true,
                Fault::PacketLoss { link: l, percent } if *l == link => {
                    loss = loss.max((*percent).min(100))
                }
                Fault::DuplexMismatch { link: l } if *l == link => duplex = true,
                Fault::Flapping { link: l, .. } if *l == link => flapping = true,
                _ => {}
            }
        }
        let Some(l) = self.links.get_mut(&link) else {
            return;
        };
        l.fault_down = down;
        l.loss_percent = loss;
        l.duplex_fault = duplex;
        if !flapping {
            l.flap_down = false;
        }
        let (a, b) = (l.a.0, l.b.0);
        self.mark_dirty(a);
        self.mark_dirty(b);
    }

    fn schedule_flap(&mut self, id: FaultId) {
        if let Some(Fault::Flapping { period_s, .. }) = self.faults.get(&id) {
            let half = u64::from((*period_s).max(1)) * 500_000;
            self.schedule(self.now + half, Event::Flap { fault: id });
        }
    }

    /// Bring error counters driven by `InterfaceErrors` faults up to date.
    pub(crate) fn account_error_faults(&mut self) {
        let now = self.now;
        let due: Vec<(FaultId, NodeId, String, u32, SimTime)> = self
            .faults
            .iter()
            .filter_map(|(id, fault)| match fault {
                Fault::InterfaceErrors {
                    node,
                    interface,
                    crc_per_s,
                } => Some((
                    *id,
                    *node,
                    interface.clone(),
                    *crc_per_s,
                    self.fault_clock.get(id).copied().unwrap_or(now),
                )),
                _ => None,
            })
            .collect();
        for (id, node, interface, rate, since) in due {
            // Whole errors only; the remainder of the interval is carried over.
            let elapsed = now.saturating_sub(since);
            let errors = elapsed / 1_000_000 * u64::from(rate)
                + elapsed % 1_000_000 * u64::from(rate) / 1_000_000;
            if errors == 0 {
                continue;
            }
            let used = errors * 1_000_000 / u64::from(rate.max(1));
            self.fault_clock.insert(id, since + used.min(elapsed));
            let state = IfId::parse(&interface)
                .and_then(|i| self.nodes.get_mut(&node).and_then(|n| n.ifs.get_mut(&i)));
            if let Some(state) = state {
                state.counters.crc += errors;
                state.counters.in_errors += errors;
            }
        }
    }
}

/// A flapping link changes state.
pub(crate) fn flap(world: &mut World, fault: FaultId) {
    let Some(Fault::Flapping { link, .. }) = world.faults.get(&fault) else {
        return;
    };
    let link = *link;
    if let Some(l) = world.links.get_mut(&link) {
        l.flap_down = !l.flap_down;
        let (a, b) = (l.a.0, l.b.0);
        world.mark_dirty(a);
        world.mark_dirty(b);
    }
    world.schedule_flap(fault);
}
