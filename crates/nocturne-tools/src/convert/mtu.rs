//! MTU and TCP MSS left over after a stack of encapsulations.
//!
//! The calculation starts from the IP MTU of the link and walks through the
//! encapsulations from the outermost (the one closest to the link) to the
//! innermost, taking each one's headers off what is left. Most overheads are
//! constants; ESP is not — its payload is padded to the cipher's block size —
//! so its cost depends on what is left when its turn comes.

use serde::{Deserialize, Serialize};

use crate::ToolError;
use crate::error::snip;

/// The outcome of [`mtu`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MtuResult {
    /// The largest IP packet that fits inside all the encapsulations without fragmenting.
    pub payload_mtu: u32,
    /// TCP maximum segment size for IPv4 inside the tunnel (`payload_mtu` - 40).
    pub tcp_mss_v4: u32,
    /// TCP maximum segment size for IPv6 inside the tunnel (`payload_mtu` - 60).
    pub tcp_mss_v6: u32,
    /// Bytes lost to all encapsulations together.
    pub overhead: u32,
    /// What each encapsulation costs, in the order given: a label such as
    /// `vxlan (IPv4 20 + UDP 8 + VXLAN 8 + inner Ethernet 14)` and its bytes.
    pub breakdown: Vec<(String, u32)>,
}

/// An encapsulation [`mtu`] knows, for building a picker.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Encapsulation {
    /// The name to pass to [`mtu`].
    pub name: String,
    /// Its overhead in bytes on a 1500-byte link with an IPv4 outer header.
    pub overhead: u32,
    /// What the overhead consists of, and the options the name accepts.
    pub description: String,
}

const IPV4_HEADER: u32 = 20;
const IPV6_HEADER: u32 = 40;
const TCP_HEADER: u32 = 20;
const UDP_HEADER: u32 = 8;
const ESP_HEADER: u32 = 8;
/// ESP's pad-length and next-header bytes.
const ESP_TRAILER: u32 = 2;

/// The deepest stack of one encapsulation that `name:count` may ask for.
const MAX_REPEAT: u32 = 64;

/// How an encapsulation consumes MTU.
#[derive(Clone, Copy)]
enum Cost {
    /// Headers of a fixed size without an IP header of their own.
    Plain(u32),
    /// A new outer IP header plus this many bytes.
    Tunnel(u32),
    /// IPsec ESP in tunnel mode.
    Esp,
}

struct Kind {
    name: &'static str,
    cost: Cost,
    /// The parts of the overhead other than the outer IP header.
    parts: &'static str,
    note: &'static str,
}

const KINDS: [Kind; 13] = [
    Kind {
        name: "dot1q",
        cost: Cost::Plain(4),
        parts: "802.1Q tag 4",
        note: "Only costs IP MTU where the frame size is fixed, e.g. inside a pseudowire or VXLAN.",
    },
    Kind {
        name: "qinq",
        cost: Cost::Plain(8),
        parts: "802.1ad S-tag 4 + C-tag 4",
        note: "As dot1q, with two tags.",
    },
    Kind {
        name: "mpls",
        cost: Cost::Plain(4),
        parts: "label 4",
        note: "One label; repeat it or write `mpls:3` for a label stack.",
    },
    Kind {
        name: "pppoe",
        cost: Cost::Plain(8),
        parts: "PPPoE 6 + PPP 2",
        note: "",
    },
    Kind {
        name: "gre",
        cost: Cost::Tunnel(4),
        parts: "GRE 4",
        note: "",
    },
    Kind {
        name: "gre-key",
        cost: Cost::Tunnel(8),
        parts: "GRE 4 + key 4",
        note: "",
    },
    Kind {
        name: "ipip",
        cost: Cost::Tunnel(0),
        parts: "",
        note: "IP-in-IP (protocol 4).",
    },
    Kind {
        name: "6in4",
        cost: Cost::Plain(IPV4_HEADER),
        parts: "IPv4 20",
        note: "IPv6 in IPv4 (protocol 41), as used by 6to4 and 6rd.",
    },
    Kind {
        name: "vxlan",
        cost: Cost::Tunnel(UDP_HEADER + 8 + 14),
        parts: "UDP 8 + VXLAN 8 + inner Ethernet 14",
        note: "Add `dot1q` if the inner frame keeps its VLAN tag.",
    },
    Kind {
        name: "geneve",
        cost: Cost::Tunnel(UDP_HEADER + 8 + 14),
        parts: "UDP 8 + Geneve 8 + inner Ethernet 14",
        note: "Without options; Geneve options add 4 bytes or more each.",
    },
    Kind {
        name: "l2tp",
        cost: Cost::Tunnel(UDP_HEADER + 8 + 4),
        parts: "UDP 8 + L2TP 8 + PPP 4",
        note: "L2TPv2 with length field; an estimate, the header has optional fields.",
    },
    Kind {
        name: "wireguard",
        cost: Cost::Tunnel(UDP_HEADER + 32),
        parts: "UDP 8 + WireGuard 32",
        note: "Message header 16 + Poly1305 tag 16.",
    },
    Kind {
        name: "ipsec-esp-tunnel",
        cost: Cost::Esp,
        parts: "",
        note: "ESP tunnel mode; an estimate that includes padding to the cipher block. Options: aes-gcm (default), \
               aes-cbc-sha1, aes-cbc-sha256, aes-cbc-sha384, aes-cbc-sha512, chacha20-poly1305, 3des-sha1, nat-t.",
    },
];

