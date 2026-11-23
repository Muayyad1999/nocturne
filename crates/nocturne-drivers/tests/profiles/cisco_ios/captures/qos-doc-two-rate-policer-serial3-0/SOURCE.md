# qos-doc-two-rate-policer-serial3-0

A two-rate policer under load, in the older layout that names the action on the counter's own line (`action: drop`). Only the violating packets are dropped; the exceeding ones are re-marked and sent.

`show_policy-map_interface.txt` is a sample printed in Cisco's documentation, not a recording
from a device: *Cisco IOS Quality of Service Solutions Command Reference*, chapter "show mls
qos through wrr-queue threshold", command `show policy-map interface`,
<https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos/command/qos-cr-book/qos-s4.html>
(fetched 2026-11-22, and again for a byte-for-byte comparison on 2026-11-23). It is the example the page introduces with

    Router# show policy-map interface serial3/0

and the file holds what the page shows beneath that line, byte for byte, as far as the end of
the example. 

Hence `origin: documented` in `expect.yml`: the sample shows the format Cisco describes, but it
has been through an editor and a typesetter, the page does not say which release or platform
printed it, and other samples on the same page have visibly lost their indentation. It does
**not** count as a capture of real device output, and `qos` stays on the profile's `unverified`
list until one exists (`../SOURCE.md` says how to record one).

The page sets the classes at three columns and writes `(match all)` without the hyphen; the
file keeps both as printed.

The expected values in `expect.yml` were written by reading the file.
