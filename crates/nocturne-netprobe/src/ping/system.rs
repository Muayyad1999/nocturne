//! Echo through the operating system's `ping` program, for machines where this process may
//! not open ICMP sockets.
//!
//! The output is parsed without relying on its language: a reply is recognised by a duration
//! in milliseconds (or, on Windows, by the value in front of `TTL=`), never by words.

use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use tokio::sync::Semaphore;

use super::PingReply;
use super::icmp::ECHO_HEADER;
use crate::ProbeError;
use crate::process::{self, RunError};

/// How many `ping` processes may run at once.
const MAX_PROCESSES: usize = 32;
/// Extra wall-clock time a `ping` process gets for starting up and exiting.
const PROCESS_GRACE: Duration = Duration::from_millis(1500);

pub(crate) struct SystemPing {
    permits: Semaphore,
}

impl SystemPing {
    pub(crate) fn new() -> Self {
        SystemPing {
            permits: Semaphore::new(MAX_PROCESSES),
        }
    }

    pub(crate) async fn echo(
        &self,
        target: IpAddr,
        timeout: Duration,
        payload: usize,
    ) -> Result<PingReply, ProbeError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .expect("the semaphore is never closed");
        let (program, args) = command_line(target, timeout, payload);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = match process::run(program, &args, timeout + PROCESS_GRACE).await {
            Ok(output) => output,
            Err(RunError::TimedOut) => return Err(ProbeError::Timeout),
            Err(RunError::Missing) => {
                return Err(ProbeError::unsupported(format!(
                    "this process may not open ICMP sockets and the system `{program}` program \
                     was not found. Install it (iputils-ping on Debian/Ubuntu), or on Linux allow \
                     unprivileged ping with `sysctl net.ipv4.ping_group_range=\"0 2147483647\"`"
                )));
            }
            Err(RunError::Io(error)) => return Err(ProbeError::Io(error)),
        };
        // Windows exits with status 0 when a router answered with an ICMP error, so only the
        // Unix exit status says anything about a reply.
        let replied = output.success && !cfg!(windows);
        match parse_output(&output.combined(), replied) {
            Outcome::Reply { rtt_ms, ttl, bytes } => {
                if rtt_ms > timeout.as_secs_f64() * 1000.0 {
                    return Err(ProbeError::Timeout);
                }
                Ok(PingReply {
                    rtt_ms,
                    ttl,
                    bytes: bytes.unwrap_or(ECHO_HEADER + payload),
                })
            }
            Outcome::Unreachable(reason) => Err(ProbeError::Unreachable(reason)),
            Outcome::Denied(reason) => Err(ProbeError::Permission(format!(
                "the system ping could not send the echo request: {reason}"
            ))),
            Outcome::NoReply => Err(ProbeError::Timeout),
            Outcome::Unrecognized(line) => Err(ProbeError::protocol(format!(
                "the system ping reported a reply, but its output was not understood: {line}"
            ))),
        }
    }
}

/// The program and arguments for one echo with this platform's `ping`.
fn command_line(target: IpAddr, timeout: Duration, payload: usize) -> (&'static str, Vec<String>) {
    let millis = timeout.as_millis().max(1).to_string();
    let seconds = timeout.as_secs_f64().ceil().max(1.0).to_string();
    let payload = payload.to_string();
    let address = target.to_string();
    let v6 = target.is_ipv6();
    let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
        let family = if v6 { "-6" } else { "-4" };
        ("ping", vec!["-n", "1", "-w", &millis, "-l", &payload, family, &address])
    } else if cfg!(any(target_os = "macos", target_os = "freebsd", target_os = "dragonfly")) {
        if v6 {
            ("ping6", vec!["-n", "-c", "1", "-s", &payload, &address])
        } else {
            // BSD ping takes the wait time in milliseconds.
            ("ping", vec!["-n", "-c", "1", "-W", &millis, "-s", &payload, &address])
        }
    } else if cfg!(any(target_os = "linux", target_os = "android")) {
        let family = if v6 { "-6" } else { "-4" };
        // iputils and busybox take whole seconds; the caller's timeout is enforced on top.
        ("ping", vec!["-n", "-c", "1", "-W", &seconds, "-s", &payload, family, &address])
    } else if v6 {
        ("ping6", vec!["-n", "-c", "1", "-s", &payload, &address])
    } else {
        ("ping", vec!["-n", "-c", "1", "-s", &payload, &address])
    };
    (program, args.into_iter().map(str::to_owned).collect())
}

