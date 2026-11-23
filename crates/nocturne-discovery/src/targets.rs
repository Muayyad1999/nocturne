//! Working out which addresses a scan may probe, and refusing the rest.
//!
//! The address maths (CIDR blocks, ranges, the network and broadcast
//! addresses of an IPv4 block) belongs to `nocturne-netprobe`; this module
//! adds the judgement a sweep needs on top of it:
//!
//! * a name is resolved, and what it resolves to is shown to the user;
//! * multicast, broadcast and unspecified addresses are never probed;
//! * anything outside private, link-local, unique-local and loopback space is
//!   refused unless the request asks for it *and* the settings allow it;
//! * blocks the settings name are never probed, whatever the request says;
//! * a target expression larger than `max_hosts` is refused rather than
//!   quietly truncated — a user who asks for a /16 and gets its first 4096
//!   addresses would believe the rest of it is empty.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use ipnet::IpNet;
use nocturne_app::{AppError, AppResult};
use nocturne_netprobe::{dns, sweep};

/// The addresses a scan will probe, and how the expression was read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub addresses: Vec<IpAddr>,
    /// One line per target as typed, saying what it expanded to. Goes into the job log.
    pub notes: Vec<String>,
    /// Addresses left out because the settings refuse them.
    pub excluded: usize,
    /// Whether public address space is included.
    pub public: bool,
}

