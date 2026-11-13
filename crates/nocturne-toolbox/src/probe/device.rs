//! `probe.device.ping` and `probe.device.traceroute` — probing *from* a
//! device.
//!
//! "Can the core switch reach it?" is a different question from "can I?", and
//! usually the right one. So the toolbox builds the platform's own ping
//! command and sends it through `connect::exec`, which means the profile's
//! command policy still decides whether the line may be sent at all: this
//! module can no more run `reload` on a router than any other reader can.
//!
//! Two rules hold the whole thing together.
//!
//! **The target is validated before it is interpolated.** Everything that is
//! not a host name or an address is refused ([`known::host_name`]), and the
//! source interface and VRF name are held to the same kind of allow-list, so
//! `8.8.8.8; reload`, a backtick or a newline cannot reach a command line.
//!
//! **A command is only built for a platform family it can be built correctly
//! for.** The families below are the ones whose ping syntax is known; anything
//! else is `unsupported`, naming the profile. Guessing at a command for an
//! unknown platform is how a tool ends up typing something unintended into a
//! router.

use std::sync::LazyLock;

use nocturne_app::model::Device;
use nocturne_app::{App, AppError, AppResult, Router, connect, inventory, profiles};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::probe::history::{self, Run};
use crate::probe::known;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct DeviceProbeParams {
    /// The device to probe from: its id, name or address.
    pub device_id: String,
    /// Where to probe: an address or a host name.
    pub target: String,
    /// Echoes to send (ignored by the families whose command has no count).
    pub count: Option<u32>,
    /// The source interface or address the device should use.
    pub source: Option<String>,
    /// The VRF / routing instance to probe in.
    pub vrf: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DeviceProbeResult {
    /// The command that was sent, exactly as it was sent.
    pub command: String,
    /// What the device printed, unchanged.
    pub output: String,
    /// The numbers, when the output is in a format this crate can read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    /// The platform family the command was built for.
    pub family: &'static str,
    /// Where it ran: the device itself.
    pub from: String,
    pub device_id: String,
    pub device_name: String,
    pub run_id: String,
    /// The device's own error line, when it rejected the command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Summary {
    pub sent: u32,
    pub received: u32,
    pub loss_pct: f64,
    pub min_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub max_ms: Option<f64>,
}

/// The platform families whose probe commands are known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// IOS, IOS-XE and EOS: `ping <target> repeat <n>`.
    IosRepeat,
    /// NX-OS and IOS-XR: `ping <target> count <n>`.
    IosCount,
    /// Junos: `ping <target> count <n>`, `routing-instance` for the VRF.
    Junos,
    /// MikroTik RouterOS: `/ping <target> count=<n>`.
    RouterOs,
    /// Huawei VRP and H3C Comware: `ping -c <n> <target>`, `tracert`.
    Vrp,
    /// A POSIX shell: `ping -c <n> <target>`.
    Linux,
}

impl Family {
    pub fn name(self) -> &'static str {
        match self {
            Family::IosRepeat => "ios-like (IOS, IOS-XE, EOS)",
            Family::IosCount => "ios-like with count (NX-OS, IOS-XR)",
            Family::Junos => "junos",
            Family::RouterOs => "routeros",
            Family::Vrp => "vrp (Huawei, H3C Comware)",
            Family::Linux => "linux",
        }
    }
}

/// Which family a profile belongs to, from what the profile says about itself.
///
/// The profile is data (`docs/profiles.md`), so this reads its `os`, `vendor`
/// and configuration syntax rather than hard-coding a list of device types —
/// and returns `None` rather than a guess, which is what makes the refusal
/// below truthful.
pub fn family_of(id: &str, os: &str, vendor: &str, syntax: &str) -> Option<Family> {
    // `IOS-XE` and `IOS XE` are the same thing, and `FortiOS` is not an IOS:
    // the joined form (`iosxr`) is matched whole and the word form (`ios`,
    // `xe`) only against whole words, so no platform is claimed by accident.
    let joined = |text: &str| {
        text.chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let words = |text: &str| {
        text.to_ascii_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_string)
            .collect::<Vec<String>>()
    };
    let has = |text: &str, wanted: &[&str]| {
        words(text)
            .iter()
            .any(|word| wanted.contains(&word.as_str()))
    };
    let (os_joined, vendor_joined, syntax) = (joined(os), joined(vendor), joined(syntax));

    if os_joined == "routeros" || vendor_joined == "mikrotik" || syntax == "routeros" {
        return Some(Family::RouterOs);
    }
    if has(os, &["junos"]) || has(id, &["junos"]) {
        return Some(Family::Junos);
    }
    if matches!(os_joined.as_str(), "iosxr" | "nxos")
        || has(id, &["iosxr", "nxos"])
        || has(os, &["nxos"])
    {
        return Some(Family::IosCount);
    }
    if has(os, &["ios", "eos"]) || has(id, &["ios", "eos"]) {
        return Some(Family::IosRepeat);
    }
    if matches!(os_joined.as_str(), "vrp" | "comware")
        || matches!(vendor_joined.as_str(), "huawei" | "h3c")
        || syntax == "vrp"
    {
        return Some(Family::Vrp);
    }
    if has(os, &["linux", "vyos", "edgeos"]) || has(id, &["linux"]) {
        return Some(Family::Linux);
    }
    None
}

