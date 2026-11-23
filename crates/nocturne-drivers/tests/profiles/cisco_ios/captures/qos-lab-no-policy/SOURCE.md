# qos-lab-no-policy

A device with no policy attached to any interface.

`show_policy-map_interface.txt` was printed by **Nocturne's own simulator**
(`nocturne-netsim`), not by a Cisco device, hence `origin: synthetic` in `expect.yml`. The
simulator lays the output out after the samples in Cisco's command reference
(`docs/dev/spec-netsim.md`, "Quality of service", lists which sample each line follows and the
three lines that follow none); it has never been compared with a real device. The lab's
devices are driven with this profile, so what it makes of their output is worth pinning down
— but it proves nothing about IOS, and `qos` stays on the profile's `unverified` list.

The file is empty, because that is what the simulator answers (`crates/nocturne-netsim/tests/tier3_qos.rs`, `a_device_with_no_policy_answers_show_policy_map_interface_with_nothing`), following what `docs/dev/spec-qos.md` says of hardware. Collected, this must be a table of zero rows — a read that worked and found nothing — and not a failure.

The expected values in `expect.yml` were written by reading the file.
