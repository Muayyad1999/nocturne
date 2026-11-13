//! The property the whole pipeline exists for: for two configurations A and
//! B, planning `Intended { B, exact }` on a device that has A and applying it
//! leaves the device with B — and undoing it leaves the device with A again.
//!
//! The configurations are small but randomly assembled from the things that
//! go wrong on real switches: VLANs that come and go, ports that change mode,
//! trunk allowed-VLAN lists that grow and shrink, static routes, NTP and
//! logging servers.

mod common;

use common::*;
use nocturne_drivers::hier::{self, Syntax};
use serde_json::json;

/// A tiny deterministic generator: the seed is printed with any failure, so a
/// failing case can be replayed exactly.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, limit: u64) -> u64 {
        self.next() % limit.max(1)
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn pick<'a, T>(&mut self, from: &'a [T]) -> &'a T {
        &from[self.below(from.len() as u64) as usize]
    }
}

/// A small IOS configuration: a management SVI that never moves, a few
/// VLANs, three ports and a handful of global lines.
///
/// `snmp-server location` and the other global commands where setting a new
/// value *replaces* the old one are left out on purpose: the fake device in
/// `nocturne_app::testing` keeps both lines where a real IOS keeps one, so a
/// configuration containing them would be testing the fake, not the pipeline
/// (which correctly spots the difference and rolls the change back).
fn configuration(rng: &mut Rng) -> String {
    let mut out = String::from("hostname SW1\n");

    let mut vlans: Vec<u16> = vec![10];
    for vlan in [20u16, 30, 40, 50] {
        if rng.chance(60) {
            vlans.push(vlan);
        }
    }
    for vlan in &vlans {
        out.push_str(&format!("vlan {vlan}\n"));
        out.push_str(&format!(" name {}\n", rng.pick(&["users", "voice", "lab"])));
    }

    for port in 1..=3 {
        out.push_str(&format!("interface GigabitEthernet0/{port}\n"));
        if rng.chance(70) {
            out.push_str(&format!(
                " description {}\n",
                rng.pick(&["uplink", "desk 12", "printer", "ap"])
            ));
        }
        if rng.chance(45) {
            out.push_str(" switchport mode trunk\n");
            if rng.chance(75) {
                let allowed: Vec<String> = vlans
                    .iter()
                    .filter(|_| rng.chance(70))
                    .map(u16::to_string)
                    .collect();
                if !allowed.is_empty() {
                    out.push_str(&format!(
                        " switchport trunk allowed vlan {}\n",
                        allowed.join(",")
                    ));
                }
            }
        } else {
            out.push_str(" switchport mode access\n");
            out.push_str(&format!(" switchport access vlan {}\n", rng.pick(&vlans)));
            if rng.chance(40) {
                out.push_str(" spanning-tree portfast\n");
            }
        }
    }

    out.push_str("interface Vlan1\n ip address 10.0.0.2 255.255.255.0\n");
    out.push_str("ip default-gateway 10.0.0.1\n");
    for server in 1..=3u8 {
        if rng.chance(45) {
            out.push_str(&format!("ntp server 192.0.2.{server}\n"));
        }
    }
    if rng.chance(50) {
        out.push_str(&format!(
            "ip route 10.{}.0.0 255.255.255.0 10.0.0.1\n",
            rng.below(4) + 1
        ));
    }
    if rng.chance(40) {
        out.push_str(&format!("logging host 10.0.0.{}\n", rng.below(5) + 20));
    }
    out.push_str("line vty 0 4\n transport input ssh\n");
    out
}

/// "Nothing left to do to turn `actual` into `wanted`" — the engine's own
/// definition of two configurations being the same.
fn equals(actual: &str, wanted: &str) -> Result<(), Vec<String>> {
    let remaining = hier::remediate(actual, wanted, Syntax::Ios);
    if remaining.commands.is_empty() {
        Ok(())
    } else {
        Err(remaining.commands)
    }
}

#[tokio::test]
async fn planning_an_intended_configuration_reaches_it_and_rolling_back_undoes_it() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let mut rng = Rng(0x5eed_1234_abcd_0001);

    for round in 0..12u32 {
        let seed = rng.0;
        let a = configuration(&mut rng);
        let b = configuration(&mut rng);
        let context = format!("round {round}, seed {seed:#x}\n--- A ---\n{a}\n--- B ---\n{b}");

        load(&lab, "SW1", &a);
        let change = planned(
            app,
            &lab.id("SW1"),
            json!({"type": "intended", "config": b, "exact": true}),
            json!({"guard_minutes": 0}),
        )
        .await;

        // The plan's own rollback must put the prediction back where it started.
        let predicted = {
            let (tree, report) = nocturne_changes::plan::predict(
                &a,
                &commands(&change)
                    .iter()
                    .map(String::to_string)
                    .collect::<Vec<_>>(),
                Syntax::Ios,
            );
            assert!(
                report.supported && report.unmodelled.is_empty(),
                "{context}"
            );
            tree.render()
        };
        equals(&predicted, &b).unwrap_or_else(|left| {
            panic!("the prediction is not B ({left:?})\n{context}");
        });
        let (undone, _) =
            nocturne_changes::plan::predict(&predicted, &rollback(&change), Syntax::Ios);
        equals(&undone.render(), &a).unwrap_or_else(|left| {
            panic!("the plan's rollback does not restore A ({left:?})\n{context}");
        });

        // Now do it for real.
        let applied = apply(app, &change).await;
        assert_eq!(
            target_state(&applied),
            "applied",
            "{:?}\n{context}",
            applied["targets"][0]["result"]["error"]
        );
        let on_device = lab.fake("SW1").running_config();
        equals(&on_device, &b).unwrap_or_else(|left| {
            panic!("the device is not B after the change ({left:?})\n{context}\n--- device ---\n{on_device}");
        });

        // …and undo it.
        run_job(app, "changes.rollback", json!({"id": change["id"]})).await;
        let rolled_back = app
            .rpc("changes.get", json!({"id": change["id"]}))
            .await
            .unwrap();
        assert_eq!(target_state(&rolled_back), "rolled_back", "{context}");
        let on_device = lab.fake("SW1").running_config();
        equals(&on_device, &a).unwrap_or_else(|left| {
            panic!("the device is not A after the rollback ({left:?})\n{context}\n--- device ---\n{on_device}");
        });
    }
}