/// An interface name or source address the device will accept, or why not.
fn source_name(source: &str) -> AppResult<String> {
    let text = source.trim();
    let allowed = text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/' | ':'));
    if text.is_empty() || !allowed || text.len() > 64 {
        return Err(AppError::invalid(format!(
            "`{text}` is not an interface name or an address, and this text goes onto the device's command line"
        )));
    }
    Ok(text.to_string())
}

/// A VRF / routing-instance name the device will accept, or why not.
fn vrf_name(vrf: &str) -> AppResult<String> {
    let text = vrf.trim();
    let allowed = text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if text.is_empty() || !allowed || text.len() > 64 {
        return Err(AppError::invalid(format!(
            "`{text}` is not a VRF name, and this text goes onto the device's command line"
        )));
    }
    Ok(text.to_string())
}

/// The ping command for a family. Every piece of `target`, `source` and `vrf`
/// has been validated by the caller.
pub fn ping_command(
    family: Family,
    target: &str,
    count: u32,
    source: Option<&str>,
    vrf: Option<&str>,
) -> AppResult<String> {
    let unsupported = |what: &str| {
        Err(AppError::new(
            "unsupported",
            format!(
                "{what} is not something this toolbox can ask a {} device for without risking a command it did not mean",
                family.name()
            ),
        ))
    };
    Ok(match family {
        Family::IosRepeat => {
            let mut command = String::from("ping ");
            if let Some(vrf) = vrf {
                command.push_str(&format!("vrf {vrf} "));
            }
            command.push_str(&format!("{target} repeat {count}"));
            if let Some(source) = source {
                command.push_str(&format!(" source {source}"));
            }
            command
        }
        Family::IosCount => {
            let mut command = format!("ping {target} count {count}");
            if let Some(source) = source {
                command.push_str(&format!(" source {source}"));
            }
            if let Some(vrf) = vrf {
                command.push_str(&format!(" vrf {vrf}"));
            }
            command
        }
        Family::Junos => {
            let mut command = format!("ping {target} count {count}");
            if let Some(source) = source {
                command.push_str(&format!(" source {source}"));
            }
            if let Some(vrf) = vrf {
                command.push_str(&format!(" routing-instance {vrf}"));
            }
            command
        }
        Family::RouterOs => {
            let mut command = format!("/ping {target} count={count}");
            if let Some(source) = source {
                command.push_str(&format!(" interface={source}"));
            }
            if let Some(vrf) = vrf {
                command.push_str(&format!(" routing-table={vrf}"));
            }
            command
        }
        Family::Vrp => {
            let mut command = format!("ping -c {count}");
            if let Some(source) = source {
                command.push_str(&format!(" -a {source}"));
            }
            if let Some(vrf) = vrf {
                command.push_str(&format!(" -vpn-instance {vrf}"));
            }
            command.push_str(&format!(" {target}"));
            command
        }
        Family::Linux => {
            if source.is_some() {
                // `ping -I eth0 …` is not in the Linux profile's read-only
                // list, and rightly so; refusing is better than sending a line
                // the policy will stop anyway.
                return unsupported("a source interface");
            }
            if vrf.is_some() {
                return unsupported("a VRF");
            }
            format!("ping -c {count} {target}")
        }
    })
}