/// Expand and vet `targets`.
///
/// `allow_public` is the request's wish *after* the settings have had their
/// say: the caller passes `false` when the settings forbid it.
pub async fn plan(
    targets: &str,
    max_hosts: u32,
    allow_public: bool,
    never_scan: &[String],
) -> AppResult<Plan> {
    let refused = parse_never_scan(never_scan)?;
    let tokens: Vec<&str> = targets
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() {
        return Err(AppError::invalid(
            "give the scan something to look at: a CIDR block (10.0.0.0/24), a range (10.0.1.5-10.0.1.40), an address or a host name",
        ));
    }

    let limit = max_hosts as usize;
    let mut plan = Plan::default();
    let mut seen = std::collections::HashSet::new();
    for token in tokens {
        // The engine owns the address maths, and it also says which tokens
        // are names. Only a name goes to the resolver: an address, a range or
        // a block it refused would otherwise be sent to DNS as one, telling a
        // resolver outside the network what was about to be scanned.
        let expanded = if sweep::is_host_name(token) {
            match dns::resolve(token).await {
                Ok(addresses) if !addresses.is_empty() => {
                    plan.notes.push(format!(
                        "`{token}` resolves to {}",
                        addresses
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                    addresses
                }
                Ok(_) => {
                    return Err(AppError::invalid(format!(
                        "`{token}` is a host name that resolves to no address"
                    )));
                }
                Err(err) => {
                    return Err(AppError::invalid(format!(
                        "`{token}` is not an address, a range (10.0.1.5-10.0.1.40) or a CIDR block (10.0.0.0/24), and as a host name it does not resolve: {err}"
                    )));
                }
            }
        } else {
            let invalid = |engine_error| AppError::invalid(format!("`{token}`: {engine_error}"));
            // Counted before it is expanded, so that "that is too big" names
            // the setting rather than an internal limit, for a range as for a
            // block.
            let hosts = sweep::count_targets(token).map_err(invalid)?;
            if hosts > limit as u128 {
                return Err(AppError::invalid(format!(
                    "`{token}` covers {hosts} addresses; this scan may probe at most {limit}. Scan it in smaller pieces, or raise `max_hosts` under Settings → Discovery (at most {})",
                    crate::settings::MAX_HOSTS_LIMIT
                )));
            }
            sweep::expand_targets(token, limit).map_err(invalid)?
        };
        let mut kept = 0usize;
        for address in expanded {
            if let Some(reason) = unprobeable(address) {
                return Err(AppError::invalid(format!(
                    "`{token}` includes {address}, which is {reason}"
                )));
            }
            if !is_internal(address) {
                if !allow_public {
                    return Err(AppError::invalid(format!(
                        "`{token}` includes {address}, which is a public address. Sweeping address space that is not yours is rarely what you meant; if it really is, allow public scans under Settings → Discovery and ask for them with `allow_public`"
                    )).with_details(serde_json::json!({"address": address.to_string(), "needs": "allow_public"})));
                }
                plan.public = true;
            }
            if refused.iter().any(|block| block.contains(&address)) {
                plan.excluded += 1;
                continue;
            }
            if seen.insert(address) {
                plan.addresses.push(address);
                kept += 1;
            }
            if plan.addresses.len() > limit {
                return Err(AppError::invalid(format!(
                    "those targets cover more than {limit} addresses between them; this scan may probe at most {limit}. Scan them in smaller pieces, or raise `max_hosts` under Settings → Discovery"
                )));
            }
        }
        if kept == 0 && plan.excluded > 0 {
            plan.notes
                .push(format!("`{token}` is entirely inside a never-scan block"));
        }
    }

    if plan.addresses.is_empty() {
        return Err(AppError::invalid(
            "every address in those targets is excluded by the never-scan list under Settings → Discovery",
        ));
    }
    if plan.excluded > 0 {
        plan.notes.push(format!(
            "{} address(es) left out: the never-scan list",
            plan.excluded
        ));
    }
    if plan.public {
        plan.notes
            .push("this scan covers public address space (allow_public)".into());
    }
    Ok(plan)
}

fn parse_never_scan(entries: &[String]) -> AppResult<Vec<IpNet>> {
    let mut blocks = Vec::new();
    for entry in entries {
        for token in entry
            .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            let block = token
                .parse::<IpNet>()
                .or_else(|_| token.parse::<IpAddr>().map(IpNet::from))
                .map_err(|_| {
                    AppError::invalid(format!(
                        "`{token}` in the never-scan list is not an address or a CIDR block"
                    ))
                })?;
            blocks.push(block);
        }
    }
    Ok(blocks)
}

/// Host addresses in a block, as a sweep counts them: the network and
/// broadcast addresses of an IPv4 block are not hosts.
pub fn host_count(block: &IpNet) -> u64 {
    match block {
        IpNet::V4(v4) => match v4.prefix_len() {
            32 => 1,
            31 => 2,
            bits => 2u64.saturating_pow(u32::from(32 - bits)).saturating_sub(2),
        },
        IpNet::V6(v6) => match v6.prefix_len() {
            bits if bits >= 64 => 2u64.saturating_pow(u32::from(128 - bits)),
            // Wider than a /64 is more addresses than there are atoms worth
            // probing; it is refused on size like anything else.
            _ => u64::MAX,
        },
    }
}

/// Why an address is never probed, or `None` when it may be.
pub fn unprobeable(address: IpAddr) -> Option<&'static str> {
    match address {
        IpAddr::V4(v4) => {
            if v4.is_unspecified() {
                Some("the unspecified address")
            } else if v4.is_multicast() {
                Some("a multicast address")
            } else if v4.is_broadcast() {
                Some("the broadcast address")
            } else {
                None
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_unspecified() {
                Some("the unspecified address")
            } else if v6.is_multicast() {
                Some("a multicast address")
            } else {
                None
            }
        }
    }
}

/// Whether an address belongs to space an operator plausibly owns: RFC 1918,
/// carrier-grade NAT, link-local, loopback, IPv6 unique-local.
pub fn is_internal(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            v4.is_private() || v4.is_link_local() || v4.is_loopback() || is_shared_v4(v4)
        }
        IpAddr::V6(v6) => v6.is_loopback() || is_unique_local(v6) || is_link_local_v6(v6),
    }
}

/// 100.64.0.0/10, RFC 6598 — the address space a carrier hands to CPE, and
/// what a good many management networks sit behind.
fn is_shared_v4(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 100 && (64..128).contains(&octets[1])
}

/// fc00::/7.
fn is_unique_local(address: Ipv6Addr) -> bool {
    address.segments()[0] & 0xfe00 == 0xfc00
}

/// fe80::/10.
fn is_link_local_v6(address: Ipv6Addr) -> bool {
    address.segments()[0] & 0xffc0 == 0xfe80
}
