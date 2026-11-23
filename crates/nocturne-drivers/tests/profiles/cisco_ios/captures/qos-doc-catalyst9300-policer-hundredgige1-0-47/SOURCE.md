# qos-doc-catalyst9300-policer-hundredgige1-0-47

An output policer as the Catalyst 9300's guide shows it: the class counts packets and no
bytes, the policer is configured with `police rate` and so prints `rate … bps, burst … bytes`
instead of `cir`, and it counts bytes and no packets. The counters are well beyond 32 bits.

`show_policy-map_interface.txt` is a sample printed in Cisco's documentation, not a recording
from a device: *Quality of Service Configuration Guide, Cisco IOS XE Amsterdam 17.3.x (Catalyst
9300 Switches)*, chapter "Configuring QoS", section "Time-to-Live Classification",
<https://www.cisco.com/c/en/us/td/docs/switches/lan/catalyst9300/software/release/17-3/configuration_guide/qos/b_173_qos_9300_cg/configuring_qos.html>
(fetched 2026-11-23). It is the last part of the example in that section, the one introduced
with

    Device#show policy-map interface hun1/0/47

and the file holds what the page shows beneath that line, byte for byte, as far as the end of
the example.

**The page prints every line of a class in the same column as its `Class-map:` line**, and the
interface at the left margin. Whether a switch prints it so or the page lost the columns
cannot be told from the page (other samples on it have tabs where a device has spaces). The
profile reads the lines of a class whatever column they stand in, so it makes no difference
here.

Hence `origin: documented` in `expect.yml`: the sample shows the format Cisco describes, but it
has been through an editor and a typesetter, and the page does not say which switch or release
printed it (the guide is the one for IOS XE 17.3). It does **not** count as a capture of real
device output, and `qos` stays on the profile's `unverified` list until one exists
(`../SOURCE.md` says how to record one).

The expected values in `expect.yml` were written by reading the file.