/// The traceroute command for a family.
pub fn trace_command(
    family: Family,
    target: &str,
    source: Option<&str>,
    vrf: Option<&str>,
) -> AppResult<String> {
    let unsupported = |what: &str| {
        Err(AppError::new(
            "unsupported",
            format!(
                "{what} is not something this toolbox can ask a {} device for without risking a command it did not mean",
                family.name()
            ),
        ))
    };
    Ok(match family {
        Family::IosRepeat | Family::IosCount => {
            let mut command = String::from("traceroute ");
            if let Some(vrf) = vrf {
                command.push_str(&format!("vrf {vrf} "));
            }
            command.push_str(target);
            if let Some(source) = source {
                command.push_str(&format!(" source {source}"));
            }
            command
        }
        Family::Junos => {
            let mut command = format!("traceroute {target}");
            if let Some(source) = source {
                command.push_str(&format!(" source {source}"));
            }
            if let Some(vrf) = vrf {
                command.push_str(&format!(" routing-instance {vrf}"));
            }
            command
        }
        Family::RouterOs => {
            let mut command = format!("/tool traceroute {target} count=1");
            if let Some(source) = source {
                command.push_str(&format!(" interface={source}"));
            }
            if let Some(vrf) = vrf {
                command.push_str(&format!(" routing-table={vrf}"));
            }
            command
        }
        Family::Vrp => {
            let mut command = String::from("tracert");
            if let Some(source) = source {
                command.push_str(&format!(" -a {source}"));
            }
            if let Some(vrf) = vrf {
                command.push_str(&format!(" -vpn-instance {vrf}"));
            }
            command.push_str(&format!(" {target}"));
            command
        }
        Family::Linux => {
            if source.is_some() || vrf.is_some() {
                return unsupported("a source interface or a VRF");
            }
            format!("traceroute {target}")
        }
    })
}

/// `Success rate is 100 percent (5/5), round-trip min/avg/max = 1/2/4 ms`
static IOS_RATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Success rate is \d+ percent \((?<received>\d+)/(?<sent>\d+)\)").expect("valid")
});
static IOS_RTT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"min/avg/max\s*=\s*(?<min>\d+(?:\.\d+)?)/(?<avg>\d+(?:\.\d+)?)/(?<max>\d+(?:\.\d+)?)",
    )
    .expect("valid")
});
/// `5 packets transmitted, 5 received, 0% packet loss` (Linux, Junos, NX-OS, VRP)
static PACKETS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?<sent>\d+) packets? transmitted, (?<received>\d+)(?: packets?)? received")
        .expect("valid")
});
/// `rtt min/avg/max/mdev = 0.1/0.2/0.3/0.0 ms` and the `round-trip` spellings.
static UNIX_RTT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"min/avg/max(?:/[a-z]+)?\s*=\s*(?<min>\d+(?:\.\d+)?)/(?<avg>\d+(?:\.\d+)?)/(?<max>\d+(?:\.\d+)?)",
    )
    .expect("valid")
});
/// `sent=5 received=5 packet-loss=0% min-rtt=1ms avg-rtt=2ms max-rtt=3ms`
static ROUTEROS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"sent=(?<sent>\d+)\s+received=(?<received>\d+)").expect("valid"));
static ROUTEROS_RTT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"min-rtt=(?<min>\d+)ms\s+avg-rtt=(?<avg>\d+)ms\s+max-rtt=(?<max>\d+)ms")
        .expect("valid")
});

/// Read the numbers out of a ping's output, or `None` when the format is not
/// one of the recognised ones — in which case the output is still returned in
/// full, because the device's own words are the answer and a made-up summary
/// would not be.
pub fn parse_summary(output: &str) -> Option<Summary> {
    let number = |text: &str| text.parse::<f64>().ok();
    let (sent, received) = IOS_RATE
        .captures(output)
        .or_else(|| PACKETS.captures(output))
        .or_else(|| ROUTEROS.captures(output))
        .and_then(|caps| {
            Some((
                caps.name("sent")?.as_str().parse::<u32>().ok()?,
                caps.name("received")?.as_str().parse::<u32>().ok()?,
            ))
        })?;
    let rtt = IOS_RTT
        .captures(output)
        .or_else(|| UNIX_RTT.captures(output))
        .or_else(|| ROUTEROS_RTT.captures(output));
    let (min, avg, max) = match rtt {
        Some(caps) => (
            caps.name("min").and_then(|m| number(m.as_str())),
            caps.name("avg").and_then(|m| number(m.as_str())),
            caps.name("max").and_then(|m| number(m.as_str())),
        ),
        None => (None, None, None),
    };
    Some(Summary {
        sent,
        received,
        loss_pct: if sent == 0 {
            0.0
        } else {
            ((f64::from(sent - received.min(sent)) * 1000.0 / f64::from(sent)).round()) / 10.0
        },
        min_ms: min,
        avg_ms: avg,
        max_ms: max,
    })
}

