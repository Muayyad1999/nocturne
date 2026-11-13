//! Incremental parsers for the output of the system `traceroute` (Linux, macOS, the BSDs,
//! busybox, inetutils) and `tracert` (Windows, any UI language).

use std::net::IpAddr;
use std::sync::LazyLock;

use regex::Regex;

use crate::util::parse_ip_literal;

/// One probe of a hop as the system tool printed it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProbeLine {
    /// Who answered; `None` for `*`.
    pub address: Option<IpAddr>,
    pub rtt_ms: Option<f64>,
    /// The `!H`-style annotation, without the `!`.
    pub flag: Option<String>,
}

/// One hop line.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HopLine {
    pub ttl: u8,
    pub probes: Vec<ProbeLine>,
    /// Text the tool printed instead of an address (Windows: "Destination host unreachable").
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flavor {
    /// `traceroute` of Linux, macOS, the BSDs, busybox and inetutils.
    Unix,
    /// Windows `tracert`.
    Windows,
}

/// Feeds on output lines and yields hops as they are completed.
///
/// A Unix hop may continue on following lines (one per additional responder), so it is only
/// known to be complete when all its probes are present or the next hop begins.
pub(crate) struct TraceParser {
    flavor: Flavor,
    probes_per_hop: usize,
    pending: Option<HopLine>,
}

impl TraceParser {
    pub(crate) fn new(flavor: Flavor, probes_per_hop: usize) -> Self {
        TraceParser {
            flavor,
            probes_per_hop,
            pending: None,
        }
    }

    /// Consumes one line of output; returns the hops that are now complete, in order.
    pub(crate) fn feed(&mut self, line: &str) -> Vec<HopLine> {
        let line = line.trim_end_matches(['\r', '\n']);
        match self.flavor {
            Flavor::Windows => parse_windows_line(line).into_iter().collect(),
            Flavor::Unix => self.feed_unix(line),
        }
    }

    /// The hop still being assembled when the output ended.
    pub(crate) fn finish(&mut self) -> Option<HopLine> {
        self.pending.take()
    }

    fn feed_unix(&mut self, line: &str) -> Vec<HopLine> {
        static HOP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(\d{1,3})\s+(\S.*)$").unwrap());
        let mut complete = Vec::new();
        if let Some(captures) = HOP.captures(line) {
            let probes = parse_unix_probes(&captures[2]);
            // "traceroute to 8.8.8.8 …" and similar never start with a number, but a line that
            // does and holds no probe at all is not a hop either.
            if let (Ok(ttl), false) = (captures[1].parse::<u8>(), probes.is_empty()) {
                complete.extend(self.pending.take());
                self.pending = Some(HopLine {
                    ttl,
                    probes,
                    message: None,
                });
            }
        } else if line.starts_with(char::is_whitespace) {
            // A continuation: more answers for the hop above, from another router.
            if let Some(pending) = &mut self.pending {
                pending.probes.extend(parse_unix_probes(line));
            }
        }
        let is_full = self
            .pending
            .as_ref()
            .is_some_and(|hop| hop.probes.len() >= self.probes_per_hop);
        if is_full {
            complete.extend(self.pending.take());
        }
        complete
    }
}

/// The probes on a Unix hop line after the hop number:
/// `72.14.215.85  9.821 ms 72.14.215.87  10.212 ms  9.914 ms`, `* * *`,
/// `router.lan (192.168.1.1)  0.411 ms !H  0.389 ms !H *`, `127.0.0.1  0.003ms`.
fn parse_unix_probes(text: &str) -> Vec<ProbeLine> {
    let mut probes: Vec<ProbeLine> = Vec::new();
    let mut responder: Option<IpAddr> = None;
    let mut tokens = text.split_whitespace().peekable();
    while let Some(token) = tokens.next() {
        if token == "*" {
            probes.push(ProbeLine {
                address: None,
                rtt_ms: None,
                flag: None,
            });
        } else if let Some(flag) = token.strip_prefix('!') {
            // "!H", "!N", "!X", "!<13>": qualifies the probe just before it.
            if let Some(last) = probes.last_mut() {
                last.flag = Some(flag.trim_matches(['<', '>']).to_owned());
            }
        } else if let Some(rtt) = parse_rtt(token, &mut tokens) {
            probes.push(ProbeLine {
                address: responder,
                rtt_ms: Some(rtt),
                flag: None,
            });
        } else if let Some(address) = parse_ip_literal(token.trim_matches(['(', ')'])) {
            responder = Some(address);
        }
        // Anything else is a host name (the address follows in parentheses) or decoration
        // such as "[AS15169]".
    }
    probes
}

