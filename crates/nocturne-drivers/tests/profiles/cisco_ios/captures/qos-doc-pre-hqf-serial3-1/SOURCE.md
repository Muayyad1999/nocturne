# qos-doc-pre-hqf-serial3-1

The first example on the page — the layout before hierarchical queueing (`Weighted Fair Queueing`, `(total drops/bytes drops)`) — exactly as the page prints it, which is with the WRED table and the last class, `class-default`, set at the left margin.

`show_policy-map_interface.txt` is a sample printed in Cisco's documentation, not a recording
from a device: *Cisco IOS Quality of Service Solutions Command Reference*, chapter "show mls
qos through wrr-queue threshold", command `show policy-map interface`,
<https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos/command/qos-cr-book/qos-s4.html>
(fetched 2026-11-22, and again for a byte-for-byte comparison on 2026-11-23). It is the example the page introduces with

    Router# show policy-map interface serial3/1 output

and the file holds what the page shows beneath that line, byte for byte, as far as the end of
the example. 

Hence `origin: documented` in `expect.yml`: the sample shows the format Cisco describes, but it
has been through an editor and a typesetter, the page does not say which release or platform
printed it, and other samples on the same page have visibly lost their indentation. It does
**not** count as a capture of real device output, and `qos` stays on the profile's `unverified`
list until one exists (`../SOURCE.md` says how to record one).

**This case expects the read to fail.** Where a class stands in a policy is told by its
indentation and by nothing else, so a class at the left margin cannot be placed; the profile
stops with an error rather than attach its counters to whatever came before. Whether a device
ever printed the sample this way or the page lost the columns cannot be told from the page.
`qos-doc-pre-hqf-serial3-1-three-classes` reads the part of the same sample that is intact.

The expected values in `expect.yml` were written by reading the file.
