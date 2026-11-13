//! Drives the system `traceroute` / `tracert` and turns its output into hops as it appears.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use super::parse::{Flavor, HopLine, TraceParser, flag_meaning};
use super::{ProbeResult, Trace, TraceOptions, build_hop};
use crate::{ProbeError, process};

/// What became of one run of the program.
enum Attempt {
    /// It printed hops (or ran to the end without finding any).
    Done,
    /// It exited with an error before printing a hop; the text is what it said.
    Failed(String),
}

/// Traces with the system program, pushing hops into `trace` as they are printed.
/// Returns the method name for the report.
pub(super) async fn run(
    trace: &mut Trace,
    options: &TraceOptions,
) -> Result<&'static str, ProbeError> {
    let v6 = trace.target.is_ipv6();
    let address = trace.target.to_string();
    let probes = options.probes.to_string();
    let hops = options.max_hops.to_string();
    let wait_ms = options.timeout_ms.to_string();
    let wait_s = options.timeout_ms.div_ceil(1000).max(1).to_string();

    if cfg!(windows) {
        let family = if v6 { "-6" } else { "-4" };
        let args = ["-d", "-h", &hops, "-w", &wait_ms, family, &address];
        return match attempt(trace, options, "tracert", &args, Flavor::Windows).await? {
            Attempt::Done => Ok("system-tracert"),
            Attempt::Failed(said) => Err(failure("tracert", &said)),
        };
    }

    let bsd = cfg!(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ));
    let program = if v6 && bsd {
        "traceroute6"
    } else {
        "traceroute"
    };
    let mut args = vec!["-n", "-q", &probes, "-w", &wait_s, "-m", &hops];
    if !bsd {
        args.push(if v6 { "-6" } else { "-4" });
    }
    args.push(&address);
    let said = match attempt(trace, options, program, &args, Flavor::Unix).await? {
        Attempt::Done => return Ok("system-traceroute"),
        Attempt::Failed(said) => said,
    };
    if !looks_like_usage_error(&said) {
        return Err(failure(program, &said));
    }
    // inetutils' traceroute knows neither -n nor -4/-6; the plain options are common to all.
    let plain = ["-q", &probes, "-w", &wait_s, "-m", &hops, &address];
    match attempt(trace, options, program, &plain, Flavor::Unix).await? {
        Attempt::Done => Ok("system-traceroute"),
        Attempt::Failed(said) => Err(failure(program, &said)),
    }
}

async fn attempt(
    trace: &mut Trace,
    options: &TraceOptions,
    program: &str,
    args: &[&str],
    flavor: Flavor,
) -> Result<Attempt, ProbeError> {
    let Some(mut command) = process::command(program) else {
        return Err(ProbeError::unsupported(format!(
            "the system `{program}` program was not found; install it to trace routes \
             (for example `apt install traceroute`)"
        )));
    };
    let mut child = command.args(args).spawn()?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    // Windows always sends three probes per hop.
    let probes_per_hop = match flavor {
        Flavor::Windows => 3,
        Flavor::Unix => usize::from(options.probes),
    };
    let mut parser = TraceParser::new(flavor, probes_per_hop);
    let hops_before = trace.hops.len();

    // Generous: a sequential tracer waits for every probe of every hop in turn.
    let limit = Duration::from_millis(options.timeout_ms + 1000)
        * u32::from(options.max_hops)
        * u32::from(options.probes.max(3))
        + Duration::from_secs(5);
    let reading = async {
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines.next_line().await? {
            for hop in parser.feed(&line) {
                record(trace, hop, options.resolve_names).await;
            }
        }
        if let Some(hop) = parser.finish() {
            record(trace, hop, options.resolve_names).await;
        }
        let mut said = String::new();
        stderr.read_to_string(&mut said).await?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status.success(), said))
    };
    let (success, said) = match tokio::time::timeout(limit, reading).await {
        Ok(result) => result?,
        // Overran: the child is killed when dropped; what was traced so far stands.
        Err(_) => return Ok(Attempt::Done),
    };
    if !success && trace.hops.len() == hops_before {
        return Ok(Attempt::Failed(said));
    }
    // The program stops before max_hops only at the destination or at a dead end. A last hop
    // that answered without an ICMP error is therefore the destination, even when it answered
    // from another of its addresses.
    let stopped_early = trace.hops.last().is_some_and(|hop| {
        hop.ttl < options.max_hops && hop.address.is_some() && hop.note.is_empty()
    });
    trace.reached |= stopped_early;
    Ok(Attempt::Done)
}

/// Converts a parsed line into a hop of the trace.
async fn record(trace: &mut Trace, line: HopLine, resolve_names: bool) {
    let message = line.message;
    let probes: Vec<ProbeResult> = line
        .probes
        .into_iter()
        .map(|probe| {
            // Windows names the ICMP error in words next to the router that reported it.
            let reported = message
                .clone()
                .filter(|_| probe.address.is_some() && probe.rtt_ms.is_none());
            ProbeResult {
                address: probe.address,
                rtt_ms: probe.rtt_ms,
                problem: probe.flag.as_deref().map(flag_meaning).or(reported),
            }
        })
        .collect();
    if probes
        .iter()
        .any(|probe| probe.address == Some(trace.target) && probe.problem.is_none())
    {
        trace.reached = true;
    }
    let hop = build_hop(line.ttl, &probes, resolve_names).await;
    trace.push(hop);
}

fn looks_like_usage_error(said: &str) -> bool {
    let said = said.to_ascii_lowercase();
    [
        "invalid option",
        "unrecognized option",
        "illegal option",
        "unknown option",
        "usage:",
        "bad option",
    ]
    .iter()
    .any(|hint| said.contains(hint))
}

/// Explains why the system program could not trace.
fn failure(program: &str, said: &str) -> ProbeError {
    let first_line = said
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("it exited with an error and no message");
    let lower = said.to_ascii_lowercase();
    if lower.contains("not permitted")
        || lower.contains("permission")
        || lower.contains("must be root")
    {
        return ProbeError::Permission(format!(
            "the system `{program}` may not send probes: {first_line}"
        ));
    }
    if lower.contains("unreachable") || lower.contains("no route") {
        return ProbeError::Unreachable(first_line.to_owned());
    }
    ProbeError::protocol(format!("the system `{program}` failed: {first_line}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_are_classified() {
        assert!(looks_like_usage_error(
            "traceroute: invalid option -- 'n'\nTry 'traceroute --help'"
        ));
        assert!(looks_like_usage_error("Usage: traceroute [OPTION...] HOST"));
        assert!(!looks_like_usage_error("connect: Network is unreachable"));

        let denied = failure(
            "traceroute",
            "traceroute: icmp socket: Operation not permitted\n",
        );
        assert!(matches!(denied, ProbeError::Permission(_)), "{denied:?}");
        let unreachable = failure("traceroute", "\nconnect: Network is unreachable\n");
        assert!(
            matches!(&unreachable, ProbeError::Unreachable(text) if text == "connect: Network is unreachable")
        );
        let other = failure("tracert", "");
        assert!(other.to_string().contains("`tracert` failed"), "{other}");
    }
}
