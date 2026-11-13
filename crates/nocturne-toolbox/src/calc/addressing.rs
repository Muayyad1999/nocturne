//! Address maths: subnets, splitting, VLSM, summarisation, wildcard masks and
//! the conversions between the ways an address can be written.
//!
//! All of it is `nocturne_tools::{subnet, vlsm, wildcard, convert}`. What is
//! added here is the parameter reading the contract asks for — notably
//! `subnet.split` taking either a new prefix length or a number of subnets,
//! which the engine (rightly) does not guess at.

use nocturne_app::{App, AppError, AppResult, Router};
use nocturne_tools::{convert, subnet, vlsm, wildcard};
use serde::{Deserialize, Serialize};

use crate::errors::tool;

#[derive(Debug, Deserialize)]
pub struct InputParams {
    /// An address with a prefix, a mask or a wildcard: `10.0.1.5/24`,
    /// `10.0.1.5 255.255.255.0`, `10.0.1.5 0.0.0.255`, `2001:db8::1/64`.
    pub input: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SplitParams {
    pub cidr: String,
    /// The prefix length of the pieces (`26`).
    pub new_len: Option<u8>,
    /// How many pieces instead: rounded up to the next power of two.
    pub count: Option<u64>,
    /// Pieces returned in full; the total is reported either way.
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct VlsmParams {
    pub base: String,
    pub sizes: Vec<SizeRequest>,
    /// Give a two-host request a /31 instead of a /30 (RFC 3021 links).
    pub p2p31: bool,
}

#[derive(Debug, Deserialize)]
pub struct SizeRequest {
    #[serde(default)]
    pub name: String,
    pub hosts: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CidrsParams {
    pub cidrs: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PairParams {
    /// The network.
    pub a: String,
    /// The address (or network) to test against it.
    pub b: String,
}

#[derive(Debug, Deserialize)]
pub struct RangeParams {
    pub first: String,
    pub last: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AclParams {
    pub address: String,
    pub wildcard: String,
    /// An address to test against the pair, if you have one in mind.
    pub candidate: Option<String>,
}

/// What a set of networks adds up to.
#[derive(Debug, Serialize)]
pub struct SummaryResult {
    /// The smallest set of blocks covering exactly the input.
    pub summarized: Vec<String>,
    /// The single smallest block containing all of it (which may cover more).
    pub supernet: String,
}

#[derive(Debug, Serialize)]
pub struct ContainsResult {
    pub contains: bool,
    pub overlaps: bool,
}

/// Which of the given blocks overlap one another.
#[derive(Debug, Serialize)]
pub struct OverlapResult {
    pub any: bool,
    pub pairs: Vec<OverlapPair>,
}

#[derive(Debug, Serialize)]
pub struct OverlapPair {
    pub a: String,
    pub b: String,
}

#[derive(Debug, Serialize)]
pub struct RangeResult {
    pub cidrs: Vec<String>,
}

/// Every way of writing what was typed in.
#[derive(Debug, Serialize)]
pub struct ConvertResult {
    pub input: String,
    /// Present when the input is a number (in any base) or an IPv4 address.
    pub number: Option<convert::NumberForms>,
    /// Present when the input is, or denotes, an address.
    pub address: Option<AddressForms>,
}

#[derive(Debug, Serialize)]
pub struct AddressForms {
    pub version: u8,
    pub address: String,
    pub integer: String,
    pub hex: String,
    pub binary: String,
    /// IPv6 fully expanded; the address itself for IPv4.
    pub expanded: String,
    /// IPv6 in RFC 5952 form; the address itself for IPv4.
    pub compressed: String,
    /// `5.1.0.10.in-addr.arpa` / the `ip6.arpa` nibble form.
    pub reverse_dns: String,
    pub scope: String,
}

#[derive(Debug, Serialize)]
pub struct AclResult {
    /// What the `address wildcard` pair matches, in words.
    pub explanation: String,
    pub prefix: Option<u8>,
    pub netmask: String,
    pub contiguous: bool,
    pub addresses: String,
    /// Whether `candidate` is matched by the pair, when one was given.
    pub matches: Option<bool>,
}

impl From<subnet::SubnetInfo> for AddressForms {
    fn from(info: subnet::SubnetInfo) -> Self {
        AddressForms {
            version: info.version,
            address: info.address,
            integer: info.integer,
            hex: info.hex,
            binary: info.binary_address,
            expanded: info.expanded,
            compressed: info.compressed,
            reverse_dns: info.reverse_dns,
            scope: info.scope,
        }
    }
}

/// The prefix length that cuts `cidr` into `count` pieces.
///
/// A request for three subnets gets four: a network can only be halved, so
/// anything else would be a lie about what the device will see.
fn length_for(cidr: &str, count: u64) -> AppResult<u8> {
    if count == 0 {
        return Err(AppError::invalid(
            "`count` must be at least 1 (how many subnets to split the network into)",
        ));
    }
    let info = subnet::info(cidr).map_err(tool)?;
    let bits = u8::try_from(count.next_power_of_two().trailing_zeros())
        .map_err(|_| AppError::invalid("`count` is larger than any network can be split into"))?;
    let max = if info.version == 4 { 32 } else { 128 };
    info.prefix
        .checked_add(bits)
        .filter(|length| *length <= max)
        .ok_or_else(|| {
            AppError::invalid(format!(
                "{} cannot be split into {count} subnets: that would need a /{} prefix",
                info.cidr,
                u32::from(info.prefix) + u32::from(bits)
            ))
        })
}

/// The default number of subnets `subnet.split` returns in full.
const SPLIT_LIMIT: usize = 256;

pub fn register(router: &mut Router) {
    router.add("tools.subnet", |_: App, params: InputParams| async move {
        subnet::info(&params.input).map_err(tool)
    });
    router.add(
        "tools.subnet.split",
        |_: App, params: SplitParams| async move {
            let new_len = match (params.new_len, params.count) {
                (Some(length), _) => length,
                (None, Some(count)) => length_for(&params.cidr, count)?,
                (None, None) => {
                    return Err(AppError::invalid(
                        "say how to split the network: `new_len` (the prefix length of the pieces) or `count` (how many)",
                    ));
                }
            };
            let limit = params
                .limit
                .unwrap_or(SPLIT_LIMIT)
                .clamp(1, subnet::MAX_SPLIT_SUBNETS);
            subnet::split(&params.cidr, new_len, limit).map_err(tool)
        },
    );
    router.add("tools.vlsm", |_: App, params: VlsmParams| async move {
        if params.sizes.is_empty() {
            return Err(AppError::invalid(
                "a VLSM plan needs at least one size: `sizes: [{name, hosts}]`",
            ));
        }
        let requests: Vec<vlsm::Request> = params
            .sizes
            .into_iter()
            .map(|size| vlsm::Request {
                name: size.name,
                hosts: size.hosts,
            })
            .collect();
        vlsm::plan(&params.base, &requests, params.p2p31).map_err(tool)
    });
    router.add("tools.supernet", |_: App, params: CidrsParams| async move {
        if params.cidrs.is_empty() {
            return Err(AppError::invalid(
                "`cidrs` is empty: give networks to summarise",
            ));
        }
        Ok(SummaryResult {
            summarized: subnet::summarize(&params.cidrs).map_err(tool)?,
            supernet: subnet::supernet(&params.cidrs).map_err(tool)?,
        })
    });
    router.add("tools.contains", |_: App, params: PairParams| async move {
        Ok(ContainsResult {
            contains: subnet::contains(&params.a, &params.b).map_err(tool)?,
            overlaps: subnet::overlaps(&params.a, &params.b).map_err(tool)?,
        })
    });
    router.add("tools.overlaps", |_: App, params: CidrsParams| async move {
        let mut pairs = Vec::new();
        for (index, a) in params.cidrs.iter().enumerate() {
            for b in &params.cidrs[index + 1..] {
                if subnet::overlaps(a, b).map_err(tool)? {
                    pairs.push(OverlapPair {
                        a: a.clone(),
                        b: b.clone(),
                    });
                }
            }
        }
        Ok(OverlapResult {
            any: !pairs.is_empty(),
            pairs,
        })
    });
    router.add("tools.ip.range", |_: App, params: RangeParams| async move {
        Ok(RangeResult {
            cidrs: subnet::range_to_cidrs(&params.first, &params.last).map_err(tool)?,
        })
    });
    router.add(
        "tools.ip.convert",
        |_: App, params: InputParams| async move {
            let input = params.input.trim().to_string();
            let number = convert::number(&input).ok();
            // "3232235777" and "0xC0A80101" are addresses too; the number form
            // says which address, and that is what gets the address treatment.
            let as_address = match subnet::info(&input) {
                Ok(info) => Some(info),
                Err(_) => number
                    .as_ref()
                    .and_then(|forms| forms.ipv4.as_deref())
                    .and_then(|address| subnet::info(address).ok()),
            };
            if number.is_none() && as_address.is_none() {
                // Both readings failed: the address reading has the better message.
                return Err(tool(subnet::info(&input).expect_err("no address")));
            }
            Ok(ConvertResult {
                input,
                number,
                address: as_address.map(AddressForms::from),
            })
        },
    );
    router.add("tools.wildcard", |_: App, params: InputParams| async move {
        wildcard::convert(&params.input).map_err(tool)
    });
    router.add(
        "tools.wildcard.acl",
        |_: App, params: AclParams| async move {
            let explanation = wildcard::explain(&params.address, &params.wildcard).map_err(tool)?;
            let mask = wildcard::convert(&params.wildcard).map_err(tool)?;
            let matches = match params.candidate.as_deref() {
                Some(candidate) => Some(
                    wildcard::matches(&params.address, &params.wildcard, candidate)
                        .map_err(tool)?,
                ),
                None => None,
            };
            Ok(AclResult {
                explanation,
                prefix: mask.prefix,
                netmask: mask.netmask,
                contiguous: mask.contiguous,
                addresses: mask.addresses,
                matches,
            })
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_of_subnets_becomes_the_prefix_length_that_delivers_them() {
        // A /24 in four is /26; three is also /26, because halving is all a
        // network can do. Hand-checked, not copied from the engine.
        assert_eq!(length_for("10.0.0.0/24", 4).unwrap(), 26);
        assert_eq!(length_for("10.0.0.0/24", 3).unwrap(), 26);
        assert_eq!(length_for("10.0.0.0/24", 1).unwrap(), 24);
        assert_eq!(length_for("2001:db8::/64", 256).unwrap(), 72);
        assert_eq!(length_for("10.0.0.0/24", 0).unwrap_err().code, "invalid");
        let too_many = length_for("10.0.0.0/24", 1_000).unwrap_err();
        assert_eq!(too_many.code, "invalid");
        assert!(too_many.message.contains("/34"), "{}", too_many.message);
    }
}
