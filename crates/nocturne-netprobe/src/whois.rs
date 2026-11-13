//! WHOIS (RFC 3912): who an address, an AS number or a domain is registered to.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use hickory_resolver::proto::rr::Name;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::util;
use crate::{ProbeError, dns};

/// The outcome of a WHOIS query.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct WhoisAnswer {
    /// The query as it was sent (trimmed; AS numbers and domains normalised).
    pub query: String,
    /// The servers that answered, in the order they were asked.
    pub servers: Vec<String>,
    /// The answer of the last server, which is the most specific one.
    pub text: String,
    /// The essentials, picked out of the answers: `netname`, `org`, `country`, `cidr`, `range`,
    /// `abuse`, `asn`, `as_name`, and for domains `domain`, `registrar`, `created`, `expires`,
    /// `name_servers`. Only what was found is listed.
    pub summary: Vec<(String, String)>,
}

/// Where every query starts: IANA knows which registry is responsible.
const ROOT_SERVER: &str = "whois.iana.org";
const WHOIS_PORT: u16 = 43;
/// How many referrals are followed after the first server.
const MAX_REFERRALS: usize = 4;
/// More than any registry sends; a runaway server is cut off here.
const MAX_RESPONSE: usize = 512 * 1024;

/// Looks up an IP address (or CIDR block), an AS number (`AS13335`) or a domain name.
///
/// The query starts at `whois.iana.org` and follows the `refer:` / `ReferralServer:` chain to
/// the responsible registry (and, for domains, on to the registrar), at most four referrals.
/// `timeout` bounds the whole chain; if a later server does not answer in time, the answer so
/// far is returned.
pub async fn whois(query: &str, timeout: Duration) -> Result<WhoisAnswer, ProbeError> {
    whois_at(query, ROOT_SERVER, timeout).await
}

/// Like [`whois`], but starts at `server` (a host or address, optionally with `:port`)
/// instead of IANA — to ask one particular registry directly.
pub async fn whois_at(
    query: &str,
    server: &str,
    timeout: Duration,
) -> Result<WhoisAnswer, ProbeError> {
    let deadline = Instant::now() + timeout;
    let query = normalize_query(query)?;
    let mut next = Some(parse_server(server).ok_or_else(|| {
        ProbeError::invalid(format!(
            "'{server}' is not a WHOIS server (expected host or host:port)"
        ))
    })?);
    let mut servers: Vec<String> = Vec::new();
    let mut answers: Vec<String> = Vec::new();
    let mut first_error = None;

    while let Some((host, port)) = next.take() {
        let label = server_label(&host, port);
        let remaining = deadline.saturating_duration_since(Instant::now());
        let line = query_for(&host, &query);
        let answer = match tokio::time::timeout(remaining, ask(&host, port, &line)).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(error)) => {
                first_error.get_or_insert(error);
                break;
            }
            Err(_) => {
                first_error.get_or_insert(ProbeError::Timeout);
                break;
            }
        };
        servers.push(label);
        if servers.len() <= MAX_REFERRALS {
            next = referral(&answer).filter(|(host, port)| {
                // Registries sometimes refer to themselves; never ask a server twice.
                !servers.contains(&server_label(host, *port))
            });
        }
        if !answer.trim().is_empty() {
            answers.push(answer);
        }
    }

    let Some(text) = answers.last().cloned() else {
        return Err(first_error.unwrap_or_else(|| {
            ProbeError::protocol(format!(
                "the WHOIS server {} sent an empty answer",
                servers.join(", ")
            ))
        }));
    };
    Ok(WhoisAnswer {
        summary: summarize(&answers),
        query: query.text,
        servers,
        text,
    })
}

/// What is being looked up decides how some registries want to be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Address,
    AsNumber,
    Domain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Query {
    text: String,
    kind: Kind,
}

