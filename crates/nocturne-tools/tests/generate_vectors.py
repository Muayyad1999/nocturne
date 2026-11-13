#!/usr/bin/env python3
"""Generate cross-check vectors for nocturne-tools from independent implementations.

The Rust test `tests/cross_check.rs` compares the crate against the output
of this script:

* `subnet` and `vlsm` against Python's `ipaddress` module;
* Cisco type 7 against `passlib.hash.cisco_type7` (skipped when passlib is
  not installed; `pip install passlib`);
* the password generator against a second implementation of its documented
  algorithm on top of `hashlib`.

A copy of the output is vendored as `tests/data/vectors.json.gz` so that the
test suite needs no Python:

    python3 tests/generate_vectors.py --out tests/data/vectors.json.gz

The output is deterministic for a given seed and count (and Python version:
the vendored file was made with CPython 3.13 and passlib 1.7.4).
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import ipaddress
import itertools
import json
import random
import sys

try:
    from passlib.hash import cisco_type7
except ImportError:  # pragma: no cover - optional
    cisco_type7 = None

V4_BITS, V6_BITS = 32, 128


# ----------------------------------------------------------------- formatting

def show(address) -> str:
    """Text form used by the crate: RFC 5952, IPv4-mapped addresses with a dotted tail."""
    if address.version == 6 and address.ipv4_mapped is not None:
        return "::ffff:" + str(address.ipv4_mapped)
    return address.compressed


def show_net(network) -> str:
    return f"{show(network.network_address)}/{network.prefixlen}"


def expanded(address) -> str:
    if address.version == 4:
        return str(address)
    digits = f"{int(address):032x}"
    return ":".join(digits[i : i + 4] for i in range(0, 32, 4))


def nets(names):
    return [ipaddress.ip_network(name) for name in names]


V4_SCOPES = [
    (["0.0.0.0/32"], "unspecified"),
    (["255.255.255.255/32"], "limited broadcast"),
    (["0.0.0.0/8"], "this network (RFC 791)"),
    (["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"], "private (RFC 1918)"),
    (["100.64.0.0/10"], "CGNAT (RFC 6598)"),
    (["127.0.0.0/8"], "loopback"),
    (["169.254.0.0/16"], "link-local"),
    (["192.0.0.0/24"], "IETF protocol assignments (RFC 6890)"),
    (["192.0.2.0/24", "198.51.100.0/24", "203.0.113.0/24"], "documentation"),
    (["192.88.99.0/24"], "6to4 relay anycast (deprecated)"),
    (["198.18.0.0/15"], "benchmarking (RFC 2544)"),
    (["224.0.0.0/4"], "multicast"),
    (["240.0.0.0/4"], "reserved"),
]
V6_SCOPES = [
    (["::/128"], "unspecified"),
    (["::1/128"], "loopback"),
    (["::ffff:0:0/96"], "IPv4-mapped"),
    (["64:ff9b::/96"], "NAT64 (RFC 6052)"),
    (["64:ff9b:1::/48"], "local-use NAT64 (RFC 8215)"),
    (["100::/64"], "discard-only (RFC 6666)"),
    (["2001::/32"], "Teredo"),
    (["2001:2::/48"], "benchmarking (RFC 5180)"),
    (["2001:10::/28"], "ORCHID (deprecated)"),
    (["2001:20::/28"], "ORCHIDv2 (RFC 7343)"),
    (["2001:db8::/32", "3fff::/20"], "documentation"),
    (["2001::/23"], "IETF protocol assignments (RFC 2928)"),
    (["2002::/16"], "6to4"),
    (["5f00::/16"], "SRv6 SIDs (RFC 9602)"),
    (["fc00::/7"], "unique local"),
    (["fe80::/10"], "link-local"),
    (["fec0::/10"], "site-local (deprecated)"),
    (["ff00::/8"], "multicast"),
    (["2000::/3"], "global unicast"),
]


def scope(address) -> str:
    table, default = (V4_SCOPES, "public") if address.version == 4 else (V6_SCOPES, "reserved")
    for blocks, name in table:
        if any(address in block for block in nets(blocks)):
            return name
    return default


def describe(interface) -> dict:
    """What `subnet::info` should return for a valid interface."""
    address, network = interface.ip, interface.network
    bits = address.max_prefixlen
    host_bits = bits - network.prefixlen
    first, last = network.network_address, network.broadcast_address
    if host_bits == 0:
        first_host = last_host = address
        usable = 1
    elif host_bits == 1:
        first_host, last_host, usable = first, last, 2
    elif address.version == 4:
        first_host, last_host, usable = first + 1, last - 1, network.num_addresses - 2
    else:
        first_host, last_host, usable = first + 1, last, network.num_addresses - 1
    info = {
        "version": address.version,
        "address": show(address),
        "network": show(first),
        "prefix": network.prefixlen,
        "cidr": show_net(network),
        "netmask": "",
        "wildcard": "",
        "broadcast": "",
        "first_host": show(first_host),
        "last_host": show(last_host),
        "usable_hosts": str(usable),
        "total_addresses": str(network.num_addresses),
        "class": "",
        "scope": scope(address),
        "binary_address": "",
        "binary_netmask": "",
        "hex": "",
        "integer": str(int(address)),
        "reverse_dns": address.reverse_pointer,
        "expanded": expanded(address),
        "compressed": show(address),
        "is_network_address": host_bits >= 2 and address == first,
        "is_broadcast_address": address.version == 4 and host_bits >= 2 and address == last,
    }
    if address.version == 4:
        octets = address.packed
        info.update(
            netmask=str(network.netmask),
            wildcard=str(network.hostmask),
            broadcast=str(last) if host_bits >= 2 else "",
            binary_address=".".join(f"{octet:08b}" for octet in octets),
            binary_netmask=".".join(f"{octet:08b}" for octet in network.netmask.packed),
            hex=f"0x{int(address):08X}",
        )
        info["class"] = "A" if octets[0] < 128 else "B" if octets[0] < 192 else "C" if octets[0] < 224 else "D" if octets[0] < 240 else "E"
    else:
        info["hex"] = f"0x{int(address):032X}"
    return info


# ------------------------------------------------------------ random inputs

INTERESTING_V4 = [
    "0.0.0.0", "0.1.2.3", "10.0.0.0", "10.255.255.255", "100.64.0.1", "127.0.0.1", "169.254.1.1", "172.16.0.1",
    "172.31.255.255", "172.32.0.0", "192.0.0.9", "192.0.2.1", "192.88.99.1", "192.168.1.10", "198.18.0.1",
    "198.51.100.1", "203.0.113.1", "224.0.0.5", "239.255.255.250", "240.0.0.1", "255.255.255.254", "255.255.255.255",
    "8.8.8.8", "1.1.1.1", "128.0.0.0", "191.255.255.255", "223.255.255.255",
]
INTERESTING_V6 = [
    "::", "::1", "::2", "::ffff:192.168.1.1", "::ffff:0:1", "64:ff9b::c000:221", "64:ff9b:1::1", "100::1", "2001::1",
    "2001:2::1", "2001:10::1", "2001:20::1", "2001:1::1", "2001:db8::1", "2001:db8:0:1:1:1:1:1", "2001:db8::1:0:0:1",
    "2001:0:0:1::1", "3fff::1", "3fff:1000::1", "2002:c000:204::1", "2606:4700:4700::1111", "5f00::1", "4000::1",
    "fc00::1", "fd12:3456:789a::1", "fe80::1", "febf::ffff", "fec0::1", "ff02::1", "ff05::1:3",
    "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", "1::", "1:2:3:4:5:6:7:8", "0:0:1::", "1:0:0:2:0:0:0:3",
]


class Gen:
    def __init__(self, seed: int):
        self.rng = random.Random(seed)

    def v4_address(self) -> ipaddress.IPv4Address:
        if self.rng.random() < 0.25:
            return ipaddress.IPv4Address(self.rng.choice(INTERESTING_V4))
        return ipaddress.IPv4Address(self.rng.getrandbits(32))

    def v6_address(self) -> ipaddress.IPv6Address:
        roll = self.rng.random()
        if roll < 0.25:
            return ipaddress.IPv6Address(self.rng.choice(INTERESTING_V6))
        if roll < 0.6:
            # Sparse addresses, so that zero runs of every shape occur.
            groups = [self.rng.choice([0, 0, 0, 1, self.rng.getrandbits(16)]) for _ in range(8)]
            return ipaddress.IPv6Address(sum(group << (16 * (7 - index)) for index, group in enumerate(groups)))
        return ipaddress.IPv6Address(self.rng.getrandbits(128))

    def prefix(self, bits: int) -> int:
        roll = self.rng.random()
        if roll < 0.3:
            return self.rng.choice([0, 1, bits - 2, bits - 1, bits, bits // 2, 8, 16, 24])
        return self.rng.randint(0, bits)

    def v4_network(self, min_prefix=0, max_prefix=32, strict=None) -> str:
        prefix = self.rng.randint(min_prefix, max_prefix)
        address = self.v4_address()
        if strict if strict is not None else self.rng.random() < 0.6:
            address = ipaddress.ip_network((int(address), prefix), strict=False).network_address
        return f"{address}/{prefix}" if prefix < 32 or self.rng.random() < 0.5 else str(address)

    def v6_network(self, min_prefix=0, max_prefix=128) -> str:
        prefix = self.rng.randint(min_prefix, max_prefix)
        address = self.v6_address()
        if self.rng.random() < 0.6:
            address = ipaddress.ip_network((int(address), prefix), strict=False).network_address
        return f"{show(address)}/{prefix}" if prefix < 128 or self.rng.random() < 0.5 else show(address)

    def clustered_v4(self, count: int, base_prefix: int = 16):
        """Networks close together, so that they overlap, nest and touch."""
        base = int(self.v4_address()) & ~((1 << (32 - base_prefix)) - 1)
        out = []
        for _ in range(count):
            prefix = self.rng.randint(base_prefix, 32)
            offset = self.rng.getrandbits(32 - base_prefix) if base_prefix < 32 else 0
            address = ipaddress.IPv4Address(base | offset)
            if self.rng.random() < 0.7:
                address = ipaddress.ip_network((int(address), prefix), strict=False).network_address
            out.append(f"{address}/{prefix}" if prefix < 32 or self.rng.random() < 0.5 else str(address))
        return out

    def clustered_v6(self, count: int, base_prefix: int = 48):
        base = int(self.v6_address()) & ~((1 << (128 - base_prefix)) - 1)
        out = []
        for _ in range(count):
            prefix = self.rng.randint(base_prefix, min(base_prefix + 24, 128))
            offset = self.rng.getrandbits(24) << (128 - base_prefix - 24)
            address = ipaddress.ip_network((base | offset, prefix), strict=False).network_address
            out.append(f"{show(address)}/{prefix}")
        return out


def loose(text: str):
    """Parse a network the way the crate does: host bits allowed."""
    return ipaddress.ip_network(text, strict=False)


# ------------------------------------------------------------------ sections

def info_vectors(gen: Gen, count: int) -> list:
    out = []
    for index in range(count):
        if index % 3 == 2:
            address = gen.v6_address()
            prefix = gen.prefix(V6_BITS)
            spelling = gen.rng.choice(["compressed", "compressed", "exploded", "upper"])
            text = {"compressed": show(address), "exploded": address.exploded, "upper": expanded(address).upper()}[spelling]
            form = gen.rng.choice(["slash", "slash", "bare", "space"])
            if form == "bare":
                given, python = text, f"{address}/128"
            elif form == "space":
                given, python = f"{text} {prefix}", f"{address}/{prefix}"
            else:
                given, python = f"{text}/{prefix}", f"{address}/{prefix}"
        else:
            address = gen.v4_address()
            prefix = gen.prefix(V4_BITS)
            network = ipaddress.ip_network((int(address), prefix), strict=False)
            python = f"{address}/{prefix}"
            form = gen.rng.choice(["slash", "slash", "slash", "bare", "netmask", "slash-netmask", "wildcard", "space"])
            # 0.0.0.0 and 255.255.255.255 are netmasks, so /32 and /0 cannot be written as wildcards.
            if form == "wildcard" and prefix in (0, 32):
                form = "netmask"
            if form == "bare":
                given, python = str(address), f"{address}/32"
            elif form == "netmask":
                given = f"{address} {network.netmask}"
            elif form == "slash-netmask":
                given = f"{address}/{network.netmask}"
            elif form == "wildcard":
                given = f"{address} {network.hostmask}"
            elif form == "space":
                given = f"{address} {prefix}"
            else:
                given = python
        out.append({"input": given, "expect": describe(ipaddress.ip_interface(python))})
    return out


def validity_vectors(gen: Gen, count: int) -> list:
    """Mutated inputs; only whether they are accepted is compared."""
    alphabet = "0123456789abcdefABCDEF:./xg-"
    out = []
    for _ in range(count):
        if gen.rng.random() < 0.5:
            text = f"{gen.v4_address()}/{gen.prefix(V4_BITS)}"
        else:
            address = gen.v6_address()
            text = f"{gen.rng.choice([show(address), address.exploded])}/{gen.prefix(V6_BITS)}"
        characters = list(text)
        for _ in range(gen.rng.randint(1, 3)):
            position = gen.rng.randrange(len(characters) + 1)
            action = gen.rng.random()
            if action < 0.4 and position < len(characters):
                characters[position] = gen.rng.choice(alphabet)
            elif action < 0.7 and position < len(characters):
                del characters[position]
            else:
                characters.insert(position, gen.rng.choice(alphabet))
        text = "".join(characters)
        try:
            ipaddress.ip_interface(text)
            valid = True
        except ValueError:
            valid = False
        out.append({"input": text, "valid": valid})
    return out


def split_vectors(gen: Gen, count: int) -> list:
    out = []
    for index in range(count):
        text = gen.v6_network() if index % 4 == 3 else gen.v4_network()
        network = loose(text)
        bits = network.max_prefixlen
        new_prefix = gen.rng.choice([
            gen.rng.randint(0, bits),
            min(bits, network.prefixlen + gen.rng.randint(0, 6)),
            network.prefixlen,
            bits,
            gen.rng.randint(bits + 1, 200),
        ])
        limit = gen.rng.choice([0, 1, 2, 5, 16, 64])
        case = {"network": text, "new_prefix": new_prefix, "limit": limit}
        if new_prefix > bits or new_prefix < network.prefixlen:
            case["error"] = True
        else:
            total = 1 << (new_prefix - network.prefixlen)
            case["total"] = str(total)
            case["truncated"] = limit < total
            case["cidrs"] = [show_net(subnet) for subnet in itertools.islice(network.subnets(new_prefix=new_prefix), limit)]
        out.append(case)
    return out


def summarize_vectors(gen: Gen, count: int) -> list:
    out = []
    for index in range(count):
        kind = index % 5
        if kind in (0, 1):
            inputs = gen.clustered_v4(gen.rng.randint(1, 12), gen.rng.choice([8, 16, 22, 24, 28]))
        elif kind == 2:
            inputs = gen.clustered_v6(gen.rng.randint(1, 8))
        elif kind == 3:
            inputs = [gen.v4_network() for _ in range(gen.rng.randint(0, 6))]
        else:
            inputs = gen.clustered_v4(gen.rng.randint(1, 6), 24) + gen.clustered_v6(gen.rng.randint(1, 4))
            gen.rng.shuffle(inputs)
        parsed = [loose(text) for text in inputs]
        v4 = [network for network in parsed if network.version == 4]
        v6 = [network for network in parsed if network.version == 6]
        collapsed = list(ipaddress.collapse_addresses(v4)) + list(ipaddress.collapse_addresses(v6))
        case = {"inputs": inputs, "summary": [show_net(network) for network in collapsed]}
        if not parsed:
            case["supernet_error"] = True
        elif v4 and v6:
            case["supernet_error"] = True
        else:
            lowest = min(network.network_address for network in parsed)
            highest = max(network.broadcast_address for network in parsed)
            candidate = ipaddress.ip_network((int(lowest), lowest.max_prefixlen))
            while not (candidate.network_address <= lowest and highest <= candidate.broadcast_address):
                candidate = candidate.supernet()
            case["supernet"] = show_net(candidate)
        out.append(case)
    return out


def relation_vectors(gen: Gen, count: int) -> list:
    out = []
    for index in range(count):
        if index % 6 == 5:
            a, b = gen.v4_network(), gen.v6_network()
        elif index % 6 == 4:
            a, b = gen.clustered_v6(2)
        else:
            a, b = gen.clustered_v4(2, gen.rng.choice([8, 16, 24, 28, 30]))
        left, right = loose(a), loose(b)
        same = left.version == right.version
        out.append({
            "a": a,
            "b": b,
            "a_contains_b": same and right.subnet_of(left),
            "overlaps": same and left.overlaps(right),
        })
    return out


def range_vectors(gen: Gen, count: int) -> list:
    out = []
    for index in range(count):
        if index % 4 == 3:
            first, last = gen.v6_address(), gen.v6_address()
            if gen.rng.random() < 0.7:
                last = ipaddress.IPv6Address(min(int(first) + gen.rng.getrandbits(gen.rng.randint(0, 70)), (1 << 128) - 1))
        else:
            first, last = gen.v4_address(), gen.v4_address()
            if gen.rng.random() < 0.7:
                last = ipaddress.IPv4Address(min(int(first) + gen.rng.getrandbits(gen.rng.randint(0, 20)), (1 << 32) - 1))
        case = {"first": show(first), "last": show(last)}
        if first > last:
            case["error"] = True
        else:
            case["cidrs"] = [show_net(network) for network in ipaddress.summarize_address_range(first, last)]
        out.append(case)
    return out


def free_vectors(gen: Gen, count: int) -> list:
    out = []
    for index in range(count):
        if index % 5 == 4:
            base_prefix = gen.rng.choice([32, 48, 56, 64])
            parent = gen.clustered_v6(1, base_prefix)[0].split("/")[0] + f"/{base_prefix}"
            parent_net = loose(parent)
            used = []
            for _ in range(gen.rng.randint(0, 6)):
                prefix = gen.rng.randint(base_prefix, base_prefix + 10)
                offset = gen.rng.getrandbits(10) << (128 - base_prefix - 10)
                address = ipaddress.ip_network((int(parent_net.network_address) | offset, prefix), strict=False)
                used.append(show_net(address))
            want = gen.rng.randint(base_prefix, base_prefix + 10)
        else:
            base_prefix = gen.rng.choice([16, 20, 22, 24, 24, 26, 28])
            parent = gen.v4_network(base_prefix, base_prefix, strict=True)
            if "/" not in parent:
                parent += "/32"
            parent_net = loose(parent)
            used = []
            for _ in range(gen.rng.randint(0, 8)):
                prefix = gen.rng.randint(base_prefix, 32)
                offset = gen.rng.getrandbits(32 - base_prefix) if base_prefix < 32 else 0
                network = ipaddress.ip_network((int(parent_net.network_address) | offset, prefix), strict=False)
                used.append(str(network.network_address) if prefix == 32 and gen.rng.random() < 0.5 else str(network))
            if gen.rng.random() < 0.15:
                used.append(gen.v4_network())  # somewhere else entirely, or a supernet
            if gen.rng.random() < 0.1:
                used.append(gen.v6_network())  # the other family never matters
            want = gen.rng.choice([gen.rng.randint(base_prefix, 32), gen.rng.randint(0, 36)])
        used_nets = [loose(text) for text in used if loose(text).version == parent_net.version]

        free = [parent_net]
        for taken in used_nets:
            remaining = []
            for block in free:
                if not block.overlaps(taken):
                    remaining.append(block)
                elif not block.subnet_of(taken):
                    remaining.extend(block.address_exclude(taken))
            free = remaining
        case = {"parent": parent, "used": used, "prefix": want, "free": [show_net(n) for n in ipaddress.collapse_addresses(free)]}

        bits = parent_net.max_prefixlen
        if want > bits:
            case["next_error"] = "invalid"
        elif want < parent_net.prefixlen:
            case["next_error"] = "does_not_fit"
        elif want - parent_net.prefixlen <= 10:
            candidates = parent_net.subnets(new_prefix=want)
            found = next((c for c in candidates if not any(c.overlaps(taken) for taken in used_nets)), None)
            case["next"] = show_net(found) if found is not None else None
        else:
            # Too many candidates to walk: the first free block of the right size starts the answer.
            fitting = [block for block in ipaddress.collapse_addresses(free) if block.prefixlen <= want]
            case["next"] = show_net(next(fitting[0].subnets(new_prefix=want))) if fitting else None
        out.append(case)
    return out


def vlsm_vectors(gen: Gen, count: int) -> list:
    out = []
    for _ in range(count):
        base_prefix = gen.rng.choice([8, 16, 20, 22, 23, 24, 24, 25, 26, 27, 28, 29, 30])
        base = gen.v4_network(base_prefix, base_prefix)
        if "/" not in base:
            base += "/32"
        network = loose(base)
        capacity = network.num_addresses
        requests = []
        for number in range(gen.rng.randint(0, 9)):
            roll = gen.rng.random()
            if roll < 0.03:
                hosts = 0
            elif roll < 0.2:
                hosts = gen.rng.choice([1, 2])
            elif roll < 0.23:
                hosts = gen.rng.choice([2**32 - 2, 2**32 - 1, 2**40, 2**64 - 1])
            else:
                hosts = gen.rng.randint(1, max(2, capacity // gen.rng.choice([2, 3, 4, 8, 16])))
            name = gen.rng.choice(["", f"net-{number}", f"LAN {number}", f"p2p-{number}", f"vlan{100 + number}"])
            requests.append([name, hosts])
        p2p31 = gen.rng.random() < 0.5
        case = {"base": base, "requests": requests, "p2p31": p2p31}

        sized, failure = [], None
        for index, (name, hosts) in enumerate(requests):
            if hosts == 0:
                failure = {"error": "invalid", "request": index}
                break
            if hosts == 1:
                prefix = 32
            elif hosts == 2 and p2p31:
                prefix = 31
            else:
                prefix = next((p for p in range(30, -1, -1) if 2 ** (32 - p) - 2 >= hosts), None)
                if prefix is None:
                    failure = {"error": "does_not_fit", "request": index}
                    break
            sized.append((prefix, index))
        if failure is None:
            sized.sort()
            cursor, end = int(network.network_address), int(network.broadcast_address)
            allocations = []
            for prefix, index in sized:
                size = 2 ** (32 - prefix)
                if cursor + size - 1 > end:
                    failure = {"error": "does_not_fit", "request": index}
                    break
                subnet = ipaddress.ip_network((cursor, prefix))
                name, hosts = requests[index]
                if prefix == 32:
                    first = last = subnet.network_address
                    usable, broadcast = 1, ""
                elif prefix == 31:
                    first, last, usable, broadcast = subnet[0], subnet[1], 2, ""
                else:
                    first, last, usable, broadcast = subnet[1], subnet[-2], size - 2, str(subnet.broadcast_address)
                allocations.append({
                    "name": name,
                    "hosts_requested": hosts,
                    "cidr": str(subnet),
                    "netmask": str(subnet.netmask),
                    "first_host": str(first),
                    "last_host": str(last),
                    "broadcast": broadcast,
                    "usable_hosts": usable,
                    "spare": usable - hosts,
                })
                cursor += size
            if failure is None:
                unallocated = []
                if cursor <= end:
                    unallocated = [str(n) for n in ipaddress.summarize_address_range(ipaddress.IPv4Address(cursor), ipaddress.IPv4Address(end))]
                case["plan"] = {
                    "allocations": allocations,
                    "unallocated": unallocated,
                    "used_addresses": cursor - int(network.network_address),
                    "total_addresses": capacity,
                }
        if failure is not None:
            case.update(failure)
        out.append(case)
    return out


def cisco7_vectors(gen: Gen, count: int) -> list:
    if cisco_type7 is None:
        return []
    printable = [chr(code) for code in range(0x20, 0x7F)]
    out = [{"plain": "cisco", "salt": salt, "encoded": cisco_type7.using(salt=salt).hash("cisco")} for salt in range(53)]
    for _ in range(count):
        plain = "".join(gen.rng.choice(printable) for _ in range(gen.rng.randint(1, 70)))
        salt = gen.rng.randint(0, 52) if gen.rng.random() < 0.3 else gen.rng.randint(0, 15)
        out.append({"plain": plain, "salt": salt, "encoded": cisco_type7.using(salt=salt).hash(plain)})
    return out


UPPER = "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
LOWER = "abcdefghijklmnopqrstuvwxyz"
DIGITS = "0123456789"
SYMBOLS = "%+,-.=^_"
AMBIGUOUS = "0Oo1lI.,"


def reference_password(length: int, upper: bool, lower: bool, digits: bool, symbols: bool, avoid_ambiguous: bool, seed: bytes) -> str:
    """The algorithm documented in src/secrets/password.rs, written a second time."""
    classes = [
        [c for c in alphabet if not (avoid_ambiguous and c in AMBIGUOUS)]
        for enabled, alphabet in ((upper, UPPER), (lower, LOWER), (digits, DIGITS), (symbols, SYMBOLS))
        if enabled
    ]
    everything = [c for group in classes for c in group]
    flags = upper | lower << 1 | digits << 2 | symbols << 3 | avoid_ambiguous << 4
    prefix = b"nocturne-tools password v1" + seed + length.to_bytes(8, "big") + bytes([flags])

    def stream():
        for counter in itertools.count():
            yield from hashlib.sha256(prefix + counter.to_bytes(8, "big")).digest()

    source = stream()

    def below(bound: int) -> int:
        limit = 256 - 256 % bound
        while True:
            byte = next(source)
            if byte < limit:
                return byte % bound

    while True:
        candidate = [everything[below(len(everything))] for _ in range(length)]
        if all(any(c in group for c in candidate) for group in classes):
            return "".join(candidate)


def password_vectors(gen: Gen, count: int) -> list:
    fixed = [
        (16, True, True, True, True, False, bytes(32)),
        (24, True, True, True, False, True, bytes([0xAB] * 32)),
        (8, False, False, True, False, False, (42).to_bytes(4, "big") + bytes(28)),
    ]
    cases = list(fixed)
    for _ in range(count):
        flags = [gen.rng.random() < 0.7 for _ in range(4)]
        if not any(flags):
            flags[gen.rng.randrange(4)] = True
        length = gen.rng.choice([sum(flags), sum(flags) + 1, 8, 12, 16, 20, 32, 64, gen.rng.randint(sum(flags), 200)])
        cases.append((length, *flags, gen.rng.random() < 0.5, gen.rng.getrandbits(256).to_bytes(32, "big")))
    return [
        {
            "length": length,
            "upper": upper,
            "lower": lower,
            "digits": digits,
            "symbols": symbols,
            "avoid_ambiguous": avoid,
            "seed": seed.hex(),
            "password": reference_password(length, upper, lower, digits, symbols, avoid, seed),
        }
        for length, upper, lower, digits, symbols, avoid, seed in cases
    ]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--seed", type=int, default=20261113)
    parser.add_argument("--count", type=int, default=1000, help="scale of every section (default 1000)")
    parser.add_argument("--out", required=True, help="output file; gzip-compressed if it ends in .gz")
    args = parser.parse_args()
    if sys.version_info < (3, 10):
        # Older versions of `ipaddress` accept leading zeros in IPv4 octets.
        print("this generator needs Python 3.10 or newer", file=sys.stderr)
        sys.exit(3)

    gen = Gen(args.seed)
    scale = args.count
    vectors = {
        "meta": {
            "generator": "tests/generate_vectors.py",
            "seed": args.seed,
            "count": scale,
            "python": sys.version.split()[0],
            "passlib": None if cisco_type7 is None else __import__("passlib").__version__,
        },
        "info": info_vectors(gen, 2 * scale),
        "validity": validity_vectors(gen, scale),
        "split": split_vectors(gen, scale // 2),
        "summarize": summarize_vectors(gen, scale // 2),
        "relations": relation_vectors(gen, scale),
        "ranges": range_vectors(gen, scale // 2),
        "free": free_vectors(gen, scale // 2),
        "vlsm": vlsm_vectors(gen, scale // 2),
        "cisco7": cisco7_vectors(gen, scale // 4),
        "passwords": password_vectors(gen, scale // 10),
    }
    payload = json.dumps(vectors, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    if args.out.endswith(".gz"):
        buffer = io.BytesIO()
        with gzip.GzipFile(fileobj=buffer, mode="wb", compresslevel=9, mtime=0, filename="") as archive:
            archive.write(payload)
        payload = buffer.getvalue()
    with open(args.out, "wb") as handle:
        handle.write(payload)
    sizes = ", ".join(f"{name} {len(section)}" for name, section in vectors.items() if isinstance(section, list))
    print(f"{args.out}: {len(payload)} bytes; {sizes}", file=sys.stderr)


if __name__ == "__main__":
    main()
