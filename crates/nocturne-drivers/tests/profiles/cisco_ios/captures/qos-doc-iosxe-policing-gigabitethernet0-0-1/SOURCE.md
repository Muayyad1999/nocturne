# qos-doc-iosxe-policing-gigabitethernet0-0-1

A marking policy on the way in and a policer on the way out, as an IOS XE guide shows them
(the page is filed under the ASR 1000): rates printed with leading zeros (`0000 bps`), a three-colour policer
whose exceeding packets are re-marked (`set-qos-transmit 1`) and whose violating ones are
dropped, and `Marker statistics: Disabled` where other releases count the marked packets.

`show_policy-map_interface.txt` is a sample printed in Cisco's documentation, not a recording
from a device: *QoS: Policing and Shaping Configuration Guide* (the Cisco IOS XE 16 edition,
which the page files under the Cisco ASR 1000 Series Aggregation Services Routers), chapter
"Class-Based Policing", section "Verifying Class-Based Traffic Policing",
<https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos_plcshp/configuration/xe-16/qos-plcshp-xe-16-book/qos-plcshp-class-plc.html>
(fetched 2026-11-23). It is the example the page introduces with

    Router# show policy-map interface gigabitethernet 0/0/1

and the file holds what the page shows beneath that line, byte for byte, as far as the end of
the example, with a final line ending added.

Hence `origin: documented` in `expect.yml`: the sample shows the format Cisco describes, but it
has been through an editor and a typesetter and the page does not say which release printed
it. It does **not** count as a capture of real device output, and `qos` stays on the profile's
`unverified` list until one exists (`../SOURCE.md` says how to record one).

The expected values in `expect.yml` were written by reading the file.