/// The family of a device, or the refusal that names its profile.
fn family_for(app: &App, device: &Device) -> AppResult<Family> {
    let profile_id = if device.profile.trim().is_empty() {
        // An undetected device has no profile yet, and the detected one is
        // kept in its state record.
        inventory::state_of(app, &device.id)?
            .detected_profile
            .unwrap_or_default()
    } else {
        device.profile.clone()
    };
    if profile_id.trim().is_empty() {
        return Err(AppError::new(
            "unsupported",
            format!(
                "{} has no device type yet, so there is no ping command to build. Connect to it once so that Nocturne can detect it.",
                device.name
            ),
        ));
    }
    let platform = profiles::platform(app, &profile_id)?;
    let profile = &platform.profile;
    let syntax = serde_json::to_value(profile.config.syntax)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default();
    family_of(&profile.id, &profile.os, &profile.vendor, &syntax).ok_or_else(|| {
        AppError::new(
            "unsupported",
            format!(
                "Nocturne does not know how to build a ping command for `{}` ({}). It can for IOS and IOS-XE, NX-OS and IOS-XR, EOS, Junos, RouterOS, Huawei VRP, H3C Comware and Linux.",
                profile.id, profile.name
            ),
        )
    })
}

async fn run(app: &App, params: DeviceProbeParams, trace: bool) -> AppResult<DeviceProbeResult> {
    let device = inventory::find(app, &params.device_id)?;
    let family = family_for(app, &device)?;
    // Validated before it is anywhere near a command line.
    let target = known::host_name(&params.target)?;
    let source = params
        .source
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(source_name)
        .transpose()?;
    let vrf = params
        .vrf
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(vrf_name)
        .transpose()?;
    let count = params.count.unwrap_or(5).clamp(1, 100);
    let command = if trace {
        trace_command(family, &target, source.as_deref(), vrf.as_deref())?
    } else {
        ping_command(family, &target, count, source.as_deref(), vrf.as_deref())?
    };

    let kind = if trace {
        "device.traceroute"
    } else {
        "device.ping"
    };
    let from = format!("{} itself", device.name);
    let mut record = Run::plain(
        kind,
        format!(
            "{} {target} from {}",
            if trace { "Traceroute to" } else { "Ping" },
            device.name
        ),
        &from,
        &target,
    );
    record.method = Some(command.clone());
    record.device = Some(super::known::DeviceHint {
        id: device.id.clone(),
        name: device.name.clone(),
        address: device.address.clone(),
        role: device.role.clone(),
        site: device.site.clone(),
        lab: device.lab.is_some(),
        matched: "the device the probe ran on".into(),
    });
    history::begin(app, &record);

    // Through the read-only gate: if the platform's policy does not class this
    // line as a read, it is not sent at all.
    match connect::exec(app, &device.id, std::slice::from_ref(&command)).await {
        Ok(outputs) => {
            let output = outputs.into_iter().next().ok_or_else(|| {
                AppError::internal("the device session returned no output for the command")
            })?;
            let summary = if trace {
                None
            } else {
                parse_summary(&output.output)
            };
            let result = DeviceProbeResult {
                command: output.command,
                summary,
                family: family.name(),
                from: from.clone(),
                device_id: device.id.clone(),
                device_name: device.name.clone(),
                run_id: record.id.clone(),
                error: output.error,
                output: output.output,
            };
            let line = match (&result.error, &result.summary) {
                (Some(error), _) => format!("the device refused the command: {error}"),
                (None, Some(summary)) => format!(
                    "{}/{} replies, {:.0}% loss{}",
                    summary.received,
                    summary.sent,
                    summary.loss_pct,
                    match summary.avg_ms {
                        Some(avg) => format!(", avg {avg} ms"),
                        None => String::new(),
                    }
                ),
                (None, None) => format!("{} bytes of output", result.output.len()),
            };
            history::succeeded(app, &mut record, line, &result);
            Ok(result)
        }
        Err(error) => {
            history::failed(app, &mut record, &error);
            Err(error)
        }
    }
}