fn normalize_query(query: &str) -> Result<Query, ProbeError> {
    let query = query.trim();
    if query.is_empty() {
        return Err(ProbeError::invalid(
            "the WHOIS query is empty; enter an IP address, an AS number (AS13335) or a domain name",
        ));
    }
    if query.len() > 255 || query.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ProbeError::invalid(format!(
            "'{query}' is not something WHOIS can look up; enter one IP address, AS number or domain name"
        )));
    }
    // An address, with or without a prefix length.
    let address_part = query.split_once('/').map_or(query, |(address, _)| address);
    if util::parse_ip_literal(address_part).is_some() {
        if let Some((_, prefix)) = query.split_once('/') {
            if prefix.parse::<u8>().is_err() {
                return Err(ProbeError::invalid(format!(
                    "'{query}' is not a CIDR block: '{prefix}' is not a prefix length"
                )));
            }
        }
        let text = query.trim_matches(['[', ']']).to_owned();
        return Ok(Query {
            text,
            kind: Kind::Address,
        });
    }
    // AS numbers: "AS13335", "as13335", "ASN13335".
    let upper = query.to_ascii_uppercase();
    let digits = upper
        .strip_prefix("ASN")
        .or_else(|| upper.strip_prefix("AS"));
    if let Some(number) = digits.and_then(|digits| digits.parse::<u32>().ok()) {
        return Ok(Query {
            text: format!("AS{number}"),
            kind: Kind::AsNumber,
        });
    }
    // Everything else is a domain name; registries want the ASCII (punycode) form.
    let name = Name::from_str_relaxed(query.trim_end_matches('.')).map(|name| name.to_ascii());
    match name {
        Ok(name) if !name.is_empty() && !name.starts_with('-') => Ok(Query {
            text: name.trim_end_matches('.').to_ascii_lowercase(),
            kind: Kind::Domain,
        }),
        _ => Err(ProbeError::invalid(format!(
            "'{query}' is not an IP address, an AS number or a domain name"
        ))),
    }
}

/// The exact line to send to `host`. A few registries need flags to give a useful answer.
fn query_for(host: &str, query: &Query) -> String {
    let text = &query.text;
    match (host, &query.kind) {
        // "n" restricts to networks, "+" asks for the full records rather than a list.
        ("whois.arin.net", Kind::Address) => {
            let address = text
                .split_once('/')
                .map_or(text.as_str(), |(address, _)| address);
            format!("n + {address}")
        }
        // Without the keyword a name also matches name servers and registrars.
        ("whois.verisign-grs.com" | "whois.crsnic.net", Kind::Domain) => format!("domain {text}"),
        ("whois.denic.de", Kind::Domain) => format!("-T dn,ace {text}"),
        // "/e" asks the Japanese registry for English output.
        ("whois.jprs.jp", Kind::Domain) => format!("{text}/e"),
        // IANA indexes blocks by address, not by prefix.
        (ROOT_SERVER, Kind::Address) => text
            .split_once('/')
            .map_or(text.as_str(), |(address, _)| address)
            .to_owned(),
        _ => text.clone(),
    }
}

/// One query / response exchange.
async fn ask(host: &str, port: u16, line: &str) -> Result<String, ProbeError> {
    let address = dns::resolve_one(host).await?;
    let mut stream = TcpStream::connect(SocketAddr::new(address, port))
        .await
        .map_err(ProbeError::from_network_io)?;
    stream.write_all(format!("{line}\r\n").as_bytes()).await?;
    let mut answer = Vec::new();
    let mut chunk = [0u8; 8192];
    while answer.len() < MAX_RESPONSE {
        match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(count) => answer.extend_from_slice(&chunk[..count]),
            // Some servers reset instead of closing once they have said everything.
            Err(_) if !answer.is_empty() => break,
            Err(error) => return Err(ProbeError::from_network_io(error)),
        }
    }
    answer.truncate(MAX_RESPONSE);
    Ok(String::from_utf8_lossy(&answer).replace("\r\n", "\n"))
}

fn server_label(host: &str, port: u16) -> String {
    if port == WHOIS_PORT {
        host.to_owned()
    } else if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// `whois.ripe.net`, `whois://whois.ripe.net:43/`, `192.0.2.1:4343`. `None` for anything that
/// is not a WHOIS server (`rwhois://…`, `https://…`).
fn parse_server(text: &str) -> Option<(String, u16)> {
    let text = text.trim().trim_end_matches('/');
    let text = match text.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("whois") => rest,
        Some(_) => return None,
        None => text,
    };
    let (host, port) = util::split_host_port(text, WHOIS_PORT).ok()?;
    Some((host.to_ascii_lowercase(), port))
}

