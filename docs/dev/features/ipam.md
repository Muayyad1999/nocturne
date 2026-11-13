# Feature: IP address management (`nocturne-ipam`)

The spreadsheet every network team keeps, kept by the network itself:
prefixes, addresses and VLANs, filled in from what devices report, with the
arithmetic done for you — what is free, what overlaps, what is nearly full,
who has this address.

Engines: `nocturne-tools` (subnet maths: read `docs/dev/spec-tools.md` —
parsing, containment, VLSM, next-free), core `collect` (interfaces, arp,
vlans), `inventory`. No device access.

## Model (contract)

```rust
struct Prefix { id, cidr /* canonical: network address/len, v4 or v6 */, name, description,
    site?, vlan_id?: u16, role?: lan|wan|loopback|management|p2p|dmz|guest|voice|other,
    gateway?, dhcp?: {start, end}, tags, source: manual|discovered, created_at, updated_at }
struct PrefixRow { ..Prefix, parent_id?, depth, children: u32,
    usable: u64, used: u32, reserved: u32, free: u64, util_pct: f32,      // from addresses inside it (v6: used only)
    devices: [{device_id, device, interface, ip}] }                      // interfaces configured in it
struct Address { ip, prefix_id?, status: active|reserved|dhcp|free|conflict,
    hostname?, description?, device_id?, interface?, mac?, vendor?,
    source: manual|interface|arp|dns, first_seen, last_seen, tags }
struct Vlan { id /* doc id */, vid: u16, name, site?, description, prefix_ids: [..],
    devices: [{device_id, device, name /* as named on that device */, ports: u32}], source }
```

## RPC (contract)

| Method | Params → Result |
|---|---|
| `ipam.prefixes` | `{site?, query?, parent_id?, flat?: false}` → `[PrefixRow]` as a containment tree in address order |
| `ipam.prefix.get` | `{id, addresses?: true}` → `{prefix: PrefixRow, addresses: [Address], free_ranges: [{start, end, count}]}` — for a /24 or smaller the addresses list is the whole block with unused ones as `free` (the classic grid); larger prefixes list only known addresses |
| `ipam.prefix.save` / `.delete` | validates CIDR (host bits set → error that shows the corrected network), refuses exact duplicates within a site, warns (not refuses) on overlap that is not containment |
| `ipam.prefix.split` | `{id, new_len}` or `{id, sizes: [hosts..]}` (VLSM) → child prefixes created |
| `ipam.next_prefix` | `{parent_id, len}` → the first free child subnet of that size (does not create it) |
| `ipam.next_address` | `{prefix_id, count?: 1, skip_first?: n}` → free addresses, skipping network/broadcast/gateway/DHCP range and anything seen in ARP within the last 30 days |
| `ipam.address.save` / `.delete` / `.reserve {prefix_id, ip?, hostname, description}` | manual records; reserving an address that is in use is a `conflict` naming who has it |
| `ipam.search` | `{query}` → `{prefixes, addresses, vlans}` — an IP finds its address record **and** the prefix containing it; a MAC or hostname fragment finds addresses |
| `ipam.sync` | `{device_ids?}` → `{prefixes_added, addresses_updated, vlans_updated, conflicts}` — pull from collected data now (also runs, debounced, on `collect.updated`) |
| `ipam.conflicts` | → `[{kind: duplicate_ip|overlap|outside_any_prefix|gateway_mismatch, subject, detail, involved: [..]}]` |
| `ipam.vlans` / `ipam.vlan.save` / `.delete` | |
| `ipam.import` | `{format: csv, text, kind: prefixes|addresses|vlans, dry_run?: true}` → preview/commit (same pattern as the device import in the core) |
| `ipam.export` | `{kind, format: csv}` → `{text}` |
| `ipam.summary` | → `{prefixes, addresses, vlans, utilisation: [{prefix, util_pct}] top 10, conflicts}` |

## Sync rules

* Every collected interface address creates/refreshes an `Address`
  (`source: interface`, `status: active`, device and interface set) and —
  when no prefix contains it — a `discovered` prefix for its network
  (site from the device). Discovered prefixes never overwrite manual
  fields; a user edit turns a prefix `manual`.
* ARP entries create/refresh `source: arp` addresses with MAC + vendor and
  `last_seen`; they never override an `interface` or `manual` record's
  identity, only its `mac`/`last_seen`. ARP-only addresses not seen for 30
  days (setting) become `free` again.
* The same IP on two devices' interfaces, or one IP with two different MACs
  seen within an hour, is a `conflict` (unless a VRRP/HSRP virtual address:
  the well-known virtual MAC ranges are recognised).
* Lab devices' addresses live in a separate namespace: prefixes carry
  `site = "lab:<lab id>"` and never mix with production utilisation.
* Collected VLANs merge by (site, vid); differing names across devices are
  kept per device in `devices[].name`.

Events (contract): `ipam.changed {kinds: [..]}`.

## Tests

Tree building (nesting, ordering, v4 and v6, siblings, a /31 and /32),
utilisation numbers checked by hand, `free_ranges`, next-free with every
exclusion, split and VLSM against hand-computed plans, CIDR validation
messages, sync from hand-built collected rows (interfaces with secondary
addresses, ARP ageing, conflicts, virtual MACs, lab separation, manual
fields surviving a sync), search by IP / partial IP / MAC / name, import
preview/commit and export round trip, 50,000 addresses stay fast (list and
search under 100 ms).