pub fn register(router: &mut Router) {
    router.add(
        "probe.device.ping",
        |app: App, params: DeviceProbeParams| async move { run(&app, params, false).await },
    );
    router.add(
        "probe.device.traceroute",
        |app: App, params: DeviceProbeParams| async move { run(&app, params, true).await },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_family_comes_from_what_the_profile_says_about_itself() {
        assert_eq!(
            family_of("cisco_ios", "IOS", "Cisco", "ios"),
            Some(Family::IosRepeat)
        );
        assert_eq!(
            family_of("cisco_nxos", "NX-OS", "Cisco", "ios"),
            Some(Family::IosCount)
        );
        assert_eq!(
            family_of("cisco_iosxr", "IOS-XR", "Cisco", "ios"),
            Some(Family::IosCount)
        );
        assert_eq!(
            family_of("arista_eos", "EOS", "Arista", "ios"),
            Some(Family::IosRepeat)
        );
        assert_eq!(
            family_of("juniper_junos", "Junos", "Juniper", "set"),
            Some(Family::Junos)
        );
        assert_eq!(
            family_of("mikrotik_routeros", "RouterOS", "MikroTik", "routeros"),
            Some(Family::RouterOs)
        );
        assert_eq!(
            family_of("huawei_vrp", "VRP", "Huawei", "vrp"),
            Some(Family::Vrp)
        );
        assert_eq!(
            family_of("linux", "Linux", "Linux", "flat"),
            Some(Family::Linux)
        );
        // The test profile of the fake device is an IOS-like one.
        assert_eq!(
            family_of("test_ios", "IOS", "Test", "ios"),
            Some(Family::IosRepeat)
        );

        // Nothing is guessed: a platform nobody has taught it is refused.
        assert_eq!(family_of("generic", "", "", "flat"), None);
        assert_eq!(family_of("fortios", "FortiOS", "Fortinet", "flat"), None);
    }

    #[test]
    fn each_family_gets_its_own_command() {
        let ping = |family| ping_command(family, "10.0.0.9", 5, None, None).unwrap();
        assert_eq!(ping(Family::IosRepeat), "ping 10.0.0.9 repeat 5");
        assert_eq!(ping(Family::IosCount), "ping 10.0.0.9 count 5");
        assert_eq!(ping(Family::Junos), "ping 10.0.0.9 count 5");
        assert_eq!(ping(Family::RouterOs), "/ping 10.0.0.9 count=5");
        assert_eq!(ping(Family::Vrp), "ping -c 5 10.0.0.9");
        assert_eq!(ping(Family::Linux), "ping -c 5 10.0.0.9");

        // The VRF goes where each platform puts it.
        assert_eq!(
            ping_command(
                Family::IosRepeat,
                "10.0.0.9",
                2,
                Some("Vlan10"),
                Some("MGMT")
            )
            .unwrap(),
            "ping vrf MGMT 10.0.0.9 repeat 2 source Vlan10"
        );
        assert_eq!(
            ping_command(Family::Junos, "10.0.0.9", 2, Some("10.0.0.1"), Some("mgmt")).unwrap(),
            "ping 10.0.0.9 count 2 source 10.0.0.1 routing-instance mgmt"
        );
        assert_eq!(
            ping_command(Family::RouterOs, "10.0.0.9", 2, Some("ether1"), None).unwrap(),
            "/ping 10.0.0.9 count=2 interface=ether1"
        );
        assert_eq!(
            ping_command(Family::Vrp, "10.0.0.9", 2, Some("10.0.0.1"), Some("mgmt")).unwrap(),
            "ping -c 2 -a 10.0.0.1 -vpn-instance mgmt 10.0.0.9"
        );
        // Linux: a source interface would not pass the profile's read-only
        // policy, so it is refused here instead of being sent and rejected.
        let refused = ping_command(Family::Linux, "10.0.0.9", 2, Some("eth0"), None).unwrap_err();
        assert_eq!(refused.code, "unsupported");

        assert_eq!(
            trace_command(Family::IosRepeat, "10.0.0.9", None, Some("MGMT")).unwrap(),
            "traceroute vrf MGMT 10.0.0.9"
        );
        assert_eq!(
            trace_command(Family::Vrp, "10.0.0.9", None, None).unwrap(),
            "tracert 10.0.0.9"
        );
        assert_eq!(
            trace_command(Family::RouterOs, "10.0.0.9", None, None).unwrap(),
            "/tool traceroute 10.0.0.9 count=1"
        );
    }

    #[test]
    fn the_pieces_that_go_on_a_command_line_are_allow_listed() {
        for bad in ["Vlan10; reload", "eth0 && reload", "`reload`", "", "a b"] {
            assert_eq!(source_name(bad).unwrap_err().code, "invalid", "{bad}");
        }
        for bad in ["MGMT;reload", "a/b", "", "x y"] {
            assert_eq!(vrf_name(bad).unwrap_err().code, "invalid", "{bad}");
        }
        assert_eq!(
            source_name(" GigabitEthernet0/1 ").unwrap(),
            "GigabitEthernet0/1"
        );
        assert_eq!(vrf_name("MGMT-vrf_1").unwrap(), "MGMT-vrf_1");
    }

    // The outputs below are written from each platform's documented ping
    // output (the formats the netsim renders and the engines parse), not
    // recorded from hardware this crate has access to.
    #[test]
    fn the_summary_is_read_from_each_platforms_own_wording() {
        let ios = "Type escape sequence to abort.\nSending 5, 100-byte ICMP Echos to 10.0.0.9, timeout is 2 seconds:\n!!!!!\nSuccess rate is 100 percent (5/5), round-trip min/avg/max = 1/2/4 ms";
        assert_eq!(
            parse_summary(ios).unwrap(),
            Summary {
                sent: 5,
                received: 5,
                loss_pct: 0.0,
                min_ms: Some(1.0),
                avg_ms: Some(2.0),
                max_ms: Some(4.0),
            }
        );

        // Two of five lost, and no round-trip line because nothing but
        // timeouts came back for some of them.
        let ios_loss = "Sending 5, 100-byte ICMP Echos to 10.0.0.9, timeout is 2 seconds:\n..!.!\nSuccess rate is 40 percent (2/5)";
        let summary = parse_summary(ios_loss).unwrap();
        assert_eq!(
            (summary.sent, summary.received, summary.loss_pct),
            (5, 2, 60.0)
        );
        assert_eq!(summary.avg_ms, None, "no average is invented");

        let junos = "PING 10.0.0.9 (10.0.0.9): 56 data bytes\n64 bytes from 10.0.0.9: icmp_seq=0 ttl=64 time=1.234 ms\n--- 10.0.0.9 ping statistics ---\n5 packets transmitted, 5 packets received, 0% packet loss\nround-trip min/avg/max/stddev = 1.100/2.200/3.300/0.500 ms";
        assert_eq!(
            parse_summary(junos).unwrap(),
            Summary {
                sent: 5,
                received: 5,
                loss_pct: 0.0,
                min_ms: Some(1.1),
                avg_ms: Some(2.2),
                max_ms: Some(3.3),
            }
        );

        let linux = "PING 10.0.0.9 (10.0.0.9) 56(84) bytes of data.\n--- 10.0.0.9 ping statistics ---\n5 packets transmitted, 4 received, 20% packet loss, time 4005ms\nrtt min/avg/max/mdev = 0.045/0.067/0.089/0.015 ms";
        let summary = parse_summary(linux).unwrap();
        assert_eq!(
            (summary.sent, summary.received, summary.loss_pct),
            (5, 4, 20.0)
        );
        assert_eq!(summary.max_ms, Some(0.089));

        let routeros = "  SEQ HOST                                     SIZE TTL TIME       STATUS\n    0 10.0.0.9                                   56  64 1ms\n    sent=5 received=5 packet-loss=0% min-rtt=1ms avg-rtt=2ms max-rtt=3ms";
        assert_eq!(
            parse_summary(routeros).unwrap(),
            Summary {
                sent: 5,
                received: 5,
                loss_pct: 0.0,
                min_ms: Some(1.0),
                avg_ms: Some(2.0),
                max_ms: Some(3.0),
            }
        );

        // An unknown format is no summary at all, not a summary of zeroes.
        assert_eq!(
            parse_summary("% Invalid input detected at '^' marker."),
            None
        );
        assert_eq!(parse_summary(""), None);
    }
}
