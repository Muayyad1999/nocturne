# nocturne-lab-router

`show_version.txt` is the `show version` of a router in Nocturne's own lab, copied from the
sample in `docs/dev/spec-netsim.md` ("show version uses Nocturne's own identity but IOS layout").
It is not output of a vendor's device, hence `synthetic: true`. The lab's devices are driven
with this profile, so what it makes of their identity is worth pinning down: the vendor is read
from the hardware line and is not assumed to be Cisco.
