# qos-doc-frame-relay-dlci

A policy attached to a Frame Relay DLCI: the line that names the interface goes on to name the circuit (`Serial3/1.1:DLCI 201 -`).

`show_policy-map_interface.txt` is a sample printed in Cisco's documentation, not a recording
from a device: *Cisco IOS Quality of Service Solutions Command Reference*, chapter "show mls
qos through wrr-queue threshold", command `show policy-map interface`,
<https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos/command/qos-cr-book/qos-s4.html>
(fetched 2026-11-22, and again for a byte-for-byte comparison on 2026-11-23). It is the example the page introduces with

    Router# show policy interface Serial3/1.1

and the file holds what the page shows beneath that line, byte for byte, as far as the end of
the example. 

Hence `origin: documented` in `expect.yml`: the sample shows the format Cisco describes, but it
has been through an editor and a typesetter, the page does not say which release or platform
printed it, and other samples on the same page have visibly lost their indentation. It does
**not** count as a capture of real device output, and `qos` stays on the profile's `unverified`
list until one exists (`../SOURCE.md` says how to record one).

**This case expects the read to fail.** A row has an interface and no field for a circuit
beneath it, so the classes of two DLCIs on one interface would be told apart by nothing; the
profile stops with an error that says so instead of reading them as one.

The expected values in `expect.yml` were written by reading the file.