/// What one run of `ping -c 1` / `ping -n 1` reported.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    Reply {
        rtt_ms: f64,
        ttl: Option<u8>,
        /// Size of the ICMP reply, when the output states it in a known form.
        bytes: Option<usize>,
    },
    /// An ICMP error came back, or the local stack had no route.
    Unreachable(String),
    /// `ping` itself was not allowed to send.
    Denied(String),
    NoReply,
    /// The exit status says the host answered, but no line of the output looks like a reply.
    Unrecognized(String),
}

/// A duration in milliseconds after `=` or `<` ("time=0.41 ms", "Zeit<1ms", "время=1мс").
static DURATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[=<]\s*(\d+(?:[.,]\d+)?)\s*(?:ms|мс)").unwrap());
static TTL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:ttl|hlim)\s*[=:]\s*(\d+)").unwrap());
/// Any `key=number` / `key<number` value.
static VALUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[=<]\s*(\d+(?:[.,]\d+)?)").unwrap());
static BYTES_FROM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+) bytes from ").unwrap());
/// "… <address>: <text>" — how every `ping` prints an ICMP error it received, in any language.
static REPORTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"((?:\d{1,3}\.){3}\d{1,3}|[0-9a-fA-F]*:[0-9a-fA-F:.]*:[0-9a-fA-F.]*)(?:%\w+)?\s?:\s+(\S.*)$")
        .unwrap()
});
/// Linux: "From 10.0.0.1 icmp_seq=1 Destination Host Unreachable".
static FROM_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^From (\S+?):? icmp_seq=\d+ (.+)$").unwrap());
static UNREACHABLE_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)unreachable|prohibited|filtered|time to live exceeded|ttl expired|hop limit|no route|network is down|frag(mentation)? needed|source route failed",
    )
    .unwrap()
});
static DENIED_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)operation not permitted|permission denied|access denied|cap_net_raw").unwrap()
});

/// Interprets the output of one echo by the system `ping` (any OS, any UI language).
///
/// `replied` is what the exit status says, on the systems where it is meaningful.
pub(crate) fn parse_output(output: &str, replied: bool) -> Outcome {
    let lines: Vec<&str> = output.lines().map(str::trim).filter(|line| !line.is_empty()).collect();

    for line in &lines {
        if let Some(rtt_ms) = reply_time(line) {
            return Outcome::Reply {
                rtt_ms,
                ttl: TTL
                    .captures(line)
                    .and_then(|captures| captures[1].parse().ok()),
                bytes: BYTES_FROM
                    .captures(line)
                    .and_then(|captures| captures[1].parse().ok()),
            };
        }
    }

    for line in &lines {
        if DENIED_WORDS.is_match(line) {
            return Outcome::Denied(strip_program_prefix(line).to_owned());
        }
    }
    for line in &lines {
        if let Some(captures) = FROM_LINE.captures(line) {
            return Outcome::Unreachable(format!("{} (reported by {})", &captures[2], &captures[1]));
        }
        if let Some(captures) = REPORTED.captures(line) {
            let text = captures[2].trim_end_matches('.').trim();
            // A value in the text means a (non-reply) status line, not an error description.
            if !text.is_empty() && !VALUE.is_match(text) {
                return Outcome::Unreachable(format!("{text} (reported by {})", &captures[1]));
            }
        }
    }
    for line in &lines {
        if UNREACHABLE_WORDS.is_match(line) {
            return Outcome::Unreachable(strip_program_prefix(line).to_owned());
        }
    }
    match lines.first() {
        Some(first) if replied => Outcome::Unrecognized((*first).to_owned()),
        _ => Outcome::NoReply,
    }
}

