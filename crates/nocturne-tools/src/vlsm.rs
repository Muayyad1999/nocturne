//! Variable-length subnet planning: carve a base network into subnets sized
//! for a list of host counts.

use serde::{Deserialize, Serialize};

use crate::ToolError;
use crate::ip::{Family, Interface, Net, format_ipv4, group_thousands, low32, netmask_v4, range_to_nets};

/// One subnet to plan for.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// What the subnet is for ("Sales", "WAN link to Berlin").
    pub name: String,
    /// How many host addresses it needs.
    pub hosts: u64,
}

/// The subnet given to one [`Request`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Allocation {
    /// The request's name.
    pub name: String,
    /// The number of hosts that were asked for.
    pub hosts_requested: u64,
    /// The allocated subnet in CIDR notation.
    pub cidr: String,
    /// Its dotted netmask.
    pub netmask: String,
    /// First usable host address.
    pub first_host: String,
    /// Last usable host address.
    pub last_host: String,
    /// Broadcast address (empty for /31 and /32).
    pub broadcast: String,
    /// Usable host addresses in the subnet.
    pub usable_hosts: u64,
    /// Usable addresses beyond what was asked for.
    pub spare: u64,
}

/// A complete VLSM plan.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The allocated subnets in address order (which is largest first).
    pub allocations: Vec<Allocation>,
    /// The part of the base network that is still free, as CIDR blocks.
    pub unallocated: Vec<String>,
    /// Addresses taken by the allocations, network and broadcast addresses included.
    pub used_addresses: u64,
    /// Addresses in the base network.
    pub total_addresses: u64,
}

/// Plan subnets for `requests` inside the IPv4 network `base`.
///
/// Requests are placed largest first (equal sizes in the order given), each
/// on its natural boundary, so the plan wastes nothing to alignment. A request
/// for N hosts gets the smallest subnet with at least N usable addresses:
/// 1 host a /32, 2 hosts a /30 — or a /31 point-to-point link (RFC 3021) when
/// `p2p31` is set — and from 3 hosts up the usual 2^n - 2 rule.
///
/// Fails with [`ToolError::DoesNotFit`], naming the request, when the base
/// network runs out.
pub fn plan(base: &str, requests: &[Request], p2p31: bool) -> Result<Plan, ToolError> {
    let base_net = Interface::parse(base)?.net;
    if base_net.family != Family::V4 {
        return Err(ToolError::invalid(
            "base network",
            base.trim(),
            "VLSM planning sizes subnets by host count, which only makes sense for IPv4; \
             IPv6 subnets are /64 by convention (use the subnet splitter)",
        ));
    }
    // IPv4 arithmetic fits comfortably in u64, including the 2^32 of a /0.
    let base_first = u64::from(low32(base_net.first));
    let base_last = u64::from(low32(base_net.last()));
    let total_addresses = base_last - base_first + 1;

    let mut sized = Vec::with_capacity(requests.len());
    for (index, request) in requests.iter().enumerate() {
        if request.hosts == 0 {
            return Err(ToolError::invalid(
                "VLSM request",
                &name_or_number(index, request),
                "it asks for 0 hosts; a subnet needs at least 1",
            ));
        }
        let Some(prefix) = prefix_for_hosts(request.hosts, p2p31) else {
            return Err(ToolError::DoesNotFit(format!(
                "{} needs {} hosts, more than any IPv4 subnet can hold",
                label(index, request),
                group_thousands(&request.hosts.to_string())
            )));
        };
        sized.push((prefix, index));
    }
    // Largest subnet (shortest prefix) first; the sort is stable, so equal
    // sizes keep the order in which they were asked for.
    sized.sort_by_key(|(prefix, _)| *prefix);
    let needed: u64 = sized.iter().map(|(prefix, _)| block_size(*prefix)).fold(0, u64::saturating_add);

    let mut allocations = Vec::with_capacity(sized.len());
    let mut cursor = base_first;
    for (prefix, index) in sized {
        let request = &requests[index];
        let size = block_size(prefix);
        // Blocks only shrink from one allocation to the next, so the cursor is
        // always on a boundary of the next block and the space that is left
        // is a whole number of such blocks: a request fails only when the
        // base is full or was too small for it from the start.
        let free = base_last + 1 - cursor;
        if size > free {
            let room = if size > total_addresses {
                format!("the base network {base_net} only has {}", group_thousands(&total_addresses.to_string()))
            } else {
                format!("{base_net} is full once the larger subnets are placed")
            };
            return Err(ToolError::DoesNotFit(format!(
                "{} does not fit: {} hosts need a /{prefix} ({} addresses), but {room} \
                 (all requests together need {} addresses, the base has {})",
                label(index, request),
                group_thousands(&request.hosts.to_string()),
                group_thousands(&size.to_string()),
                group_thousands(&needed.to_string()),
                group_thousands(&total_addresses.to_string()),
            )));
        }
        allocations.push(allocation(request, cursor, prefix));
        cursor += size;
    }

    let mut unallocated = Vec::new();
    if cursor <= base_last {
        range_to_nets(Family::V4, u128::from(cursor), u128::from(base_last), &mut unallocated);
    }
    Ok(Plan {
        allocations,
        unallocated: unallocated.iter().map(Net::to_string).collect(),
        used_addresses: cursor - base_first,
        total_addresses,
    })
}

