# qos-lab-campus-edge

The edge router of Nocturne's example campus lab: a shaped uplink with a queueing policy
inside the shaper (voice first, a scavenger class that loses in the queue), and one input
policy attached to two interfaces, whose class polices and re-marks the guest network.

`show_policy-map_interface.txt` was printed by **Nocturne's own simulator**
(`nocturne-netsim`), not by a Cisco device, hence `origin: synthetic` in `expect.yml`. The
simulator lays the output out after the samples in Cisco's command reference
(`docs/dev/spec-netsim.md`, "Quality of service"); it has never been compared with a real
device. It proves nothing about IOS, and `qos` stays on the profile's `unverified` list.

It is what the console of the node `EDGE` answered to `show policy-map interface` in a run of
the lab example `demo` on 2026-11-22, read through the same CLI session Nocturne collects
with (`connect::cli` in `crates/nocturne-lab/tests/qos_collect.rs`, with a line added for the
occasion that printed the answer) and saved as it came, with a final line ending added. The
counters are those of that moment; another run stops at other numbers.

The expected values in `expect.yml` were written by reading the file.