/// The round-trip time on a reply line, or `None` if the line is not a reply.
fn reply_time(line: &str) -> Option<f64> {
    if let Some(captures) = DURATION.captures(line) {
        return parse_number(&captures[1]);
    }
    // Windows prints "bytes=32 time=1ms TTL=64" with the words — and, in some locales, the
    // unit — in the system language and code page. The value just before TTL is the time.
    let ttl = TTL.find(line)?;
    let before = &line[..ttl.start()];
    let values: Vec<_> = VALUE.captures_iter(before).collect();
    if values.len() < 2 {
        return None;
    }
    parse_number(&values.last()?[1])
}

fn parse_number(text: &str) -> Option<f64> {
    text.replace(',', ".").parse().ok()
}

/// "ping: connect: Network is unreachable" -> "Network is unreachable"
fn strip_program_prefix(line: &str) -> &str {
    let mut text = line;
    for prefix in ["ping6: ", "ping: ", "PING: ", "connect: ", "sendto: ", "sendmsg: ", "socket: "] {
        text = text.strip_prefix(prefix).unwrap_or(text);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn reply(rtt_ms: f64, ttl: Option<u8>, bytes: Option<usize>) -> Outcome {
        Outcome::Reply { rtt_ms, ttl, bytes }
    }

    #[test]
    fn linux_iputils() {
        let ok = "PING 192.168.1.1 (192.168.1.1) 56(84) bytes of data.\n\
                  64 bytes from 192.168.1.1: icmp_seq=1 ttl=64 time=0.412 ms\n\
                  \n\
                  --- 192.168.1.1 ping statistics ---\n\
                  1 packets transmitted, 1 received, 0% packet loss, time 0ms\n\
                  rtt min/avg/max/mdev = 0.412/0.412/0.412/0.000 ms\n";
        assert_eq!(parse_output(ok, true), reply(0.412, Some(64), Some(64)));

        let slow = "PING 8.8.8.8 (8.8.8.8) 1400(1428) bytes of data.\n\
                    1408 bytes from 8.8.8.8: icmp_seq=1 ttl=117 time=1024 ms\n";
        assert_eq!(parse_output(slow, true), reply(1024.0, Some(117), Some(1408)));

        let v6 = "PING ::1(::1) 56 data bytes\n\
                  64 bytes from ::1: icmp_seq=1 ttl=64 time=0.045 ms\n";
        assert_eq!(parse_output(v6, true), reply(0.045, Some(64), Some(64)));

        let timeout = "PING 10.255.255.1 (10.255.255.1) 56(84) bytes of data.\n\
                       \n\
                       --- 10.255.255.1 ping statistics ---\n\
                       1 packets transmitted, 0 received, 100% packet loss, time 0ms\n\n";
        assert_eq!(parse_output(timeout, false), Outcome::NoReply);

        let unreachable = "PING 192.168.1.77 (192.168.1.77) 56(84) bytes of data.\n\
                           From 192.168.1.10 icmp_seq=1 Destination Host Unreachable\n\
                           \n\
                           --- 192.168.1.77 ping statistics ---\n\
                           1 packets transmitted, 0 received, +1 errors, 100% packet loss, time 0ms\n";
        assert_eq!(
            parse_output(unreachable, false),
            Outcome::Unreachable("Destination Host Unreachable (reported by 192.168.1.10)".into())
        );

        let filtered = "PING 10.9.9.9 (10.9.9.9) 56(84) bytes of data.\n\
                        From 10.0.0.1 icmp_seq=1 Packet filtered\n";
        assert_eq!(
            parse_output(filtered, false),
            Outcome::Unreachable("Packet filtered (reported by 10.0.0.1)".into())
        );

        let ttl = "PING 1.1.1.1 (1.1.1.1) 56(84) bytes of data.\n\
                   From 10.0.0.1 icmp_seq=1 Time to live exceeded\n";
        assert_eq!(
            parse_output(ttl, false),
            Outcome::Unreachable("Time to live exceeded (reported by 10.0.0.1)".into())
        );

        assert_eq!(
            parse_output("\nping: connect: Network is unreachable\n", false),
            Outcome::Unreachable("Network is unreachable".into())
        );
        assert_eq!(
            parse_output("ping: sendmsg: No route to host\n", false),
            Outcome::Unreachable("No route to host".into())
        );

        let denied = "ping: socktype: SOCK_RAW\n\
                      ping: socket: Operation not permitted\n\
                      ping: => missing cap_net_raw+p capability or setuid?\n";
        assert_eq!(
            parse_output(denied, false),
            Outcome::Denied("Operation not permitted".into())
        );
    }

    #[test]
    fn busybox() {
        let ok = "PING 10.0.0.1 (10.0.0.1): 56 data bytes\n\
                  64 bytes from 10.0.0.1: seq=0 ttl=64 time=0.331 ms\n\
                  \n\
                  --- 10.0.0.1 ping statistics ---\n\
                  1 packets transmitted, 1 packets received, 0% packet loss\n\
                  round-trip min/avg/max = 0.331/0.331/0.331 ms\n";
        assert_eq!(parse_output(ok, true), reply(0.331, Some(64), Some(64)));
    }

    #[test]
    fn macos() {
        let ok = "PING 192.168.1.1 (192.168.1.1): 56 data bytes\n\
                  64 bytes from 192.168.1.1: icmp_seq=0 ttl=64 time=1.234 ms\n\
                  \n\
                  --- 192.168.1.1 ping statistics ---\n\
                  1 packets transmitted, 1 packets received, 0.0% packet loss\n\
                  round-trip min/avg/max/stddev = 1.234/1.234/1.234/0.000 ms\n";
        assert_eq!(parse_output(ok, true), reply(1.234, Some(64), Some(64)));

        let v6 = "PING6(56=40+8+8 bytes) ::1 --> ::1\n\
                  16 bytes from ::1, icmp_seq=0 hlim=64 time=0.058 ms\n\
                  \n\
                  --- ::1 ping6 statistics ---\n\
                  1 packets transmitted, 1 packets received, 0.0% packet loss\n\
                  round-trip min/avg/max/std-dev = 0.058/0.058/0.058/0.000 ms\n";
        assert_eq!(parse_output(v6, true), reply(0.058, Some(64), None));

        let timeout = "PING 10.255.255.1 (10.255.255.1): 56 data bytes\n\
                       \n\
                       --- 10.255.255.1 ping statistics ---\n\
                       1 packets transmitted, 0 packets received, 100.0% packet loss\n";
        assert_eq!(parse_output(timeout, false), Outcome::NoReply);

        let unreachable = "PING 192.168.1.77 (192.168.1.77): 56 data bytes\n\
                           92 bytes from 192.168.1.1: Destination Host Unreachable\n\
                           Vr HL TOS  Len   ID Flg  off TTL Pro  cks      Src      Dst\n\
                           \x204  5  00 5400 a1b2   0 0000  3f  01 5c1b 192.168.1.10  192.168.1.77 \n\
                           \n\
                           --- 192.168.1.77 ping statistics ---\n\
                           1 packets transmitted, 0 packets received, 100.0% packet loss\n";
        assert_eq!(
            parse_output(unreachable, false),
            Outcome::Unreachable("Destination Host Unreachable (reported by 192.168.1.1)".into())
        );

        assert_eq!(
            parse_output("PING 10.1.1.1 (10.1.1.1): 56 data bytes\nping: sendto: No route to host\n", false),
            Outcome::Unreachable("No route to host".into())
        );
    }

    #[test]
    fn windows_english() {
        let ok = "\r\nPinging 192.168.1.1 with 32 bytes of data:\r\n\
                  Reply from 192.168.1.1: bytes=32 time=1ms TTL=64\r\n\
                  \r\n\
                  Ping statistics for 192.168.1.1:\r\n\
                  \x20   Packets: Sent = 1, Received = 1, Lost = 0 (0% loss),\r\n\
                  Approximate round trip times in milli-seconds:\r\n\
                  \x20   Minimum = 1ms, Maximum = 1ms, Average = 1ms\r\n";
        assert_eq!(parse_output(ok, true), reply(1.0, Some(64), None));

        let fast = "\r\nPinging 127.0.0.1 with 32 bytes of data:\r\n\
                    Reply from 127.0.0.1: bytes=32 time<1ms TTL=128\r\n";
        assert_eq!(parse_output(fast, true), reply(1.0, Some(128), None));

        let v6 = "\r\nPinging ::1 with 32 bytes of data:\r\nReply from ::1: time<1ms \r\n";
        assert_eq!(parse_output(v6, true), reply(1.0, None, None));

        let timeout = "\r\nPinging 10.255.255.1 with 32 bytes of data:\r\n\
                       Request timed out.\r\n\
                       \r\n\
                       Ping statistics for 10.255.255.1:\r\n\
                       \x20   Packets: Sent = 1, Received = 0, Lost = 1 (100% loss),\r\n";
        assert_eq!(parse_output(timeout, false), Outcome::NoReply);

        // Windows exits with status 0 here: the "reply" is a router's ICMP error.
        let unreachable = "\r\nPinging 192.168.1.77 with 32 bytes of data:\r\n\
                           Reply from 192.168.1.10: Destination host unreachable.\r\n\
                           \r\n\
                           Ping statistics for 192.168.1.77:\r\n\
                           \x20   Packets: Sent = 1, Received = 1, Lost = 0 (0% loss),\r\n";
        assert_eq!(
            parse_output(unreachable, false),
            Outcome::Unreachable("Destination host unreachable (reported by 192.168.1.10)".into())
        );

        let expired = "\r\nPinging 8.8.8.8 with 32 bytes of data:\r\n\
                       Reply from 10.0.0.1: TTL expired in transit.\r\n";
        assert_eq!(
            parse_output(expired, false),
            Outcome::Unreachable("TTL expired in transit (reported by 10.0.0.1)".into())
        );

        assert_eq!(
            parse_output("PING: transmit failed. General failure. \r\n", false),
            Outcome::NoReply
        );
    }

    #[test]
    fn windows_in_other_languages() {
        let german = "\r\nPing wird ausgeführt für 192.168.1.1 mit 32 Bytes Daten:\r\n\
                      Antwort von 192.168.1.1: Bytes=32 Zeit=3ms TTL=64\r\n\
                      \r\n\
                      Ping-Statistik für 192.168.1.1:\r\n\
                      \x20   Pakete: Gesendet = 1, Empfangen = 1, Verloren = 0\r\n\
                      \x20   (0% Verlust),\r\n\
                      Ca. Zeitangaben in Millisek.:\r\n\
                      \x20   Minimum = 3ms, Maximum = 3ms, Mittelwert = 3ms\r\n";
        assert_eq!(parse_output(german, true), reply(3.0, Some(64), None));

        let german_timeout = "\r\nPing wird ausgeführt für 10.255.255.1 mit 32 Bytes Daten:\r\n\
                              Zeitüberschreitung der Anforderung.\r\n\
                              \r\n\
                              Ping-Statistik für 10.255.255.1:\r\n\
                              \x20   Pakete: Gesendet = 1, Empfangen = 0, Verloren = 1\r\n\
                              \x20   (100% Verlust),\r\n";
        assert_eq!(parse_output(german_timeout, false), Outcome::NoReply);

        let german_unreachable = "\r\nPing wird ausgeführt für 192.168.1.77 mit 32 Bytes Daten:\r\n\
                                  Antwort von 192.168.1.10: Zielhost nicht erreichbar.\r\n";
        assert_eq!(
            parse_output(german_unreachable, false),
            Outcome::Unreachable("Zielhost nicht erreichbar (reported by 192.168.1.10)".into())
        );

        // French puts a space before the colon and inside the duration.
        let french = "\r\nEnvoi d'une requête 'Ping'  192.168.1.1 avec 32 octets de données :\r\n\
                      Réponse de 192.168.1.1 : octets=32 temps=12 ms TTL=64\r\n";
        assert_eq!(parse_output(french, true), reply(12.0, Some(64), None));
        let french_fast = "Réponse de 192.168.1.1 : octets=32 temps<1ms TTL=64\r\n";
        assert_eq!(parse_output(french_fast, true), reply(1.0, Some(64), None));

        let russian = "\r\nОбмен пакетами с 192.168.1.1 по с 32 байтами данных:\r\n\
                       Ответ от 192.168.1.1: число байт=32 время=2мс TTL=64\r\n";
        assert_eq!(parse_output(russian, true), reply(2.0, Some(64), None));

        // The same line after a lossy decode of the OEM code page: only ASCII survives.
        let garbled = "\r\n����� �������� � 192.168.1.1 �� � 32 ������� ������:\r\n\
                       ����� �� 192.168.1.1: ����� ����=32 �����=2�� TTL=64\r\n";
        assert_eq!(parse_output(garbled, true), reply(2.0, Some(64), None));

        let japanese = "\r\n192.168.1.1 に ping を送信しています 32 バイトのデータ:\r\n\
                        192.168.1.1 からの応答: バイト数 =32 時間 =1ms TTL=64\r\n";
        assert_eq!(parse_output(japanese, true), reply(1.0, Some(64), None));

        let chinese = "\r\n正在 Ping 192.168.1.1 具有 32 字节的数据:\r\n\
                       来自 192.168.1.1 的回复: 字节=32 时间<1ms TTL=64\r\n";
        assert_eq!(parse_output(chinese, true), reply(1.0, Some(64), None));

        let spanish_decimal_comma = "64 bytes desde 10.0.0.1: icmp_seq=1 ttl=64 tiempo=0,512 ms\n";
        assert_eq!(parse_output(spanish_decimal_comma, true), reply(0.512, Some(64), None));
    }

    #[test]
    fn statistics_alone_are_not_a_reply() {
        // No reply line: the summary's "= 3ms" values must not be taken for one.
        let only_summary = "    Packets: Sent = 1, Received = 0, Lost = 1 (100% loss),\r\n";
        assert_eq!(parse_output(only_summary, false), Outcome::NoReply);
        assert_eq!(parse_output("", false), Outcome::NoReply);
        assert_eq!(parse_output("\n\n\n", true), Outcome::NoReply);
    }

    #[test]
    fn a_reply_per_exit_status_with_unknown_output_is_flagged() {
        // Exit status 0 on Unix means "answered"; a reply must not be invented from it.
        let odd = "PONG from 10.0.0.1 in a format nobody has seen\n";
        assert_eq!(
            parse_output(odd, true),
            Outcome::Unrecognized("PONG from 10.0.0.1 in a format nobody has seen".into())
        );
        assert_eq!(parse_output(odd, false), Outcome::NoReply);
    }

    #[test]
    fn command_lines_pass_the_address_as_a_plain_argument() {
        let (program, args) = command_line("192.0.2.9".parse().unwrap(), Duration::from_millis(800), 56);
        assert!(program.starts_with("ping"));
        assert_eq!(args.last().map(String::as_str), Some("192.0.2.9"));
        assert!(args.contains(&"56".to_owned()));
        if cfg!(target_os = "linux") {
            assert_eq!(args, ["-n", "-c", "1", "-W", "1", "-s", "56", "-4", "192.0.2.9"]);
            let (_, v6) = command_line("2001:db8::1".parse().unwrap(), Duration::from_millis(2500), 0);
            assert_eq!(v6, ["-n", "-c", "1", "-W", "3", "-s", "0", "-6", "2001:db8::1"]);
        }
    }
}
