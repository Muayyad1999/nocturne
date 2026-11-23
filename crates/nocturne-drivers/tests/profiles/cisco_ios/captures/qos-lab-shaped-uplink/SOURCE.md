# qos-lab-shaped-uplink

A shaped uplink with a queueing policy behind the shaper: a priority class, a bandwidth class and a `class-default` that loses what the shaper cannot carry.

`show_policy-map_interface.txt` was printed by **Nocturne's own simulator**
(`nocturne-netsim`), not by a Cisco device, hence `origin: synthetic` in `expect.yml`. The
simulator lays the output out after the samples in Cisco's command reference
(`docs/dev/spec-netsim.md`, "Quality of service", lists which sample each line follows and the
three lines that follow none); it has never been compared with a real device. The lab's
devices are driven with this profile, so what it makes of their output is worth pinning down
— but it proves nothing about IOS, and `qos` stays on the profile's `unverified` list.

The file is the constant `HIERARCHICAL` of `crates/nocturne-netsim/tests/tier3_qos.rs`, byte for byte: ten simulated seconds of voice (0.8 Mbit/s), video (1.5 Mbit/s) and unmarked traffic (4 Mbit/s) through the configuration `SHAPED_UPLINK` of the same file. That test asserts the simulator prints exactly this text.

The expected values in `expect.yml` were written by reading the file.
