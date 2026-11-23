# qos-lab-policed-input

An input policy with a policer that drops what exceeds 2 Mbit/s, and a `class-default` nothing falls into.

`show_policy-map_interface.txt` was printed by **Nocturne's own simulator**
(`nocturne-netsim`), not by a Cisco device, hence `origin: synthetic` in `expect.yml`. The
simulator lays the output out after the samples in Cisco's command reference
(`docs/dev/spec-netsim.md`, "Quality of service", lists which sample each line follows and the
three lines that follow none); it has never been compared with a real device. The lab's
devices are driven with this profile, so what it makes of their output is worth pinning down
— but it proves nothing about IOS, and `qos` stays on the profile's `unverified` list.

The file is the constant `FLAT` of `crates/nocturne-netsim/tests/tier3_qos.rs`, byte for byte: ten simulated seconds of a 5 Mbit/s flow through the configuration `POLICED_USERS` of the same file. That test asserts the simulator prints exactly this text.

The expected values in `expect.yml` were written by reading the file.