/// The server an answer refers on to, if any.
fn referral(answer: &str) -> Option<(String, u16)> {
    // In order of authority: IANA's and ARIN's explicit referrals, then the registry's
    // pointer to the registrar.
    const KEYS: [&str; 5] = [
        "refer",
        "referralserver",
        "registrar whois server",
        "whois server",
        "whois",
    ];
    KEYS.iter().find_map(|wanted| {
        fields(answer)
            .filter(|(key, _)| key.eq_ignore_ascii_case(wanted))
            .find_map(|(_, value)| parse_server(value))
    })
}

/// The `key: value` lines of an answer (comments and free text are skipped).
fn fields(answer: &str) -> impl Iterator<Item = (&str, &str)> {
    answer.lines().filter_map(|line| {
        let line = line.trim();
        if line.starts_with(['%', '#', ';']) {
            return None;
        }
        let (key, value) = line.split_once(':')?;
        let (key, value) = (key.trim(), value.trim());
        (!key.is_empty() && !value.is_empty() && key.len() <= 40).then_some((key, value))
    })
}

/// Which field names of the various registries mean the same thing.
const SUMMARY_KEYS: [(&str, &[&str]); 13] = [
    ("netname", &["netname", "network-name"]),
    (
        "org",
        &[
            "org-name",
            "orgname",
            "owner",
            "organization",
            "registrant organization",
            "registrant",
            "descr",
        ],
    ),
    ("country", &["country", "registrant country"]),
    ("cidr", &["cidr", "route", "route6"]),
    ("range", &["inetnum", "inet6num", "netrange"]),
    (
        "abuse",
        &[
            "orgabuseemail",
            "abuse-mailbox",
            "registrar abuse contact email",
        ],
    ),
    ("asn", &["aut-num", "asnumber", "originas", "origin"]),
    ("as_name", &["as-name", "asname"]),
    ("domain", &["domain name", "domain"]),
    ("registrar", &["registrar", "sponsoring registrar"]),
    (
        "created",
        &["creation date", "created", "regdate", "registered"],
    ),
    (
        "expires",
        &[
            "registry expiry date",
            "registrar registration expiration date",
            "expiry date",
            "expiration date",
            "paid-till",
            "expires",
        ],
    ),
    ("name_servers", &["name server", "nserver"]),
];

/// Picks the essentials out of the answers, the most specific answer first.
fn summarize(answers: &[String]) -> Vec<(String, String)> {
    let mut summary: Vec<(String, String)> = Vec::new();
    for (name, aliases) in SUMMARY_KEYS {
        let found = answers.iter().rev().find_map(|answer| {
            // ARIN lists the parent networks before the one asked about: its last record
            // counts. Everyone else puts the most specific object first.
            let arin_style = answer.contains("NetRange:") || answer.contains("ASNumber:");
            // Aliases are in order of preference.
            aliases.iter().find_map(|alias| {
                let mut values = fields(answer)
                    .filter(|(key, _)| key.eq_ignore_ascii_case(alias))
                    .map(|(_, value)| value);
                if name == "name_servers" {
                    let mut servers: Vec<String> = values
                        .map(|server| first_word(server).to_lowercase())
                        .collect();
                    servers.dedup();
                    return (!servers.is_empty()).then(|| servers.join(", "));
                }
                let value = if arin_style {
                    values.last()
                } else {
                    values.next()
                };
                value.map(str::to_owned)
            })
        });
        if let Some(value) = found {
            summary.push((name.to_owned(), value));
        }
    }
    // RIPE-style registries publish the abuse contact as a comment line.
    if !summary.iter().any(|(name, _)| name == "abuse") {
        let contact = answers.iter().rev().find_map(|answer| {
            answer
                .lines()
                .find(|line| line.contains("Abuse contact for"))
                .and_then(|line| line.rsplit('\'').nth(1))
                .filter(|contact| contact.contains('@'))
        });
        if let Some(contact) = contact {
            let position = summary
                .iter()
                .position(|(name, _)| ["asn", "as_name", "domain"].contains(&name.as_str()))
                .unwrap_or(summary.len());
            summary.insert(position, ("abuse".to_owned(), contact.to_owned()));
        }
    }
    // A registry that gives only "first - last" still describes CIDR blocks.
    if !summary.iter().any(|(name, _)| name == "cidr") {
        let range = summary.iter().position(|(name, _)| name == "range");
        if let Some(cidr) = range.and_then(|index| range_to_cidr(&summary[index].1)) {
            summary.insert(range.unwrap_or(0), ("cidr".to_owned(), cidr));
        }
    }
    summary
}

