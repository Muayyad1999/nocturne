# qos-doc-pre-hqf-serial3-1-three-classes

The layout before hierarchical queueing (releases before 12.4(20)T): `Weighted Fair Queueing`
with `(total drops/bytes drops)` for the priority class and `(depth/total drops/no-buffer
drops)` for the bandwidth classes, and a WRED table.

`show_policy-map_interface.txt` is **an excerpt of** a sample printed in Cisco's documentation,
not a recording from a device: *Cisco IOS Quality of Service Solutions Command Reference*,
chapter "show mls qos through wrr-queue threshold", command `show policy-map
interface`, <https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos/command/qos-cr-book/qos-s4.html>
(fetched 2026-11-22, and again for a byte-for-byte comparison on 2026-11-23). It is the first example on the page, the one introduced with

    Router# show policy-map interface serial3/1 output

and the file holds what the page shows beneath that line, byte for byte, **up to and including
the last row of the WRED table**. The four lines that follow on the page — the class
`class-default`, which the page sets at the left margin — are left out, because a class at the
left margin cannot be placed in the policy. `qos-doc-pre-hqf-serial3-1` has the whole sample and
expects exactly that failure. Nothing inside the excerpt was changed.

Hence `origin: documented` in `expect.yml`: see `qos-doc-shaped-llq-serial4-3/SOURCE.md` for
what that does and does not prove. It does **not** count as a capture of real device output.

The expected values in `expect.yml` were written by reading the file.