/// `0.411 ms` (two tokens) or `0.411ms` (one).
fn parse_rtt<'a>(
    token: &str,
    rest: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
) -> Option<f64> {
    if let Some(number) = token.strip_suffix("ms") {
        return number.parse().ok();
    }
    let number: f64 = token.parse().ok()?;
    // A bare number is a time only when "ms" follows; otherwise it could be an IPv4-less
    // token such as a hop count in a header.
    (rest.peek() == Some(&"ms")).then(|| {
        rest.next();
        number
    })
}

/// A hop line of `tracert`:
///
/// ```text
///   1    <1 ms    <1 ms    <1 ms  192.168.1.1
///   3     *        *        *     Request timed out.
///   4     9 ms    10 ms     9 ms  core1.example.net [72.14.215.85]
///   6  192.168.1.1  reports: Destination host unreachable.
/// ```
///
/// Only numbers, `ms`, `*` and addresses are relied on, so the UI language does not matter.
fn parse_windows_line(line: &str) -> Option<HopLine> {
    static PROBES: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\s*(\d{1,3})\s+((?:(?:<?\d+\s*ms|\*)\s+){3})(.*)$").unwrap()
    });
    static PROBE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(<?)(\d+)\s*ms|\*").unwrap());
    static REPORTS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*(\d{1,3})\s+(\S+)\s+\S+:\s+(\S.*)$").unwrap());

    if let Some(captures) = PROBES.captures(line) {
        let ttl = captures[1].parse().ok()?;
        let rest = captures[3].trim();
        // "name [address]" or just the address; anything else is a message ("Request timed out.").
        let address = rest
            .rsplit_once('[')
            .map(|(_, bracketed)| bracketed.trim_end_matches(']'))
            .or(Some(rest))
            .and_then(parse_ip_literal);
        let probes = PROBE
            .captures_iter(&captures[2])
            .map(|probe| {
                let rtt_ms = probe.get(2).and_then(|number| number.as_str().parse().ok());
                ProbeLine {
                    address: address.filter(|_| rtt_ms.is_some()),
                    rtt_ms,
                    flag: None,
                }
            })
            .collect();
        let message = match address {
            None if !rest.is_empty() && rest.chars().any(char::is_alphabetic) => {
                Some(rest.trim_end_matches('.').to_owned())
            }
            _ => None,
        };
        return Some(HopLine {
            ttl,
            probes,
            message,
        });
    }
    // "<ttl>  <address>  reports: <ICMP error>": a router answered with an unreachable message.
    let captures = REPORTS.captures(line)?;
    let address = parse_ip_literal(&captures[2])?;
    Some(HopLine {
        ttl: captures[1].parse().ok()?,
        probes: vec![ProbeLine {
            address: Some(address),
            rtt_ms: None,
            flag: None,
        }],
        message: Some(captures[3].trim_end_matches('.').to_owned()),
    })
}

