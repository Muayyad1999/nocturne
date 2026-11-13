# netns-bridge-router

A real Linux kernel, printing real interfaces, addresses, routes, neighbours and bridge
forwarding entries. The ntc-templates corpus has captures of the *text* output of
`ip address show`, `ip link show`, `ip route show` and `arp -a`, but none of the JSON this
profile asks for, so the output here was **recorded rather than copied**: on the machine this
profile was written on (Ubuntu 24.04, kernel 6.18, iproute2 6.1.0), in a network namespace
built for the purpose, and saved verbatim.

The namespace was built with these commands, which is what the captures describe:

```
ip netns add nocturne-lab
ip netns exec nocturne-lab bash
  ip link set lo up
  ip link add br0 type bridge
  ip link add eth0 type veth peer name peer0      # and eth1/peer1, eth2/peer2
  ip link set eth0 master br0 ; ip link set eth1 master br0
  ip link set eth0 up ; ip link set eth1 up ; ip link set peer0 up ; ip link set peer1 up
  ip link set br0 up ; ip link set br0 mtu 9000
  ip link set eth2 down                            # a port that is administratively down
  ip link set eth1 alias "uplink to core-sw1"      # an interface description
  ip addr add 192.0.2.2/24 brd + dev br0
  ip addr add 192.0.2.3/24 dev br0                 # a secondary address
  ip addr add 10.100.0.1/30 dev eth2               # an address on a down interface
  ip route add default via 192.0.2.1 dev br0
  ip route add 10.0.0.0/8 via 192.0.2.254 dev br0 metric 100
  ip route add 172.16.0.0/16 dev br0 scope link    # a route with no gateway
  ip neigh add 192.0.2.1  lladdr aa:bb:cc:00:00:01 dev br0 nud permanent
  ip neigh add 192.0.2.50 lladdr aa:bb:cc:00:00:02 dev br0 nud stale
  ip neigh add 192.0.2.77 dev br0 nud incomplete   # an address that never answered
  bridge fdb add aa:bb:cc:00:00:11 dev eth0 master static
  bridge fdb add aa:bb:cc:00:00:12 dev eth1 master static
```

| file | recorded with |
|---|---|
| `ip_-json_-details_-statistics_address_show.txt` | `ip netns exec nocturne-lab ip -json -details -statistics address show` |
| `ip_-json_route_show.txt` | `ip netns exec nocturne-lab ip -json route show` |
| `ip_-json_neighbour_show.txt` | `ip netns exec nocturne-lab ip -json neighbour show` |
| `bridge_-json_fdb_show.txt` | `ip netns exec nocturne-lab bridge -json fdb show` |

The addresses are the documentation ranges of RFC 5737 and RFC 1918; the MAC addresses the
kernel generated are random, and the ones added by hand are in the `aa:bb:cc` range.

**What this capture cannot show:** the kernel of that machine has no IPv6 and no `dummy`,
`vlan` or `bond` modules, so there is no IPv6 address, no VLAN interface and no bond here.
The IPv6 mapping is the same expression as the IPv4 one with `inet6` in place of `inet`, but
it is not covered by a test — see `docs/profiles.md` and the profile's own comments.

The expected values in `expect.yml` were written by reading these files.