fn first_word(text: &str) -> &str {
    text.split_whitespace().next().unwrap_or(text)
}

/// `193.0.0.0 - 193.0.7.255` as `193.0.0.0/21`; a range that is not one block becomes several.
/// An `inet6num` is already a prefix and is returned as is.
fn range_to_cidr(range: &str) -> Option<String> {
    let Some((first, last)) = range.split_once('-') else {
        return range.contains('/').then(|| range.trim().to_owned());
    };
    let first: Ipv4Addr = first.trim().parse().ok()?;
    let last: Ipv4Addr = last.trim().parse().ok()?;
    let (mut start, end) = (u64::from(u32::from(first)), u64::from(u32::from(last)));
    if start > end {
        return None;
    }
    let mut blocks = Vec::new();
    while start <= end && blocks.len() < 32 {
        // The largest block that starts here: limited by alignment and by what is left.
        let alignment = if start == 0 {
            32
        } else {
            start.trailing_zeros().min(32)
        };
        let fits = (end - start + 1).ilog2();
        let size_bits = alignment.min(fits);
        blocks.push(format!(
            "{}/{}",
            Ipv4Addr::from(start as u32),
            32 - size_bits
        ));
        start += 1u64 << size_bits;
    }
    Some(blocks.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tokio::net::TcpListener;

    fn address(text: &str) -> Query {
        Query {
            text: text.into(),
            kind: Kind::Address,
        }
    }

    #[test]
    fn queries_are_classified_and_normalised() {
        assert_eq!(normalize_query(" 8.8.8.8 ").unwrap(), address("8.8.8.8"));
        assert_eq!(
            normalize_query("193.0.0.0/21").unwrap(),
            address("193.0.0.0/21")
        );
        assert_eq!(
            normalize_query("2001:4860:4860::8888").unwrap(),
            address("2001:4860:4860::8888")
        );
        assert_eq!(
            normalize_query("[2001:db8::1]").unwrap(),
            address("2001:db8::1")
        );
        for asn in ["AS13335", "as13335", "ASN13335", "asn13335"] {
            assert_eq!(
                normalize_query(asn).unwrap(),
                Query {
                    text: "AS13335".into(),
                    kind: Kind::AsNumber
                }
            );
        }
        assert_eq!(
            normalize_query("Example.COM.").unwrap(),
            Query {
                text: "example.com".into(),
                kind: Kind::Domain
            }
        );
        assert_eq!(
            normalize_query("bücher.example").unwrap().text,
            "xn--bcher-kva.example"
        );
        // "AS" followed by anything but a number is a name.
        assert_eq!(normalize_query("asus.com").unwrap().kind, Kind::Domain);
        for bad in ["", "   ", "two words", "8.8.8.8/abc", "a\tb", "-h"] {
            assert!(
                matches!(normalize_query(bad), Err(ProbeError::Invalid(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn registries_get_the_query_form_they_expect() {
        let ip = address("8.8.8.8");
        let block = address("8.8.8.0/24");
        let domain = Query {
            text: "example.com".into(),
            kind: Kind::Domain,
        };
        let asn = Query {
            text: "AS15169".into(),
            kind: Kind::AsNumber,
        };
        assert_eq!(query_for("whois.iana.org", &ip), "8.8.8.8");
        assert_eq!(query_for("whois.iana.org", &block), "8.8.8.0");
        assert_eq!(query_for("whois.arin.net", &ip), "n + 8.8.8.8");
        assert_eq!(query_for("whois.arin.net", &block), "n + 8.8.8.0");
        assert_eq!(query_for("whois.arin.net", &asn), "AS15169");
        assert_eq!(query_for("whois.ripe.net", &block), "8.8.8.0/24");
        assert_eq!(
            query_for("whois.verisign-grs.com", &domain),
            "domain example.com"
        );
        assert_eq!(
            query_for("whois.denic.de", &domain),
            "-T dn,ace example.com"
        );
        assert_eq!(query_for("whois.jprs.jp", &domain), "example.com/e");
        assert_eq!(query_for("whois.markmonitor.com", &domain), "example.com");
    }

    const IANA_IP: &str = "% IANA WHOIS server\n% for more information on IANA, visit http://www.iana.org\n% This query returned 1 object\n\nrefer:        whois.ripe.net\n\ninetnum:      193.0.0.0 - 193.255.255.255\norganisation: RIPE NCC\nstatus:       ALLOCATED\n\nwhois:        whois.ripe.net\n\nchanged:      1993-05\nsource:       IANA\n";

    const RIPE_IP: &str = "% This is the RIPE Database query service.\n% The objects are in RPSL format.\n%\n% Information related to '193.0.0.0 - 193.0.7.255'\n\n% Abuse contact for '193.0.0.0 - 193.0.7.255' is 'abuse@ripe.net'\n\ninetnum:        193.0.0.0 - 193.0.7.255\nnetname:        RIPE-NCC\ndescr:          RIPE Network Coordination Centre\ndescr:          Amsterdam, Netherlands\ncountry:        NL\norg:            ORG-RIEN1-RIPE\nadmin-c:        BRD-RIPE\ntech-c:         OPS4-RIPE\nstatus:         ASSIGNED PA\nmnt-by:         RIPE-NCC-MNT\ncreated:        2003-03-17T12:15:57Z\nlast-modified:  2017-12-04T14:42:31Z\nsource:         RIPE\n\norganisation:   ORG-RIEN1-RIPE\norg-name:       Reseaux IP Europeens Network Coordination Centre (RIPE NCC)\ncountry:        NL\norg-type:       LIR\naddress:        P.O. Box 10096\nabuse-c:        OPS4-RIPE\nsource:         RIPE\n\n% Information related to '193.0.0.0/21AS3333'\n\nroute:          193.0.0.0/21\ndescr:          RIPE-NCC\norigin:         AS3333\nmnt-by:         RIPE-NCC-MNT\nsource:         RIPE\n";

    const ARIN_IP: &str = "\n#\n# ARIN WHOIS data and services are subject to the Terms of Use\n#\n\n# start\n\nNetRange:       8.0.0.0 - 8.127.255.255\nCIDR:           8.0.0.0/9\nNetName:        LVLT-ORG-8-8\nNetHandle:      NET-8-0-0-0-1\nParent:         NET8 (NET-8-0-0-0-0)\nNetType:        Direct Allocation\nOriginAS:       \nOrganization:   Level 3 Parent, LLC (LPL-141)\nRegDate:        1992-12-01\nUpdated:        2018-04-23\n\nOrgName:        Level 3 Parent, LLC\nOrgId:          LPL-141\nCity:           Monroe\nCountry:        US\n\nOrgAbuseHandle: IPADD5-ARIN\nOrgAbuseEmail:  abuse@level3.com\n\n# end\n\n# start\n\nNetRange:       8.8.8.0 - 8.8.8.255\nCIDR:           8.8.8.0/24\nNetName:        GOGL\nNetHandle:      NET-8-8-8-0-2\nParent:         LVLT-ORG-8-8 (NET-8-0-0-0-1)\nNetType:        Direct Allocation\nOriginAS:       \nOrganization:   Google LLC (GOGL)\nRegDate:        2023-12-28\nUpdated:        2023-12-28\n\nOrgName:        Google LLC\nOrgId:          GOGL\nCity:           Mountain View\nCountry:        US\n\nOrgAbuseHandle: ABUSE5250-ARIN\nOrgAbuseEmail:  network-abuse@google.com\n\n# end\n";

    fn pairs(summary: &[(String, String)]) -> Vec<(&str, &str)> {
        summary
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect()
    }

    #[test]
    fn referrals_are_recognised() {
        assert_eq!(referral(IANA_IP), Some(("whois.ripe.net".into(), 43)));
        assert_eq!(referral(RIPE_IP), None);
        assert_eq!(referral(ARIN_IP), None);
        let arin_referral = "NetRange: 193.0.0.0 - 193.255.255.255\nReferralServer:  whois://whois.ripe.net\nResourceLink:  https://apps.db.ripe.net/search/query.html\n";
        assert_eq!(referral(arin_referral), Some(("whois.ripe.net".into(), 43)));
        let with_port = "ReferralServer: whois://whois.example.net:4343/\n";
        assert_eq!(
            referral(with_port),
            Some(("whois.example.net".into(), 4343))
        );
        // rwhois speaks a different protocol on another port.
        assert_eq!(
            referral("ReferralServer:  rwhois://rwhois.example.net:4321\n"),
            None
        );
        let registry = "   Domain Name: EXAMPLE.COM\n   Registrar WHOIS Server: whois.iana.org\n   Registrar URL: http://res-dom.iana.org\n";
        assert_eq!(referral(registry), Some(("whois.iana.org".into(), 43)));
        let tld = "domain:       COM\n\norganisation: VeriSign Global Registry Services\n\nwhois:        whois.verisign-grs.com\n";
        assert_eq!(referral(tld), Some(("whois.verisign-grs.com".into(), 43)));
        assert_eq!(referral("% nothing here\n\nrefer:\n"), None);
    }

    #[test]
    fn ripe_style_summary() {
        let summary = summarize(&[IANA_IP.to_owned(), RIPE_IP.to_owned()]);
        assert_eq!(
            pairs(&summary),
            [
                ("netname", "RIPE-NCC"),
                (
                    "org",
                    "Reseaux IP Europeens Network Coordination Centre (RIPE NCC)"
                ),
                ("country", "NL"),
                ("cidr", "193.0.0.0/21"),
                ("range", "193.0.0.0 - 193.0.7.255"),
                ("abuse", "abuse@ripe.net"),
                ("asn", "AS3333"),
                ("created", "2003-03-17T12:15:57Z"),
            ]
        );
    }

    #[test]
    fn arin_summary_uses_the_most_specific_network() {
        let summary = summarize(&[ARIN_IP.to_owned()]);
        assert_eq!(
            pairs(&summary),
            [
                ("netname", "GOGL"),
                ("org", "Google LLC"),
                ("country", "US"),
                ("cidr", "8.8.8.0/24"),
                ("range", "8.8.8.0 - 8.8.8.255"),
                ("abuse", "network-abuse@google.com"),
                ("created", "2023-12-28"),
            ]
        );
    }

    #[test]
    fn domain_summary_prefers_the_registrar_answer() {
        let registry = "   Domain Name: EXAMPLE.COM\n   Registry Domain ID: 2336799_DOMAIN_COM-VRSN\n   Registrar WHOIS Server: whois.iana.org\n   Updated Date: 2024-08-14T07:01:34Z\n   Creation Date: 1995-08-14T04:00:00Z\n   Registry Expiry Date: 2025-08-13T04:00:00Z\n   Registrar: RESERVED-Internet Assigned Numbers Authority\n   Registrar IANA ID: 376\n   Domain Status: clientDeleteProhibited https://icann.org/epp#clientDeleteProhibited\n   Name Server: A.IANA-SERVERS.NET\n   Name Server: B.IANA-SERVERS.NET\n   DNSSEC: signedDelegation\n>>> Last update of whois database: 2024-11-13T10:00:00Z <<<\n";
        let registrar = "Domain Name: example.com\nRegistrar: Example Registrar, Inc.\nRegistrar Abuse Contact Email: abuse@registrar.example\nRegistrant Organization: Internet Assigned Numbers Authority\nRegistrant Country: US\nRegistrar Registration Expiration Date: 2025-08-13T04:00:00Z\n";
        let summary = summarize(&[registry.to_owned(), registrar.to_owned()]);
        assert_eq!(
            pairs(&summary),
            [
                ("org", "Internet Assigned Numbers Authority"),
                ("country", "US"),
                ("abuse", "abuse@registrar.example"),
                ("domain", "example.com"),
                ("registrar", "Example Registrar, Inc."),
                ("created", "1995-08-14T04:00:00Z"),
                ("expires", "2025-08-13T04:00:00Z"),
                ("name_servers", "a.iana-servers.net, b.iana-servers.net"),
            ]
        );
    }

    #[test]
    fn lacnic_apnic_and_as_number_summaries() {
        let lacnic = "% Joint Whois - whois.lacnic.net\n\ninetnum:     200.160.0.0/20\nstatus:      assigned\naut-num:     AS22548\nowner:       Núcleo de Inf. e Coord. do Ponto BR - NIC.BR\ncountry:     BR\ncreated:     20010925\n";
        assert_eq!(
            pairs(&summarize(&[lacnic.to_owned()])),
            [
                ("org", "Núcleo de Inf. e Coord. do Ponto BR - NIC.BR"),
                ("country", "BR"),
                ("cidr", "200.160.0.0/20"),
                ("range", "200.160.0.0/20"),
                ("asn", "AS22548"),
                ("created", "20010925"),
            ]
        );
        let apnic = "% [whois.apnic.net]\n% Abuse contact for '1.1.1.0 - 1.1.1.255' is 'helpdesk@apnic.net'\n\ninetnum:        1.1.1.0 - 1.1.1.255\nnetname:        APNIC-LABS\ndescr:          APNIC and Cloudflare DNS Resolver project\ncountry:        AU\n";
        assert_eq!(
            pairs(&summarize(&[apnic.to_owned()])),
            [
                ("netname", "APNIC-LABS"),
                ("org", "APNIC and Cloudflare DNS Resolver project"),
                ("country", "AU"),
                ("cidr", "1.1.1.0/24"),
                ("range", "1.1.1.0 - 1.1.1.255"),
                ("abuse", "helpdesk@apnic.net"),
            ]
        );
        let asn = "ASNumber:       13335\nASName:         CLOUDFLARENET\nASHandle:       AS13335\nRegDate:        2010-07-14\n\nOrgName:        Cloudflare, Inc.\nCountry:        US\nOrgAbuseEmail:  abuse@cloudflare.com\n";
        assert_eq!(
            pairs(&summarize(&[asn.to_owned()])),
            [
                ("org", "Cloudflare, Inc."),
                ("country", "US"),
                ("abuse", "abuse@cloudflare.com"),
                ("asn", "13335"),
                ("as_name", "CLOUDFLARENET"),
                ("created", "2010-07-14"),
            ]
        );
    }

    #[test]
    fn ranges_become_cidr_blocks() {
        assert_eq!(
            range_to_cidr("193.0.0.0 - 193.0.7.255").as_deref(),
            Some("193.0.0.0/21")
        );
        assert_eq!(
            range_to_cidr("10.0.0.0-10.255.255.255").as_deref(),
            Some("10.0.0.0/8")
        );
        assert_eq!(
            range_to_cidr("0.0.0.0 - 255.255.255.255").as_deref(),
            Some("0.0.0.0/0")
        );
        assert_eq!(
            range_to_cidr("192.0.2.7 - 192.0.2.7").as_deref(),
            Some("192.0.2.7/32")
        );
        // Not one block: 192.0.2.1 .. 192.0.2.6.
        assert_eq!(
            range_to_cidr("192.0.2.1 - 192.0.2.6").as_deref(),
            Some("192.0.2.1/32, 192.0.2.2/31, 192.0.2.4/31, 192.0.2.6/32")
        );
        assert_eq!(
            range_to_cidr("2001:db8::/32").as_deref(),
            Some("2001:db8::/32")
        );
        assert_eq!(range_to_cidr("10.0.0.9 - 10.0.0.1"), None);
        assert_eq!(range_to_cidr("garbage"), None);
    }

    /// A WHOIS server on loopback that answers every query with `answer` and records the query.
    async fn fake_server(answer: String) -> (u16, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (queries, received) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut query = Vec::new();
                let mut byte = [0u8; 1];
                while !query.ends_with(b"\r\n") && stream.read_exact(&mut byte).await.is_ok() {
                    query.push(byte[0]);
                }
                let _ = queries.send(String::from_utf8_lossy(&query).into_owned());
                let _ = stream
                    .write_all(answer.replace('\n', "\r\n").as_bytes())
                    .await;
            }
        });
        (port, received)
    }

    #[tokio::test]
    async fn follows_the_referral_chain_and_reports_every_server() {
        let (registry, mut registry_queries) = fake_server(RIPE_IP.to_owned()).await;
        let middle = format!(
            "NetRange: 193.0.0.0 - 193.255.255.255\nReferralServer: whois://127.0.0.1:{registry}\n"
        );
        let (rir, mut rir_queries) = fake_server(middle).await;
        let root = format!(
            "% IANA WHOIS server\n\nrefer:        127.0.0.1:{rir}\n\ninetnum:      193.0.0.0 - 193.255.255.255\n"
        );
        let (iana, mut iana_queries) = fake_server(root).await;

        let answer = whois_at(
            "193.0.0.1",
            &format!("127.0.0.1:{iana}"),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(answer.query, "193.0.0.1");
        assert_eq!(
            answer.servers,
            [
                format!("127.0.0.1:{iana}"),
                format!("127.0.0.1:{rir}"),
                format!("127.0.0.1:{registry}")
            ]
        );
        assert_eq!(answer.text, RIPE_IP);
        assert_eq!(
            answer.summary[0],
            ("netname".to_owned(), "RIPE-NCC".to_owned())
        );
        for queries in [&mut iana_queries, &mut rir_queries, &mut registry_queries] {
            assert_eq!(queries.recv().await.unwrap(), "193.0.0.1\r\n");
        }
    }

    #[tokio::test]
    async fn referral_loops_and_dead_referrals_end_the_chain() {
        // A registry that refers to itself.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let (own, _queries) =
            fake_server(format!("refer: 127.0.0.1:{port}\nnetname: SELF\n")).await;
        // The referral points at a port nobody listens on: the answer so far is the result.
        let answer = whois_at(
            "192.0.2.1",
            &format!("127.0.0.1:{own}"),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(answer.servers, [format!("127.0.0.1:{own}")]);
        assert_eq!(answer.summary, [("netname".to_owned(), "SELF".to_owned())]);

        let looping_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let looping_port = looping_listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let answer = format!("refer: 127.0.0.1:{looping_port}\nnetname: LOOP\n");
            while let Ok((mut stream, _)) = looping_listener.accept().await {
                let mut sink = [0u8; 64];
                let _ = stream.read(&mut sink).await;
                let _ = stream.write_all(answer.as_bytes()).await;
            }
        });
        let answer = whois_at(
            "192.0.2.1",
            &format!("127.0.0.1:{looping_port}"),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(answer.servers.len(), 1, "a server is never asked twice");
    }

    #[tokio::test]
    async fn at_most_four_referrals_are_followed() {
        // Six servers in a row, each referring to the next.
        let mut next_answer = "netname: END-OF-CHAIN\n".to_owned();
        let mut ports = Vec::new();
        for _ in 0..6 {
            let (port, _queries) = fake_server(next_answer).await;
            next_answer = format!("refer: 127.0.0.1:{port}\nnetname: HOP-{port}\n");
            ports.push(port);
        }
        let first = *ports.last().unwrap();
        let answer = whois_at(
            "192.0.2.1",
            &format!("127.0.0.1:{first}"),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(answer.servers.len(), 1 + MAX_REFERRALS);
        assert!(
            answer.text.contains("HOP-"),
            "the chain was cut before its end: {}",
            answer.text
        );
    }

    #[tokio::test]
    async fn failures_of_the_first_server_are_errors() {
        let closed = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            listener.local_addr().unwrap().port()
        };
        let error = whois_at(
            "192.0.2.1",
            &format!("127.0.0.1:{closed}"),
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ProbeError::Refused), "{error:?}");

        // A server that accepts and never answers.
        let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = silent.local_addr().unwrap().port();
        let started = Instant::now();
        let error = whois_at(
            "192.0.2.1",
            &format!("127.0.0.1:{port}"),
            Duration::from_millis(300),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ProbeError::Timeout), "{error:?}");
        assert!(started.elapsed() < Duration::from_secs(2));

        let (empty, _queries) = fake_server(String::new()).await;
        let error = whois_at(
            "192.0.2.1",
            &format!("127.0.0.1:{empty}"),
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ProbeError::Protocol(_)), "{error:?}");

        let error = whois_at("192.0.2.1", "rwhois://example.net", Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(matches!(error, ProbeError::Invalid(_)), "{error:?}");
    }

    #[tokio::test]
    async fn an_endless_answer_is_cut_off() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let line = b"remarks: this server never stops talking\n".repeat(256);
            while stream.write_all(&line).await.is_ok() {}
        });
        let answer = whois_at(
            "192.0.2.1",
            &format!("127.0.0.1:{port}"),
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert_eq!(answer.text.len(), MAX_RESPONSE);
    }
}