/// An ESP transform: sizes of the IV and the integrity check value, and the
/// alignment the encrypted payload is padded to.
struct Transform {
    names: &'static [&'static str],
    label: &'static str,
    iv: u32,
    icv: u32,
    block: u32,
}

const TRANSFORMS: [Transform; 7] = [
    Transform {
        names: &["aes-gcm", "gcm", "aes-gcm-16"],
        label: "AES-GCM",
        iv: 8,
        icv: 16,
        block: 4,
    },
    Transform {
        names: &["aes-cbc-sha1", "aes-cbc-sha", "cbc", "aes-cbc"],
        label: "AES-CBC + HMAC-SHA1-96",
        iv: 16,
        icv: 12,
        block: 16,
    },
    Transform {
        names: &["aes-cbc-sha256"],
        label: "AES-CBC + HMAC-SHA-256-128",
        iv: 16,
        icv: 16,
        block: 16,
    },
    Transform {
        names: &["aes-cbc-sha384"],
        label: "AES-CBC + HMAC-SHA-384-192",
        iv: 16,
        icv: 24,
        block: 16,
    },
    Transform {
        names: &["aes-cbc-sha512"],
        label: "AES-CBC + HMAC-SHA-512-256",
        iv: 16,
        icv: 32,
        block: 16,
    },
    Transform {
        names: &["chacha20-poly1305", "chacha20"],
        label: "ChaCha20-Poly1305",
        iv: 8,
        icv: 16,
        block: 4,
    },
    Transform {
        names: &["3des-sha1", "3des"],
        label: "3DES-CBC + HMAC-SHA1-96",
        iv: 8,
        icv: 12,
        block: 8,
    },
];

/// Work out the MTU that is left inside a stack of encapsulations.
///
/// `link_mtu` is the IP MTU of the underlying link (1500 on plain
/// Ethernet). `encapsulations` lists what is stacked on it, outermost first:
/// `dot1q`, `qinq`, `mpls` (one label each), `gre`, `gre-key`,
/// `ipsec-esp-tunnel`, `vxlan`, `geneve`, `pppoe`, `l2tp`, `wireguard`,
/// `ipip`, `6in4`. Names are case-insensitive and take options after a colon:
///
/// * a number repeats the encapsulation (`mpls:3`);
/// * `v6` gives a tunnel an IPv6 outer header (`vxlan:v6`, `wireguard:v6`);
/// * `ipsec-esp-tunnel` takes a transform (`aes-gcm`, the default, or
///   `aes-cbc-sha1`, `aes-cbc-sha256`, ...) and `nat-t`, e.g.
///   `ipsec-esp-tunnel:aes-cbc-sha256,nat-t`.
///
/// See [`encapsulations`] for the list with every overhead.
pub fn mtu(link_mtu: u32, encapsulations: &[String]) -> Result<MtuResult, ToolError> {
    let mut left = link_mtu;
    let mut breakdown = Vec::new();
    for entry in encapsulations
        .iter()
        .filter(|entry| !entry.trim().is_empty())
    {
        let layer = Layer::parse(entry)?;
        for _ in 0..layer.repeat {
            let (label, cost) = layer.apply(left).ok_or_else(|| {
                ToolError::DoesNotFit(format!(
                    "`{}` does not fit: only {left} bytes of the {link_mtu}-byte link MTU are left for it",
                    layer.kind.name
                ))
            })?;
            breakdown.push((label, cost));
            left -= cost;
        }
    }
    Ok(MtuResult {
        payload_mtu: left,
        tcp_mss_v4: left.saturating_sub(IPV4_HEADER + TCP_HEADER),
        tcp_mss_v6: left.saturating_sub(IPV6_HEADER + TCP_HEADER),
        overhead: link_mtu - left,
        breakdown,
    })
}

