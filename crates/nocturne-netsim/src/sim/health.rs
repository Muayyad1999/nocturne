//! Simulated device health: CPU, memory and temperature figures that react to
//! traffic and faults, with seeded noise that does not depend on call order.

use crate::api::SimTime;
use crate::catalog::Role;
use crate::sim::node::Node;
use crate::util::rng;

impl Node {
    /// CPU load in percent: a little noise, a term for forwarded traffic, and
    /// whatever a `HighCpu` fault adds.
    pub(crate) fn cpu_percent(&self, at: SimTime, seed: u64) -> f32 {
        // Noise changes every five seconds.
        let bucket = at / 5_000_000;
        let noise = (rng::hash(&[seed, u64::from(self.id), bucket]) % 400) as f32 / 100.0;
        let load_bps: f64 = self.ifs.values().map(|s| s.load_in).sum();
        // Forwarding 100 Mbit/s costs about ten percent.
        let traffic = (load_bps / 10_000_000.0) as f32;
        (2.0 + noise + traffic.min(60.0) + f32::from(self.cpu_fault)).clamp(0.0, 100.0)
    }

    /// (total, used) memory in bytes; grows slowly with the size of the tables.
    pub(crate) fn memory(&self) -> (u64, u64) {
        let total: u64 = match self.role() {
            Role::Router | Role::Cloud => 512 << 20,
            Role::L3Switch => 512 << 20,
            Role::L2Switch | Role::Bridge => 256 << 20,
            Role::Host => 4096 << 20,
        };
        let tables = (self.rib.routes.len() + self.arp.rows(u64::MAX / 2).len()) as u64 * 2048
            + self.log.entries.len() as u64 * 256;
        (total, total * 3 / 10 + tables)
    }

    pub(crate) fn temperature(&self, at: SimTime, seed: u64) -> f32 {
        let noise =
            (rng::hash(&[seed, u64::from(self.id), at / 30_000_000, 7]) % 100) as f32 / 100.0;
        36.0 + noise + self.cpu_percent(at, seed) * 0.12
    }
}