/// The request's name, or its position in the list if it has none.
fn name_or_number(index: usize, request: &Request) -> String {
    let name = request.name.trim();
    if name.is_empty() { format!("#{}", index + 1) } else { crate::error::snip(name, 60) }
}

fn label(index: usize, request: &Request) -> String {
    if request.name.trim().is_empty() {
        format!("request #{}", index + 1)
    } else {
        format!("request `{}`", name_or_number(index, request))
    }
}

fn block_size(prefix: u8) -> u64 {
    1u64 << (32 - prefix)
}

/// Usable host addresses in an IPv4 subnet of the given prefix length.
fn usable_hosts(prefix: u8) -> u64 {
    match prefix {
        32 => 1,
        31 => 2,
        _ => block_size(prefix) - 2,
    }
}

/// The longest prefix whose subnet still has `hosts` usable addresses, or
/// `None` if not even a /0 is big enough.
fn prefix_for_hosts(hosts: u64, p2p31: bool) -> Option<u8> {
    match hosts {
        0 => None,
        1 => Some(32),
        2 if p2p31 => Some(31),
        _ => {
            // Smallest n with 2^n >= hosts + network + broadcast.
            let addresses = hosts.checked_add(2)?;
            let host_bits = 64 - (addresses - 1).leading_zeros();
            u8::try_from(host_bits).ok().and_then(|bits| 32u8.checked_sub(bits))
        }
    }
}