/// The encapsulations [`mtu`] understands, with their overhead on a
/// 1500-byte link.
pub fn encapsulations() -> Vec<Encapsulation> {
    KINDS
        .iter()
        .map(|kind| {
            let layer = Layer {
                kind,
                repeat: 1,
                outer_v6: false,
                transform: &TRANSFORMS[0],
                nat_t: false,
            };
            let (label, overhead) = layer.apply(1500).unwrap_or_default();
            let parts = label
                .split_once(" (")
                .map_or("", |(_, parts)| parts.trim_end_matches(')'));
            let description = if kind.note.is_empty() {
                format!("{parts}.")
            } else {
                format!("{parts}. {}", kind.note)
            };
            Encapsulation {
                name: kind.name.to_owned(),
                overhead,
                description,
            }
        })
        .collect()
}

/// One entry of the encapsulation list, with its options.
struct Layer {
    kind: &'static Kind,
    repeat: u32,
    outer_v6: bool,
    transform: &'static Transform,
    nat_t: bool,
}

impl Layer {
    fn parse(entry: &str) -> Result<Self, ToolError> {
        const WHAT: &str = "encapsulation";
        let text = entry.trim().to_ascii_lowercase();
        let (name, options) = text.split_once(':').unwrap_or((&text, ""));
        let name = name.trim();
        // `ipsec-esp-tunnel-aes-cbc-sha1` is as good as `ipsec-esp-tunnel:aes-cbc-sha1`.
        let (name, inline_option) = match name.strip_prefix("ipsec-esp-tunnel-") {
            Some(transform) => ("ipsec-esp-tunnel", transform),
            None => (name, ""),
        };
        let Some(kind) = KINDS.iter().find(|kind| kind.name == name) else {
            let known: Vec<&str> = KINDS.iter().map(|kind| kind.name).collect();
            return Err(ToolError::invalid(
                WHAT,
                entry.trim(),
                format!("unknown; the supported ones are {}", known.join(", ")),
            ));
        };

        let mut layer = Layer {
            kind,
            repeat: 1,
            outer_v6: false,
            transform: &TRANSFORMS[0],
            nat_t: false,
        };
        for option in options
            .split(',')
            .chain([inline_option])
            .map(str::trim)
            .filter(|option| !option.is_empty())
        {
            let is_esp = matches!(kind.cost, Cost::Esp);
            if let Ok(count) = option.parse::<u32>() {
                if !(1..=MAX_REPEAT).contains(&count) {
                    return Err(ToolError::invalid(
                        WHAT,
                        entry.trim(),
                        format!("a repeat count must be between 1 and {MAX_REPEAT}"),
                    ));
                }
                layer.repeat = count;
            } else if matches!(option, "v6" | "ipv6") && !matches!(kind.cost, Cost::Plain(_)) {
                layer.outer_v6 = true;
            } else if option == "nat-t" && is_esp {
                layer.nat_t = true;
            } else if let Some(transform) = TRANSFORMS
                .iter()
                .find(|transform| transform.names.contains(&option))
                .filter(|_| is_esp)
            {
                layer.transform = transform;
            } else {
                let allowed = match kind.cost {
                    Cost::Plain(_) => "a repeat count".to_owned(),
                    Cost::Tunnel(_) => "a repeat count or `v6` for an IPv6 outer header".to_owned(),
                    Cost::Esp => format!(
                        "`v6`, `nat-t` or a transform ({})",
                        TRANSFORMS
                            .iter()
                            .map(|transform| transform.names[0])
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                };
                return Err(ToolError::invalid(
                    WHAT,
                    entry.trim(),
                    format!(
                        "`{}` is not an option of {}; it takes {allowed}",
                        snip(option, 30),
                        kind.name
                    ),
                ));
            }
        }
        Ok(layer)
    }

    /// The label and cost of this layer when `left` bytes are available, or
    /// `None` if it does not fit (an encapsulation must leave at least a byte).
    fn apply(&self, left: u32) -> Option<(String, u32)> {
        let name = self.kind.name;
        let (outer_label, outer) = if self.outer_v6 {
            ("IPv6 40", IPV6_HEADER)
        } else {
            ("IPv4 20", IPV4_HEADER)
        };
        let (label, cost) = match self.kind.cost {
            Cost::Plain(bytes) => (format!("{name} ({})", self.kind.parts), bytes),
            Cost::Tunnel(bytes) if self.kind.parts.is_empty() => {
                (format!("{name} ({outer_label})"), outer + bytes)
            }
            Cost::Tunnel(bytes) => (
                format!("{name} ({outer_label} + {})", self.kind.parts),
                outer + bytes,
            ),
            Cost::Esp => {
                let Transform {
                    label,
                    iv,
                    icv,
                    block,
                    ..
                } = *self.transform;
                let nat_t = if self.nat_t { UDP_HEADER } else { 0 };
                // What is left for the encrypted part, which is padded so that
                // payload + pad length + next header fill whole blocks.
                let encrypted =
                    left.checked_sub(outer + nat_t + ESP_HEADER + iv + icv)? / block * block;
                let payload = encrypted.checked_sub(ESP_TRAILER)?;
                let cost = left - payload;
                let padding = cost - (outer + nat_t + ESP_HEADER + iv + icv);
                let udp = if self.nat_t { " + NAT-T UDP 8" } else { "" };
                (
                    format!(
                        "{name} ({label}: {outer_label}{udp} + ESP 8 + IV {iv} + padding and trailer {padding} + ICV {icv})"
                    ),
                    cost,
                )
            }
        };
        (cost < left).then_some((label, cost))
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn stack(link_mtu: u32, names: &[&str]) -> MtuResult {
        mtu(
            link_mtu,
            &names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn payload(link_mtu: u32, names: &[&str]) -> u32 {
        stack(link_mtu, names).payload_mtu
    }

    #[test]
    fn no_encapsulation() {
        let plain = stack(1500, &[]);
        assert_eq!(
            plain,
            MtuResult {
                payload_mtu: 1500,
                tcp_mss_v4: 1460,
                tcp_mss_v6: 1440,
                overhead: 0,
                breakdown: vec![]
            }
        );
        assert_eq!(stack(9000, &["", "  "]).payload_mtu, 9000);
    }

    #[test]
    fn well_known_overheads() {
        // The numbers every network engineer carries around.
        assert_eq!(payload(1500, &["pppoe"]), 1492);
        assert_eq!(payload(1500, &["gre"]), 1476);
        assert_eq!(payload(1500, &["gre-key"]), 1472);
        assert_eq!(payload(1500, &["ipip"]), 1480);
        assert_eq!(payload(1500, &["6in4"]), 1480);
        assert_eq!(payload(1500, &["vxlan"]), 1450);
        assert_eq!(payload(1500, &["geneve"]), 1450);
        assert_eq!(payload(1500, &["wireguard"]), 1440);
        assert_eq!(payload(1500, &["wireguard:v6"]), 1420);
        assert_eq!(payload(1500, &["l2tp"]), 1460);
        assert_eq!(payload(1500, &["dot1q"]), 1496);
        assert_eq!(payload(1500, &["qinq"]), 1492);
        assert_eq!(payload(1500, &["mpls"]), 1496);
        assert_eq!(payload(1500, &["mpls", "mpls"]), 1492);
        assert_eq!(payload(1500, &["mpls:3"]), 1488);
        assert_eq!(payload(1500, &["vxlan:v6"]), 1430);
        assert_eq!(payload(1500, &["gre:v6"]), 1456);
        assert_eq!(payload(9216, &["vxlan"]), 9166);
    }

    #[test]
    fn esp_tunnel_mode_depends_on_the_transform() {
        // AES-GCM: 1500 - 20 - 8 (ESP) - 8 (IV) - 16 (ICV) = 1448, a multiple of 4; minus 2 trailer bytes.
        assert_eq!(payload(1500, &["ipsec-esp-tunnel"]), 1446);
        assert_eq!(payload(1500, &["ipsec-esp-tunnel:aes-gcm"]), 1446);
        // AES-CBC + SHA-1: 1500 - 20 - 8 - 16 - 12 = 1444 -> 1440 (90 blocks of 16) - 2.
        assert_eq!(payload(1500, &["ipsec-esp-tunnel:aes-cbc-sha1"]), 1438);
        assert_eq!(payload(1500, &["ipsec-esp-tunnel-aes-cbc-sha"]), 1438);
        // AES-CBC + SHA-256: 1500 - 20 - 8 - 16 - 16 = 1440 -> 1440 - 2.
        assert_eq!(payload(1500, &["ipsec-esp-tunnel:aes-cbc-sha256"]), 1438);
        // AES-CBC + SHA-512: 1500 - 20 - 8 - 16 - 32 = 1424 -> 1424 - 2.
        assert_eq!(payload(1500, &["ipsec-esp-tunnel:aes-cbc-sha512"]), 1422);
        // NAT traversal adds a UDP header: 1500 - 20 - 8 - 8 - 8 - 16 = 1440 -> 1438.
        assert_eq!(payload(1500, &["ipsec-esp-tunnel:nat-t"]), 1438);
        assert_eq!(
            payload(1500, &["ipsec-esp-tunnel:aes-cbc-sha256,nat-t"]),
            1422
        );
        assert_eq!(payload(1500, &["ipsec-esp-tunnel:v6"]), 1426);
        assert_eq!(payload(1500, &["ipsec-esp-tunnel:3des"]), 1446);

        let gcm = stack(1500, &["ipsec-esp-tunnel"]);
        assert_eq!(gcm.breakdown, [(
            "ipsec-esp-tunnel (AES-GCM: IPv4 20 + ESP 8 + IV 8 + padding and trailer 2 + ICV 16)".to_owned(),
            54
        )]);
        let cbc = stack(1500, &["ipsec-esp-tunnel:aes-cbc-sha1,nat-t"]);
        assert_eq!(cbc.breakdown, [(
            "ipsec-esp-tunnel (AES-CBC + HMAC-SHA1-96: IPv4 20 + NAT-T UDP 8 + ESP 8 + IV 16 + padding and trailer 14 + ICV 12)"
                .to_owned(),
            78
        )]);
        assert_eq!(cbc.payload_mtu, 1422);
    }

    #[test]
    fn esp_padding_follows_what_is_left() {
        // Every inner packet up to the result fits, and one byte more does not.
        for link in 100..1600 {
            for transform in &TRANSFORMS {
                let result = payload(link, &[&format!("ipsec-esp-tunnel:{}", transform.names[0])]);
                let wire = |inner: u32| {
                    20 + 8
                        + transform.iv
                        + (inner + 2).div_ceil(transform.block) * transform.block
                        + transform.icv
                };
                assert!(wire(result) <= link, "{link} {}", transform.label);
                assert!(wire(result + 1) > link, "{link} {}", transform.label);
            }
        }
    }

    #[test]
    fn stacks_add_up_in_order() {
        // GRE over IPsec over PPPoE (a classic DSL branch office).
        let dsl = stack(1500, &["pppoe", "ipsec-esp-tunnel:aes-cbc-sha1", "gre"]);
        assert_eq!(dsl.breakdown, [
            ("pppoe (PPPoE 6 + PPP 2)".to_owned(), 8),
            // 1492 - 56 bytes of headers = 1436, of which 89 blocks of 16 = 1424 can be encrypted.
            ("ipsec-esp-tunnel (AES-CBC + HMAC-SHA1-96: IPv4 20 + ESP 8 + IV 16 + padding and trailer 14 + ICV 12)".to_owned(), 70),
            ("gre (IPv4 20 + GRE 4)".to_owned(), 24),
        ]);
        assert_eq!(
            (
                dsl.payload_mtu,
                dsl.overhead,
                dsl.tcp_mss_v4,
                dsl.tcp_mss_v6
            ),
            (1398, 102, 1358, 1338)
        );
        assert_eq!(
            dsl.breakdown.iter().map(|(_, bytes)| bytes).sum::<u32>(),
            dsl.overhead
        );

        // VXLAN in an MPLS L3VPN with two labels.
        let dc = stack(9000, &["mpls:2", "vxlan", "dot1q"]);
        assert_eq!((dc.payload_mtu, dc.overhead), (8938, 62));
        assert_eq!(dc.breakdown.len(), 4);
        assert_eq!(
            dc.breakdown[2].0,
            "vxlan (IPv4 20 + UDP 8 + VXLAN 8 + inner Ethernet 14)"
        );
    }

    #[test]
    fn names_are_forgiving() {
        assert_eq!(payload(1500, &[" VXLAN "]), 1450);
        assert_eq!(payload(1500, &["GRE-Key"]), 1472);
        assert_eq!(payload(1500, &["mpls: 2"]), 1492);
        assert_eq!(payload(1500, &["IPsec-ESP-Tunnel:AES-GCM, NAT-T"]), 1438);
    }

    #[test]
    fn rejections_say_what_is_wrong() {
        let message = |link: u32, names: &[&str]| {
            mtu(
                link,
                &names
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect::<Vec<_>>(),
            )
            .unwrap_err()
            .to_string()
        };
        assert_eq!(
            message(1500, &["vlan"]),
            "invalid encapsulation `vlan`: unknown; the supported ones are dot1q, qinq, mpls, pppoe, gre, gre-key, ipip, \
             6in4, vxlan, geneve, l2tp, wireguard, ipsec-esp-tunnel"
        );
        assert_eq!(
            message(1500, &["mpls:0"]),
            "invalid encapsulation `mpls:0`: a repeat count must be between 1 and 64"
        );
        assert_eq!(
            message(u32::MAX, &["mpls:4000000000"]),
            "invalid encapsulation `mpls:4000000000`: a repeat count must be between 1 and 64"
        );
        assert_eq!(
            message(1500, &["mpls:v6"]),
            "invalid encapsulation `mpls:v6`: `v6` is not an option of mpls; it takes a repeat count"
        );
        assert_eq!(
            message(1500, &["gre:aes-gcm"]),
            "invalid encapsulation `gre:aes-gcm`: `aes-gcm` is not an option of gre; it takes a repeat count or `v6` for an IPv6 outer header"
        );
        assert!(
            message(1500, &["ipsec-esp-tunnel:rot13"])
                .contains("it takes `v6`, `nat-t` or a transform (aes-gcm, aes-cbc-sha1,")
        );
        assert_eq!(
            message(40, &["vxlan"]),
            "`vxlan` does not fit: only 40 bytes of the 40-byte link MTU are left for it"
        );
        assert_eq!(
            message(50, &["vxlan"]),
            "`vxlan` does not fit: only 50 bytes of the 50-byte link MTU are left for it"
        );
        assert_eq!(
            message(100, &["vxlan", "vxlan"]),
            "`vxlan` does not fit: only 50 bytes of the 100-byte link MTU are left for it"
        );
        assert_eq!(
            message(0, &["dot1q"]),
            "`dot1q` does not fit: only 0 bytes of the 0-byte link MTU are left for it"
        );
        assert!(message(50, &["ipsec-esp-tunnel"]).contains("does not fit"));
        assert_eq!(
            message(100, &["mpls:64"]),
            "`mpls` does not fit: only 4 bytes of the 100-byte link MTU are left for it"
        );
    }

    #[test]
    fn small_and_huge_links_do_not_break() {
        assert_eq!(stack(0, &[]).payload_mtu, 0);
        let tiny = stack(30, &[]);
        assert_eq!((tiny.tcp_mss_v4, tiny.tcp_mss_v6), (0, 0));
        assert_eq!(payload(51, &["vxlan"]), 1);
        let huge = stack(u32::MAX, &["vxlan", "ipsec-esp-tunnel"]);
        assert_eq!(huge.overhead, u32::MAX - huge.payload_mtu);
    }

    #[test]
    fn the_catalogue_lists_every_encapsulation() {
        let catalogue = encapsulations();
        let overheads: Vec<(&str, u32)> = catalogue
            .iter()
            .map(|entry| (entry.name.as_str(), entry.overhead))
            .collect();
        assert_eq!(
            overheads,
            [
                ("dot1q", 4),
                ("qinq", 8),
                ("mpls", 4),
                ("pppoe", 8),
                ("gre", 24),
                ("gre-key", 28),
                ("ipip", 20),
                ("6in4", 20),
                ("vxlan", 50),
                ("geneve", 50),
                ("l2tp", 40),
                ("wireguard", 60),
                ("ipsec-esp-tunnel", 54),
            ]
        );
        for entry in &catalogue {
            assert_eq!(
                payload(1500, &[&entry.name]),
                1500 - entry.overhead,
                "{}",
                entry.name
            );
            assert!(
                entry.description.ends_with('.') && !entry.description.starts_with('.'),
                "{}: {}",
                entry.name,
                entry.description
            );
        }
        assert_eq!(
            catalogue[8].description,
            "IPv4 20 + UDP 8 + VXLAN 8 + inner Ethernet 14. Add `dot1q` if the inner frame keeps its VLAN tag."
        );
    }
}