/// What a `!X` annotation of traceroute means.
pub(crate) fn flag_meaning(flag: &str) -> String {
    match flag {
        "H" => "host unreachable".to_owned(),
        "N" => "network unreachable".to_owned(),
        "P" => "protocol unreachable".to_owned(),
        "S" => "source route failed".to_owned(),
        "F" => "fragmentation needed".to_owned(),
        "X" => "communication administratively prohibited".to_owned(),
        "V" => "host precedence violation".to_owned(),
        "C" => "precedence cutoff in effect".to_owned(),
        "A" => "address unreachable".to_owned(),
        other => match other.parse::<u8>() {
            Ok(code) => format!("destination unreachable (ICMP code {code})"),
            Err(_) => format!("ICMP error !{other}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn parse_all(flavor: Flavor, probes: usize, output: &str) -> Vec<HopLine> {
        let mut parser = TraceParser::new(flavor, probes);
        let mut hops = Vec::new();
        for line in output.lines() {
            hops.extend(parser.feed(line));
        }
        hops.extend(parser.finish());
        hops
    }

    fn answered(address: &str, rtt_ms: f64) -> ProbeLine {
        ProbeLine {
            address: Some(address.parse().unwrap()),
            rtt_ms: Some(rtt_ms),
            flag: None,
        }
    }

    const SILENT: ProbeLine = ProbeLine {
        address: None,
        rtt_ms: None,
        flag: None,
    };

    fn hop(ttl: u8, probes: Vec<ProbeLine>) -> HopLine {
        HopLine {
            ttl,
            probes,
            message: None,
        }
    }

    #[test]
    fn linux_numeric() {
        let output = "traceroute to 8.8.8.8 (8.8.8.8), 30 hops max, 60 byte packets\n\
                      \x201  192.168.1.1  0.411 ms  0.389 ms  0.357 ms\n\
                      \x202  10.20.0.1  2.105 ms  2.324 ms  2.279 ms\n\
                      \x203  * * *\n\
                      \x204  72.14.215.85  9.821 ms 72.14.215.87  10.212 ms  9.914 ms\n\
                      \x205  * 108.170.250.33  11.4 ms *\n\
                      \x206  8.8.8.8  9.544 ms  9.397 ms  9.468 ms\n";
        assert_eq!(
            parse_all(Flavor::Unix, 3, output),
            vec![
                hop(1, vec![answered("192.168.1.1", 0.411), answered("192.168.1.1", 0.389), answered("192.168.1.1", 0.357)]),
                hop(2, vec![answered("10.20.0.1", 2.105), answered("10.20.0.1", 2.324), answered("10.20.0.1", 2.279)]),
                hop(3, vec![SILENT, SILENT, SILENT]),
                hop(4, vec![answered("72.14.215.85", 9.821), answered("72.14.215.87", 10.212), answered("72.14.215.87", 9.914)]),
                hop(5, vec![SILENT, answered("108.170.250.33", 11.4), SILENT]),
                hop(6, vec![answered("8.8.8.8", 9.544), answered("8.8.8.8", 9.397), answered("8.8.8.8", 9.468)]),
            ]
        );
    }

    #[test]
    fn linux_with_names_and_unreachable_flags() {
        let output = "traceroute to 192.0.2.1 (192.0.2.1), 3 hops max, 60 byte packets\n\
                      \x201  gw.example.net (192.0.2.1)  0.234 ms  0.202 ms\n";
        assert_eq!(
            parse_all(Flavor::Unix, 2, output),
            vec![hop(1, vec![answered("192.0.2.1", 0.234), answered("192.0.2.1", 0.202)])]
        );

        let unreachable = "traceroute to 10.9.9.9 (10.9.9.9), 30 hops max, 60 byte packets\n\
                           \x201  192.168.1.1  0.5 ms  0.4 ms  0.4 ms\n\
                           \x202  192.168.1.1  3051.2 ms !H  3052.8 ms !H *\n";
        let hops = parse_all(Flavor::Unix, 3, unreachable);
        assert_eq!(hops.len(), 2);
        assert_eq!(
            hops[1].probes,
            vec![
                ProbeLine { flag: Some("H".into()), ..answered("192.168.1.1", 3051.2) },
                ProbeLine { flag: Some("H".into()), ..answered("192.168.1.1", 3052.8) },
                SILENT,
            ]
        );

        let prohibited = " 7  203.0.113.9  12.1 ms !X  12.0 ms !<10>  12.4 ms !N\n";
        let flags: Vec<Option<String>> = parse_all(Flavor::Unix, 3, prohibited)[0]
            .probes
            .iter()
            .map(|probe| probe.flag.clone())
            .collect();
        assert_eq!(flags, [Some("X".into()), Some("10".into()), Some("N".into())]);
    }

    #[test]
    fn linux_ipv6() {
        let output = "traceroute to 2001:4860:4860::8888 (2001:4860:4860::8888), 30 hops max, 80 byte packets\n\
                      \x201  2001:db8:1::1  0.612 ms  0.544 ms  0.521 ms\n\
                      \x202  * * *\n\
                      \x203  2001:4860:4860::8888  10.2 ms  10.1 ms  10.3 ms\n";
        let hops = parse_all(Flavor::Unix, 3, output);
        assert_eq!(hops.len(), 3);
        assert_eq!(hops[0].probes[0], answered("2001:db8:1::1", 0.612));
        assert_eq!(hops[2].probes[2], answered("2001:4860:4860::8888", 10.3));
    }

    #[test]
    fn macos_continuation_lines() {
        // macOS prints additional responders of a hop on their own lines. The header goes to
        // stderr, so stdout starts with the first hop.
        let output = " 1  192.168.1.1 (192.168.1.1)  1.123 ms  0.912 ms  0.855 ms\n\
                      \x202  * * *\n\
                      \x203  ae-1.r01.example.net (10.20.0.1)  2.105 ms\n\
                      \x20   ae-2.r01.example.net (10.20.0.2)  2.324 ms  2.279 ms\n\
                      \x204  dns.google (8.8.8.8)  9.544 ms  9.397 ms  9.468 ms\n";
        assert_eq!(
            parse_all(Flavor::Unix, 3, output),
            vec![
                hop(1, vec![answered("192.168.1.1", 1.123), answered("192.168.1.1", 0.912), answered("192.168.1.1", 0.855)]),
                hop(2, vec![SILENT, SILENT, SILENT]),
                hop(3, vec![answered("10.20.0.1", 2.105), answered("10.20.0.2", 2.324), answered("10.20.0.2", 2.279)]),
                hop(4, vec![answered("8.8.8.8", 9.544), answered("8.8.8.8", 9.397), answered("8.8.8.8", 9.468)]),
            ]
        );
    }

    #[test]
    fn hops_are_emitted_as_soon_as_they_are_complete() {
        let mut parser = TraceParser::new(Flavor::Unix, 3);
        assert_eq!(parser.feed("traceroute to 8.8.8.8 (8.8.8.8), 30 hops max, 60 byte packets"), vec![]);
        // Three probes on the line: complete at once.
        assert_eq!(parser.feed(" 1  192.168.1.1  0.4 ms  0.3 ms  0.3 ms").len(), 1);
        // One probe so far: wait for a continuation or the next hop.
        assert_eq!(parser.feed(" 2  10.20.0.1  2.1 ms"), vec![]);
        assert_eq!(parser.feed("    10.20.0.2  2.3 ms  2.2 ms").len(), 1);
        assert_eq!(parser.feed(" 3  10.30.0.1  5.0 ms"), vec![]);
        // The next hop starts: hop 3 was all there was.
        let emitted = parser.feed(" 4  8.8.8.8  9.5 ms  9.4 ms  9.4 ms");
        assert_eq!(emitted.iter().map(|hop| hop.ttl).collect::<Vec<_>>(), [3, 4]);
        assert_eq!(parser.finish(), None);
    }

    #[test]
    fn inetutils_and_busybox() {
        let inetutils = "traceroute to 127.0.0.1 (127.0.0.1), 64 hops max\n\
                         \x20 1   127.0.0.1  0.003ms  0.002ms  0.001ms \n";
        assert_eq!(
            parse_all(Flavor::Unix, 3, inetutils),
            vec![hop(1, vec![answered("127.0.0.1", 0.003), answered("127.0.0.1", 0.002), answered("127.0.0.1", 0.001)])]
        );
        let busybox = "traceroute to 10.0.0.1 (10.0.0.1), 30 hops max, 46 byte packets\n\
                       \x201  10.0.0.1 (10.0.0.1)  0.331 ms  0.290 ms  0.270 ms\n";
        assert_eq!(parse_all(Flavor::Unix, 3, busybox)[0].probes.len(), 3);
    }

    #[test]
    fn windows_english() {
        let output = "\r\nTracing route to 8.8.8.8 over a maximum of 30 hops\r\n\r\n\
                      \x20 1    <1 ms    <1 ms    <1 ms  192.168.1.1 \r\n\
                      \x20 2     2 ms     2 ms     2 ms  10.20.0.1 \r\n\
                      \x20 3     *        *        *     Request timed out.\r\n\
                      \x20 4     9 ms    10 ms     9 ms  72.14.215.85 \r\n\
                      \x20 5     *        9 ms     *     8.8.8.8 \r\n\
                      \r\nTrace complete.\r\n";
        assert_eq!(
            parse_all(Flavor::Windows, 3, output),
            vec![
                hop(1, vec![answered("192.168.1.1", 1.0), answered("192.168.1.1", 1.0), answered("192.168.1.1", 1.0)]),
                hop(2, vec![answered("10.20.0.1", 2.0), answered("10.20.0.1", 2.0), answered("10.20.0.1", 2.0)]),
                HopLine { message: Some("Request timed out".into()), ..hop(3, vec![SILENT, SILENT, SILENT]) },
                hop(4, vec![answered("72.14.215.85", 9.0), answered("72.14.215.85", 10.0), answered("72.14.215.85", 9.0)]),
                hop(5, vec![SILENT, answered("8.8.8.8", 9.0), SILENT]),
            ]
        );
    }

    #[test]
    fn windows_with_names_and_unreachable_report() {
        let output = "\r\nTracing route to dns.google [8.8.8.8]\r\nover a maximum of 30 hops:\r\n\r\n\
                      \x20 1     1 ms    <1 ms    <1 ms  router.lan [192.168.1.1] \r\n\
                      \x20 2  192.168.1.1  reports: Destination host unreachable.\r\n\
                      \r\nTrace complete.\r\n";
        assert_eq!(
            parse_all(Flavor::Windows, 3, output),
            vec![
                hop(1, vec![answered("192.168.1.1", 1.0), answered("192.168.1.1", 1.0), answered("192.168.1.1", 1.0)]),
                HopLine {
                    ttl: 2,
                    probes: vec![ProbeLine { address: Some("192.168.1.1".parse().unwrap()), rtt_ms: None, flag: None }],
                    message: Some("Destination host unreachable".into()),
                },
            ]
        );
    }

    #[test]
    fn windows_german() {
        let output = "\r\nRoutenverfolgung zu 8.8.8.8 über maximal 30 Hops\r\n\r\n\
                      \x20 1    <1 ms    <1 ms    <1 ms  192.168.1.1 \r\n\
                      \x20 2     *        *        *     Zeitüberschreitung der Anforderung.\r\n\
                      \x20 3    12 ms    11 ms    12 ms  8.8.8.8 \r\n\
                      \x20 4  192.168.1.1  meldet: Zielhost nicht erreichbar.\r\n\
                      \r\nAblaufverfolgung beendet.\r\n";
        let hops = parse_all(Flavor::Windows, 3, output);
        assert_eq!(hops.len(), 4);
        assert_eq!(hops[0].probes[0], answered("192.168.1.1", 1.0));
        assert_eq!(hops[1].probes, vec![SILENT, SILENT, SILENT]);
        assert_eq!(hops[1].message.as_deref(), Some("Zeitüberschreitung der Anforderung"));
        assert_eq!(hops[2].probes[1], answered("8.8.8.8", 11.0));
        assert_eq!(hops[3].message.as_deref(), Some("Zielhost nicht erreichbar"));
        assert_eq!(hops[3].probes[0].address, Some("192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn windows_ipv6() {
        let output = "\r\nTracing route to 2001:4860:4860::8888 over a maximum of 30 hops\r\n\r\n\
                      \x20 1     1 ms    <1 ms    <1 ms  2001:db8:1::1 \r\n\
                      \x20 2    11 ms    10 ms    10 ms  2001:4860:4860::8888 \r\n";
        let hops = parse_all(Flavor::Windows, 3, output);
        assert_eq!(hops[1].probes[2], answered("2001:4860:4860::8888", 10.0));
    }

    #[test]
    fn headers_errors_and_garbage_are_not_hops() {
        for flavor in [Flavor::Unix, Flavor::Windows] {
            for junk in [
                "",
                "traceroute to 8.8.8.8 (8.8.8.8), 30 hops max, 60 byte packets",
                "traceroute: unknown host no-such-host",
                "Tracing route to 8.8.8.8 over a maximum of 30 hops",
                "Trace complete.",
                "Unable to resolve target system name nohost.",
                "30 hops max",
                "1",
                " 12 ",
                "999999999999 * * *",
                "\u{0}\u{1}\u{2}",
                "  1    <1 ms    <1 ms",
            ] {
                let hops = parse_all(flavor, 3, junk);
                assert!(hops.is_empty(), "{flavor:?} {junk:?} -> {hops:?}");
            }
        }
    }

    #[test]
    fn flags_are_explained() {
        assert_eq!(flag_meaning("H"), "host unreachable");
        assert_eq!(flag_meaning("N"), "network unreachable");
        assert_eq!(flag_meaning("X"), "communication administratively prohibited");
        assert_eq!(flag_meaning("13"), "destination unreachable (ICMP code 13)");
        assert_eq!(flag_meaning("Q"), "ICMP error !Q");
    }
}