fn allocation(request: &Request, first: u64, prefix: u8) -> Allocation {
    let last = first + block_size(prefix) - 1;
    let show = |value: u64| format_ipv4(low32(u128::from(value)));
    let (first_host, last_host, broadcast) = match prefix {
        32 => (first, first, String::new()),
        31 => (first, last, String::new()),
        _ => (first + 1, last - 1, show(last)),
    };
    let usable_hosts = usable_hosts(prefix);
    Allocation {
        name: request.name.clone(),
        hosts_requested: request.hosts,
        cidr: format!("{}/{prefix}", show(first)),
        netmask: format_ipv4(netmask_v4(prefix)),
        first_host: show(first_host),
        last_host: show(last_host),
        broadcast,
        usable_hosts,
        spare: usable_hosts - request.hosts,
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn requests(items: &[(&str, u64)]) -> Vec<Request> {
        items.iter().map(|(name, hosts)| Request { name: (*name).to_owned(), hosts: *hosts }).collect()
    }

    #[test]
    fn sizes_follow_the_usable_host_rule() {
        for (hosts, classic, with_p2p31) in [
            (1, 32, 32),
            (2, 30, 31),
            (3, 29, 29),
            (6, 29, 29),
            (7, 28, 28),
            (14, 28, 28),
            (15, 27, 27),
            (126, 25, 25),
            (127, 24, 24),
            (254, 24, 24),
            (255, 23, 23),
            (510, 23, 23),
            (511, 22, 22),
            (65_534, 16, 16),
            (65_535, 15, 15),
            (16_777_214, 8, 8),
            (16_777_215, 7, 7),
            (2_147_483_646, 1, 1),
            (2_147_483_647, 0, 0),
            (4_294_967_294, 0, 0),
        ] {
            assert_eq!(prefix_for_hosts(hosts, false), Some(classic), "{hosts} hosts");
            assert_eq!(prefix_for_hosts(hosts, true), Some(with_p2p31), "{hosts} hosts with /31");
        }
        assert_eq!(prefix_for_hosts(4_294_967_295, false), None);
        assert_eq!(prefix_for_hosts(u64::MAX, true), None);
        assert_eq!(prefix_for_hosts(0, false), None);
    }

    #[test]
    fn a_textbook_plan() {
        // The classic exercise: 192.168.1.0/24 for LANs of 100, 50, 25 and 10
        // hosts plus two WAN links.
        let plan = plan(
            "192.168.1.0/24",
            &requests(&[("WAN-1", 2), ("LAN-C", 25), ("LAN-A", 100), ("LAN-D", 10), ("WAN-2", 2), ("LAN-B", 50)]),
            false,
        )
        .unwrap();
        let rows: Vec<(&str, &str, &str, &str, &str, u64, u64)> = plan
            .allocations
            .iter()
            .map(|a| {
                (
                    a.name.as_str(),
                    a.cidr.as_str(),
                    a.first_host.as_str(),
                    a.last_host.as_str(),
                    a.broadcast.as_str(),
                    a.usable_hosts,
                    a.spare,
                )
            })
            .collect();
        assert_eq!(rows, [
            ("LAN-A", "192.168.1.0/25", "192.168.1.1", "192.168.1.126", "192.168.1.127", 126, 26),
            ("LAN-B", "192.168.1.128/26", "192.168.1.129", "192.168.1.190", "192.168.1.191", 62, 12),
            ("LAN-C", "192.168.1.192/27", "192.168.1.193", "192.168.1.222", "192.168.1.223", 30, 5),
            ("LAN-D", "192.168.1.224/28", "192.168.1.225", "192.168.1.238", "192.168.1.239", 14, 4),
            ("WAN-1", "192.168.1.240/30", "192.168.1.241", "192.168.1.242", "192.168.1.243", 2, 0),
            ("WAN-2", "192.168.1.244/30", "192.168.1.245", "192.168.1.246", "192.168.1.247", 2, 0),
        ]);
        assert_eq!(plan.allocations[0].netmask, "255.255.255.128");
        assert_eq!(plan.allocations[4].netmask, "255.255.255.252");
        assert_eq!(plan.allocations[2].hosts_requested, 25);
        assert_eq!(plan.unallocated, ["192.168.1.248/29"]);
        assert_eq!((plan.used_addresses, plan.total_addresses), (248, 256));
    }

    #[test]
    fn point_to_point_links_can_use_slash_31() {
        let plan = plan("10.0.0.0/29", &requests(&[("p2p-a", 2), ("p2p-b", 2), ("loopback", 1)]), true).unwrap();
        let cidrs: Vec<&str> = plan.allocations.iter().map(|a| a.cidr.as_str()).collect();
        assert_eq!(cidrs, ["10.0.0.0/31", "10.0.0.2/31", "10.0.0.4/32"]);
        let link = &plan.allocations[1];
        assert_eq!((link.first_host.as_str(), link.last_host.as_str(), link.broadcast.as_str()), ("10.0.0.2", "10.0.0.3", ""));
        assert_eq!((link.usable_hosts, link.spare, link.netmask.as_str()), (2, 0, "255.255.255.254"));
        let loopback = &plan.allocations[2];
        assert_eq!(
            (loopback.first_host.as_str(), loopback.last_host.as_str(), loopback.broadcast.as_str()),
            ("10.0.0.4", "10.0.0.4", "")
        );
        assert_eq!((loopback.usable_hosts, loopback.spare, loopback.netmask.as_str()), (1, 0, "255.255.255.255"));
        assert_eq!(plan.unallocated, ["10.0.0.5/32", "10.0.0.6/31"]);
        assert_eq!((plan.used_addresses, plan.total_addresses), (5, 8));
    }

    #[test]
    fn a_plan_that_exactly_fills_the_base() {
        let plan = plan("172.16.0.0/22", &requests(&[("a", 500), ("b", 250), ("c", 120), ("d", 60), ("e", 60)]), false).unwrap();
        let cidrs: Vec<&str> = plan.allocations.iter().map(|a| a.cidr.as_str()).collect();
        assert_eq!(cidrs, ["172.16.0.0/23", "172.16.2.0/24", "172.16.3.0/25", "172.16.3.128/26", "172.16.3.192/26"]);
        assert!(plan.unallocated.is_empty());
        assert_eq!((plan.used_addresses, plan.total_addresses), (1024, 1024));
        // Equal-sized requests keep the order they were given in.
        assert_eq!((plan.allocations[3].name.as_str(), plan.allocations[4].name.as_str()), ("d", "e"));
    }

    #[test]
    fn a_plan_that_overflows_names_the_request() {
        let error = plan("192.168.1.0/24", &requests(&[("Servers", 100), ("Users", 120), ("Guests", 60)]), false).unwrap_err();
        assert_eq!(
            error,
            ToolError::DoesNotFit(
                "request `Guests` does not fit: 60 hosts need a /26 (64 addresses), but 192.168.1.0/24 is full once the \
                 larger subnets are placed (all requests together need 320 addresses, the base has 256)"
                    .into()
            )
        );

        // The request that fails is the first one, largest first, that finds no room -
        // not necessarily the last one in the list.
        let odd = plan("10.0.0.0/24", &requests(&[("big", 100), ("mid", 50), ("small", 20), ("mid2", 50)]), false).unwrap_err();
        assert_eq!(
            odd.to_string(),
            "request `small` does not fit: 20 hosts need a /27 (32 addresses), but 10.0.0.0/24 is full once the larger \
             subnets are placed (all requests together need 288 addresses, the base has 256)"
        );

        let too_big = plan("192.168.1.0/24", &requests(&[("Campus", 1000)]), false).unwrap_err();
        assert_eq!(
            too_big,
            ToolError::DoesNotFit(
                "request `Campus` does not fit: 1,000 hosts need a /22 (1,024 addresses), but the base network \
                 192.168.1.0/24 only has 256 (all requests together need 1,024 addresses, the base has 256)"
                    .into()
            )
        );

        let unnamed = plan("10.0.0.0/30", &requests(&[("", 2), ("  ", 2)]), false).unwrap_err();
        assert!(unnamed.to_string().starts_with("request #2 does not fit"), "{unnamed}");

        let absurd = plan("10.0.0.0/8", &requests(&[("everything", u64::MAX)]), false).unwrap_err();
        assert_eq!(
            absurd,
            ToolError::DoesNotFit(
                "request `everything` needs 18,446,744,073,709,551,615 hosts, more than any IPv4 subnet can hold".into()
            )
        );
    }

    #[test]
    fn edge_cases() {
        // No requests: everything is unallocated.
        let empty = plan("10.1.2.3/24", &[], false).unwrap();
        assert_eq!(empty.unallocated, ["10.1.2.0/24"]);
        assert_eq!((empty.used_addresses, empty.total_addresses), (0, 256));

        // The whole IPv4 space in one request.
        let all = plan("0.0.0.0/0", &requests(&[("internet", 4_294_967_294)]), false).unwrap();
        assert_eq!(all.allocations[0].cidr, "0.0.0.0/0");
        assert_eq!(all.allocations[0].broadcast, "255.255.255.255");
        assert_eq!((all.used_addresses, all.total_addresses), (4_294_967_296, 4_294_967_296));

        // A /32 base holds exactly one loopback.
        let single = plan("10.0.0.1", &requests(&[("lo0", 1)]), false).unwrap();
        assert_eq!(single.allocations[0].cidr, "10.0.0.1/32");
        assert!(single.unallocated.is_empty());

        // Without /31 support two hosts need a /30, which a /31 base cannot hold.
        assert!(matches!(plan("10.0.0.0/31", &requests(&[("p2p", 2)]), false), Err(ToolError::DoesNotFit(_))));
        assert!(plan("10.0.0.0/31", &requests(&[("p2p", 2)]), true).is_ok());

        let zero = plan("10.0.0.0/24", &requests(&[("ok", 5), ("nothing", 0)]), false).unwrap_err();
        assert_eq!(zero.to_string(), "invalid VLSM request `nothing`: it asks for 0 hosts; a subnet needs at least 1");
        let unnamed_zero = plan("10.0.0.0/24", &requests(&[("", 0)]), false).unwrap_err();
        assert_eq!(unnamed_zero.to_string(), "invalid VLSM request `#1`: it asks for 0 hosts; a subnet needs at least 1");

        assert!(plan("2001:db8::/48", &requests(&[("lan", 100)]), false).unwrap_err().to_string().contains("only makes sense for IPv4"));
        assert!(plan("not a network", &[], false).is_err());
    }
}
