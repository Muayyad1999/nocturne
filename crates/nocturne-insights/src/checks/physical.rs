//! Ports and the cables between them.

use nocturne_drivers::model::Interface;

use crate::checks::{NO_LINK, link_ends, owner, skip_link, skip_rest};
use crate::links::Link;
use crate::logs;
use crate::metrics;
use crate::model::{Finding, Severity, Suggestion};
use crate::norm::{self, Duplex};
use crate::report::CheckReport;
use crate::settings::Settings;
use crate::world::{DeviceData, World};

/// Speeds at which half duplex is a fault rather than a period detail.
/// 10BASE-T hubs were half duplex by design; 100 Mb/s and above with a switch
/// at the far end never is.
const HALF_DUPLEX_IS_WRONG_FROM: u64 = 100_000_000;

// ----------------------------------------------------------- duplex mismatch

/// The two ends of a link settled on different duplex.
///
/// Look-alikes this stays quiet about, and why:
///
/// * **`auto` on one end.** A port that has not finished negotiating, or
///   whose platform prints `auto` without the outcome, has no duplex to
///   compare. `a-full` *is* a verdict (auto-negotiated to full) and is read as
///   full; a bare `auto` is not.
/// * **Half duplex at 10 Mb/s on both ends.** Legacy equipment, a media
///   converter, an old industrial sensor: both ends agree, which is what
///   matters. (If the ends *disagree* at 10 Mb/s it is still a mismatch and is
///   still reported — the exception is about agreement, not about speed.)
/// * **A port with two neighbours, or a neighbour that is not in the
///   inventory.** There is no "other end" to compare with; see `links`.
pub fn duplex_mismatch(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for link in world.target_links() {
        let (a, a_port, b, b_port) = match link_ends(world, link) {
            Ok(ends) => ends,
            Err(reason) => {
                skip_link(report, link, reason);
                continue;
            }
        };
        let (a_duplex, b_duplex) = (norm::duplex(&a_port.duplex), norm::duplex(&b_port.duplex));
        if a_duplex == Duplex::Unresolved || b_duplex == Duplex::Unresolved {
            skip_link(
                report,
                link,
                "one end does not report a resolved duplex (still negotiating, or not collected)",
            );
            continue;
        }
        report.ok(a.id());
        report.ok(b.id());
        if a_duplex == b_duplex {
            continue;
        }
        // The half-duplex end is where the collisions are, and it is the end
        // worth changing — so it owns the finding, deterministically.
        let (near, near_port, far, far_port) = if a_duplex == Duplex::Half {
            (a, a_port, b, b_port)
        } else {
            (b, b_port, a, a_port)
        };
        let mut finding = Finding::new(
            "duplex_mismatch",
            near.id(),
            &near_port.name,
            format!(
                "{} {} is half duplex while {} {} is full duplex",
                near.name(),
                near_port.name,
                far.name(),
                far_port.name
            ),
        )
        .detail(
            "A duplex mismatch does not stop the link coming up: it degrades it. The half-duplex \
             end counts late collisions and the full-duplex end counts CRC errors, and throughput \
             collapses as soon as both directions are busy at once.",
        )
        .evidence(
            format!("{} {}", near.name(), near_port.name),
            describe(near_port),
        )
        .evidence(
            format!("{} {}", far.name(), far_port.name),
            describe(far_port),
        )
        .evidence("Link", link_evidence(link))
        .related(far.id(), far.name(), &far_port.name)
        .suggest(duplex_suggestion(near, near_port, far, far_port));
        if near_port
            .speed_bps
            .is_some_and(|speed| speed >= 1_000_000_000)
        {
            // A gigabit link cannot legitimately be half duplex at all.
            finding = finding.severity(Severity::High);
        }
        report.found(finding);
    }
    skip_rest(world, report, NO_LINK);
}

fn describe(port: &Interface) -> String {
    match port.speed_bps {
        Some(speed) => format!(
            "{} duplex, {} (as reported: `{}`)",
            norm::duplex(&port.duplex).as_str(),
            norm::speed(speed),
            port.duplex
        ),
        None => format!(
            "{} duplex (as reported: `{}`)",
            norm::duplex(&port.duplex).as_str(),
            port.duplex
        ),
    }
}

fn link_evidence(link: &Link) -> String {
    format!(
        "{} {} ↔ {} {} ({}, {})",
        link.a.device,
        link.a.interface,
        link.b.device,
        link.b.interface,
        link.how,
        if link.confirmed {
            "both ends report it"
        } else {
            "reported by one end"
        }
    )
}

/// A fix is only offered when the configuration shows which end is forcing
/// the duplex. Setting `duplex auto` on the end that is *already* negotiating
/// changes nothing, and guessing would mean bouncing a live uplink for
/// nothing.
fn duplex_suggestion(
    near: &DeviceData,
    near_port: &Interface,
    far: &DeviceData,
    far_port: &Interface,
) -> Suggestion {
    let forced = |data: &DeviceData, port: &Interface| -> Option<String> {
        let section = norm::ios_section(data, &port.name)?;
        section
            .children
            .iter()
            .find(|child| child.text.starts_with("duplex ") && child.text != "duplex auto")
            .map(|child| child.text.clone())
    };
    let commands = [
        format!("show interfaces {}", near_port.name),
        format!("show interfaces {}", far_port.name),
    ];
    match (forced(near, near_port), forced(far, far_port)) {
        (Some(line), _) => Suggestion::text(format!(
            "{} {} has `{line}` in its configuration. Set both ends to auto-negotiate, starting \
             here; the link will drop for a few seconds while it renegotiates.",
            near.name(),
            near_port.name
        ))
        .lines([
            format!("interface {}", near_port.name),
            "duplex auto".into(),
        ])
        .commands(commands),
        (None, Some(line)) => Suggestion::text(format!(
            "{} {} negotiates, but {} {} has `{line}` forced. The change belongs on {}, not here.",
            near.name(),
            near_port.name,
            far.name(),
            far_port.name,
            far.name()
        ))
        .commands(commands),
        (None, None) => Suggestion::text(
            "Neither end forces duplex in its stored configuration, so one of them is older than \
             the configuration Nocturne holds or is set outside it. Confirm on both devices before \
             changing anything.",
        )
        .commands(commands),
    }
}

// ------------------------------------------------------- half duplex uplink

/// A port with a network device plugged into it, running half duplex.
///
/// Look-alikes this stays quiet about:
///
/// * **10 Mb/s half duplex**, which is simply what 10BASE-T does.
/// * **Ports whose far end is managed and full duplex** — that is a duplex
///   mismatch and `duplex_mismatch` reports it, with both ends' evidence.
/// * **Ports with no neighbour at all**: a half-duplex printer or a badge
///   reader is not a network fault.
pub fn half_duplex_uplink(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for data in world.targets() {
        let Some(interfaces) = data.interfaces.as_ref() else {
            report.skip(data.id(), "no interface data collected");
            continue;
        };
        if data.neighbors.is_none() {
            report.skip(
                data.id(),
                "no neighbours collected, so no port is known to be a link",
            );
            continue;
        }
        for port in &interfaces.rows {
            if !norm::is_physical(&port.name) {
                continue;
            }
            let Some(neighbor) = data.neighbors_of(&port.name).next() else {
                continue;
            };
            if norm::duplex(&port.duplex) == Duplex::Unresolved {
                report.skip(data.id(), "the port does not report a resolved duplex");
                continue;
            }
            report.ok(data.id());
            if norm::duplex(&port.duplex) != Duplex::Half {
                continue;
            }
            match port.speed_bps {
                Some(speed) if speed < HALF_DUPLEX_IS_WRONG_FROM => continue,
                None => continue,
                Some(_) => {}
            }
            // Leave the two-managed-ends case to `duplex_mismatch`.
            let managed_full = world.links.iter().any(|link| {
                link.ends_from(data.id()).is_some_and(|(near, far)| {
                    crate::world::canonical(&near.interface) == crate::world::canonical(&port.name)
                        && world
                            .device(&far.device_id)
                            .and_then(|far_data| far_data.interface(&far.interface))
                            .is_some_and(|far_port| norm::duplex(&far_port.duplex) == Duplex::Full)
                })
            });
            if managed_full {
                continue;
            }
            report.found(
                Finding::new(
                    "half_duplex_uplink",
                    data.id(),
                    &port.name,
                    format!(
                        "{} is half duplex at {} with {} plugged into it",
                        port.name,
                        port.speed_bps.map(norm::speed).unwrap_or_default(),
                        if neighbor.device.is_empty() {
                            "a network device"
                        } else {
                            neighbor.device.as_str()
                        }
                    ),
                )
                .detail(
                    "Above 10 Mb/s, half duplex between two network devices is the result of a \
                     forced setting on one side or of failed auto-negotiation. The link works \
                     until it is busy, then collapses into collisions.",
                )
                .evidence("Port", describe(port))
                .evidence(
                    "Neighbour",
                    format!(
                        "{} {} ({})",
                        neighbor.device, neighbor.remote_interface, neighbor.protocol
                    ),
                )
                .suggest(
                    Suggestion::text(
                        "Set both ends to auto-negotiate, or hard-code both to the same speed and \
                         duplex. Changing one end alone is what caused this.",
                    )
                    .commands([format!("show interfaces {}", port.name)]),
                ),
            );
        }
    }
    skip_rest(world, report, "no port with a neighbour");
}

// ------------------------------------------------------------ speed and MTU

/// The two ends of an *up* link report different speeds.
///
/// Look-alikes: a port that is **down** reports its configured maximum rather
/// than a negotiated speed, so a down link whose ends "disagree" is not a
/// finding; and a port that reports no speed at all is not compared.
pub fn speed_mismatch(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for link in world.target_links() {
        let (a, a_port, b, b_port) = match link_ends(world, link) {
            Ok(ends) => ends,
            Err(reason) => {
                skip_link(report, link, reason);
                continue;
            }
        };
        if a_port.oper_up != Some(true) || b_port.oper_up != Some(true) {
            skip_link(
                report,
                link,
                "the link is not up at both ends, so the reported speeds are configured \
                 maximums rather than a negotiated speed",
            );
            continue;
        }
        let (Some(a_speed), Some(b_speed)) = (a_port.speed_bps, b_port.speed_bps) else {
            skip_link(report, link, "one end reports no speed");
            continue;
        };
        report.ok(a.id());
        report.ok(b.id());
        if a_speed == b_speed {
            continue;
        }
        let (owner_id, _) = owner(a.id(), b.id());
        let (near, near_port, far, far_port) = if owner_id == a.id() {
            (a, a_port, b, b_port)
        } else {
            (b, b_port, a, a_port)
        };
        report.found(
            Finding::new(
                "speed_mismatch",
                near.id(),
                &near_port.name,
                format!(
                    "{} {} runs at {} but {} {} reports {}",
                    near.name(),
                    near_port.name,
                    norm::speed(near_port.speed_bps.unwrap_or_default()),
                    far.name(),
                    far_port.name,
                    norm::speed(far_port.speed_bps.unwrap_or_default())
                ),
            )
            .detail(
                "Two ends of one cable cannot run at different speeds. One of them is reporting \
                 a forced speed that the other did not take, or the link is not the cable \
                 Nocturne thinks it is — both are worth looking at before the next outage.",
            )
            .evidence(
                format!("{} {}", near.name(), near_port.name),
                describe(near_port),
            )
            .evidence(
                format!("{} {}", far.name(), far_port.name),
                describe(far_port),
            )
            .evidence("Link", link_evidence(link))
            .related(far.id(), far.name(), &far_port.name)
            .suggest(
                Suggestion::text(
                    "Confirm on both devices. If the speeds really differ, one end is hard-coded: \
                     set both to auto-negotiate during a maintenance window.",
                )
                .commands([
                    format!("show interfaces {}", near_port.name),
                    format!("show interfaces {}", far_port.name),
                ]),
            ),
        );
    }
    skip_rest(world, report, NO_LINK);
}

/// MTU slack that is a counting convention rather than a difference: the
/// 14-byte Ethernet header plus the 4-byte FCS.
const MTU_SLACK: u64 = 18;

/// The two ends of a link are configured with different MTUs.
///
/// Look-alike: **1500 against 1514** (and 9216 against 9198) is the same MTU
/// counted differently — some platforms include the Ethernet header and FCS
/// and some do not. Anything within 18 bytes is left alone.
pub fn mtu_mismatch(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for link in world.target_links() {
        let (a, a_port, b, b_port) = match link_ends(world, link) {
            Ok(ends) => ends,
            Err(reason) => {
                skip_link(report, link, reason);
                continue;
            }
        };
        if a_port.oper_up != Some(true) || b_port.oper_up != Some(true) {
            skip_link(report, link, "the link is not up at both ends");
            continue;
        }
        let (Some(a_mtu), Some(b_mtu)) = (a_port.mtu, b_port.mtu) else {
            skip_link(report, link, "one end reports no MTU");
            continue;
        };
        report.ok(a.id());
        report.ok(b.id());
        if a_mtu.abs_diff(b_mtu) <= MTU_SLACK {
            continue;
        }
        let (owner_id, _) = owner(a.id(), b.id());
        let (near, near_port, far, far_port) = if owner_id == a.id() {
            (a, a_port, b, b_port)
        } else {
            (b, b_port, a, a_port)
        };
        report.found(
            Finding::new(
                "mtu_mismatch",
                near.id(),
                &near_port.name,
                format!(
                    "MTU {} on {} {} against {} on {} {}",
                    near_port.mtu.unwrap_or_default(),
                    near.name(),
                    near_port.name,
                    far_port.mtu.unwrap_or_default(),
                    far.name(),
                    far_port.name
                ),
            )
            .detail(
                "Small packets cross this link and large ones are dropped, which looks like an \
                 application fault rather than a network one: file transfers and tunnels stall \
                 while ping succeeds.",
            )
            .evidence(
                format!("{} {}", near.name(), near_port.name),
                format!("MTU {}", near_port.mtu.unwrap_or_default()),
            )
            .evidence(
                format!("{} {}", far.name(), far_port.name),
                format!("MTU {}", far_port.mtu.unwrap_or_default()),
            )
            .evidence("Link", link_evidence(link))
            .related(far.id(), far.name(), &far_port.name)
            .suggest(
                Suggestion::text(
                    "Decide which MTU the link should carry and set both ends to it. Raising an \
                     MTU is safe; lowering one mid-path is what breaks tunnels.",
                )
                .commands([
                    format!("show interfaces {}", near_port.name),
                    format!("show interfaces {}", far_port.name),
                ]),
            ),
        );
    }
    skip_rest(world, report, NO_LINK);
}

// --------------------------------------------------------- interface errors

/// Input errors growing between the last two collections.
///
/// Look-alikes this stays quiet about:
///
/// * **Errors since boot.** Every port that was ever unplugged has a non-zero
///   counter. Only the difference between two collections counts.
/// * **A counter that went backwards** (`clear counters`, a reboot, a 32-bit
///   wrap): there is no rate to compute, so the port is not evaluated.
/// * **A handful of errors on a busy port.** The rate is measured against the
///   packets the port actually carried, so one error in ten million is not a
///   finding; and a growth smaller than the minimum is never one.
pub fn interface_errors(world: &World, settings: &Settings, report: &mut CheckReport) {
    let thresholds = &settings.thresholds;
    for data in world.targets() {
        let (Some(now), Some(before)) =
            (data.interfaces.as_ref(), data.previous_interfaces.as_ref())
        else {
            report.skip(
                data.id(),
                "only one collection of interfaces: a counter on its own is not a rate",
            );
            continue;
        };
        let elapsed_ms = now.at - before.at;
        if elapsed_ms < 60_000 {
            report.skip(
                data.id(),
                "the two collections are less than a minute apart, which is too short to read a rate from",
            );
            continue;
        }
        let minutes = elapsed_ms as f64 / 60_000.0;
        for port in &now.rows {
            let Some(was) = data.previous_interface(&port.name) else {
                continue;
            };
            let delta = |now: Option<u64>, before: Option<u64>| -> Option<Option<u64>> {
                match (now, before) {
                    (Some(now), Some(before)) => Some(now.checked_sub(before)),
                    _ => None,
                }
            };
            let errors = delta(port.counters.in_errors, was.counters.in_errors);
            let crcs = delta(port.counters.crc, was.counters.crc);
            if errors.is_none() && crcs.is_none() {
                report.skip(data.id(), "no error counters were collected");
                continue;
            }
            if errors == Some(None) || crcs == Some(None) {
                report.skip(
                    data.id(),
                    "an error counter went backwards (counters cleared, a restart, or a wrap)",
                );
                continue;
            }
            report.ok(data.id());
            // IOS counts CRCs inside input errors; the larger growth is the
            // honest figure rather than their sum.
            let grew = errors
                .flatten()
                .unwrap_or(0)
                .max(crcs.flatten().unwrap_or(0));
            if grew < thresholds.interface_error_min_delta {
                continue;
            }
            let packets = delta(port.counters.in_packets, was.counters.in_packets).flatten();
            let (rate_label, rate, over) = match packets {
                Some(packets) if packets > 0 => {
                    let ppm = grew as f64 * 1_000_000.0 / packets as f64;
                    (
                        format!("{ppm:.0} errors per million packets ({packets} packets)"),
                        ppm,
                        ppm >= thresholds.interface_error_ppm,
                    )
                }
                _ => {
                    let per_minute = grew as f64 / minutes;
                    (
                        format!("{per_minute:.1} errors a minute (packet counts not collected)"),
                        per_minute,
                        per_minute >= 1.0,
                    )
                }
            };
            if !over {
                continue;
            }
            let severity = if rate >= thresholds.interface_error_ppm * 10.0 {
                Severity::High
            } else {
                Severity::Medium
            };
            report.found(
                Finding::new(
                    "interface_errors",
                    data.id(),
                    &port.name,
                    format!("{} is taking input errors: {rate_label}", port.name),
                )
                .severity(severity)
                .detail(
                    "Input errors are counted on receipt, so the fault is in the cable, the \
                     transceiver, or what the far end is transmitting — not in this port's own \
                     configuration. Growing CRCs on a copper link usually mean the cable, a \
                     connector, or a duplex mismatch.",
                )
                .evidence(
                    "Growth",
                    format!(
                        "+{grew} over {:.0} minutes ({} → {})",
                        minutes,
                        was.counters.in_errors.or(was.counters.crc).unwrap_or(0),
                        port.counters.in_errors.or(port.counters.crc).unwrap_or(0)
                    ),
                )
                .evidence("Rate", rate_label)
                .evidence("Port", describe(port))
                .suggest(
                    Suggestion::text(
                        "Check the cable and the transceiver at both ends, and the far end's \
                         duplex. Clear the counters once the cause is found so the next scan \
                         measures the repair.",
                    )
                    .commands([format!("show interfaces {}", port.name)]),
                ),
            );
        }
    }
    skip_rest(world, report, "no interface data collected");
}

// -------------------------------------------------------------- flapping

/// State *changes* per port, from the device's own link log lines: IOS
/// reports one flap twice (`%LINK-3-UPDOWN` and `%LINEPROTO-5-UPDOWN`), and
/// counting lines would halve the threshold without saying so. Counted this
/// way the figure means the same thing as the sampled one.
fn changes_logged(events: &[logs::LinkEvent]) -> Vec<(String, u32, i64)> {
    let mut per_interface: std::collections::BTreeMap<&str, Vec<&logs::LinkEvent>> =
        Default::default();
    for event in events {
        per_interface
            .entry(event.interface.as_str())
            .or_default()
            .push(event);
    }
    per_interface
        .into_iter()
        .map(|(interface, mut seen)| {
            seen.sort_by_key(|event| event.at);
            let mut changes = 0;
            let mut state: Option<bool> = None;
            for event in &seen {
                if state.is_some_and(|was| was != event.up) {
                    changes += 1;
                }
                state = Some(event.up);
            }
            let last = seen.last().map(|event| event.at).unwrap_or_default();
            (interface.to_string(), changes, last)
        })
        .collect()
}

/// A port that went up and down repeatedly within the hour.
///
/// Counted from polled `if.oper` samples where monitoring took some in the
/// hour, and otherwise from the device's own link up/down log lines. A device
/// with neither is not evaluated: a port nobody watched is not a port that
/// was stable — and neither is one whose samples stop before the hour starts
/// (monitoring stopped polling it), or whose samples or log entries could not
/// be read.
///
/// Look-alike: a port that is **administratively down now** flapped before
/// somebody shut it; that is history, not a fault, and is not reported.
pub fn interface_flapping(world: &World, settings: &Settings, report: &mut CheckReport) {
    let since = world.now - 3_600_000;
    let threshold = settings.thresholds.flap_transitions_per_hour;
    // One read for the whole fleet. A failure stays one, so that it reads as
    // "could not be read" and never as "nothing was logged".
    let events = logs::link_events(world.store(), since);

    for data in world.targets() {
        let sampled = if world.has_metric(data.id(), "if.oper") {
            metrics::transitions(world.store(), data.id(), "if.oper", since)
                .map(|counted| Some(counted).filter(|counted| !counted.is_empty()))
        } else {
            Ok(None)
        };
        let logged = events
            .as_ref()
            .map(|events| events.get(data.id()).map(Vec::as_slice));
        let (counted, from_samples) = match (sampled, logged) {
            (Ok(Some(counted)), _) => (counted, true),
            (_, Ok(Some(events))) => (changes_logged(events), false),
            (Err(_), Err(_)) => {
                report.skip(
                    data.id(),
                    "neither the device's `if.oper` samples nor the log entries could be read",
                );
                continue;
            }
            (Err(_), Ok(None)) => {
                report.skip(
                    data.id(),
                    "the device's `if.oper` samples could not be read, and it logged no link \
                     changes in the last hour to count instead",
                );
                continue;
            }
            (Ok(None), Err(_)) => {
                report.skip(
                    data.id(),
                    "the log entries could not be read, and monitoring took no `if.oper` samples \
                     of this device in the last hour",
                );
                continue;
            }
            (Ok(None), Ok(None)) => {
                report.skip(
                    data.id(),
                    "neither monitoring samples nor log entries for this device in the last hour",
                );
                continue;
            }
        };
        report.ok(data.id());
        for (instance, transitions, last) in counted {
            if transitions < threshold {
                continue;
            }
            // A port somebody has since shut is not flapping any more.
            if let Some(port) = data.interface(&instance)
                && port.admin_up == Some(false)
            {
                continue;
            }
            let name = data
                .interface(&instance)
                .map(|port| port.name.clone())
                .unwrap_or_else(|| nocturne_drivers::ifname::long(&instance));
            report.found(
                Finding::new(
                    "interface_flapping",
                    data.id(),
                    &name,
                    format!("{name} changed state {transitions} times in the last hour"),
                )
                .detail(
                    "A link that keeps coming and going takes spanning tree, routing adjacencies \
                     and MAC tables with it every time. The usual causes are a failing \
                     transceiver, a damaged or too-long cable, power to the far end, or a device \
                     at the far end restarting in a loop.",
                )
                .evidence(
                    "Transitions",
                    format!(
                        "{transitions} in the last hour (threshold {threshold}), last at {last}"
                    ),
                )
                .evidence(
                    "Source",
                    if from_samples {
                        "`if.oper` samples from monitoring".to_string()
                    } else {
                        "link up/down log entries from the device".to_string()
                    },
                )
                .suggest(
                    Suggestion::text(
                        "Look at the far end and the transceiver first. If the port must stay \
                         down until it is repaired, shut it deliberately rather than leaving it \
                         flapping.",
                    )
                    .commands([format!("show interfaces {name}")]),
                ),
            );
        }
    }
    skip_rest(world, report, "no interface history for this device");
}

// ------------------------------------------------------------ err-disabled

/// A port the switch disabled itself and that is still down.
///
/// Look-alikes: a port that **recovered** (err-disable recovery brought it
/// back, or somebody bounced it) is not reported, and neither is one that has
/// since been **shut** deliberately.
pub fn err_disabled(world: &World, settings: &Settings, report: &mut CheckReport) {
    let since = world.now - i64::from(settings.thresholds.history_hours) * 3_600_000;
    // Failures stay failures: an empty answer here is "nothing was disabled"
    // and "the device sends no log", both verdicts.
    let events = logs::err_disable_events(world.store(), since);
    let ids: Vec<&str> = world.targets().map(|data| data.id()).collect();
    let with_logs = logs::devices_with_logs(world.store(), &ids, since);

    for data in world.targets() {
        if data.interfaces.is_none() {
            report.skip(
                data.id(),
                "no interface data to confirm whether the port is still down",
            );
            continue;
        }
        let (Ok(with_logs), Ok(events)) = (&with_logs, &events) else {
            report.skip(data.id(), "the device's log entries could not be read");
            continue;
        };
        if !with_logs.contains(data.id()) {
            report.skip(
                data.id(),
                "the device sent no log entries to Nocturne in the window",
            );
            continue;
        }
        report.ok(data.id());
        let mut seen: Vec<String> = Vec::new();
        for event in events.get(data.id()).map(Vec::as_slice).unwrap_or_default() {
            if seen.contains(&event.interface) {
                continue;
            }
            seen.push(event.interface.clone());
            let Some(port) = data.interface(&event.interface) else {
                continue;
            };
            if port.oper_up == Some(true) || port.admin_up == Some(false) {
                continue;
            }
            report.found(
                Finding::new(
                    "err_disabled",
                    data.id(),
                    &port.name,
                    format!("{} was disabled by the switch and is still down", port.name),
                )
                .detail(
                    "The switch shut the port itself to protect the network. Until the cause is \
                     dealt with, whatever is plugged into it has no connection — and err-disable \
                     recovery, if it is on, will shut it again.",
                )
                .evidence("Cause", event.cause.clone())
                .evidence("Log entry", event.message.clone())
                .evidence(
                    "Port now",
                    format!(
                        "admin {}, link {}",
                        state(port.admin_up),
                        state(port.oper_up)
                    ),
                )
                .suggest(
                    Suggestion::text(
                        "Find what triggered it before bringing the port back: a port in \
                         err-disable is usually protecting the network from a loop or an \
                         unauthorised device. `shutdown` then `no shutdown` on the port clears the \
                         state once the cause is gone.",
                    )
                    .commands([
                        "show interfaces status err-disabled".into(),
                        format!("show interfaces {}", port.name),
                    ]),
                ),
            );
        }
    }
    skip_rest(world, report, "no log entries from this device");
}

fn state(flag: Option<bool>) -> &'static str {
    match flag {
        Some(true) => "up",
        Some(false) => "down",
        None => "unknown",
    }
}

// ------------------------------------------------------------- down uplink

/// A port with a known neighbour that is up administratively but has no link.
///
/// Look-alike: a port somebody **shut** is somebody's decision, not a fault.
/// A port whose neighbour was never collected is not an uplink as far as this
/// check is concerned — it may be a desk port that nobody is using.
pub fn down_uplink(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for data in world.targets() {
        let Some(interfaces) = data.interfaces.as_ref() else {
            report.skip(data.id(), "no interface data collected");
            continue;
        };
        if data.neighbors.is_none() && data.previous_neighbors.is_none() {
            report.skip(
                data.id(),
                "no neighbours were ever collected for this device",
            );
            continue;
        }
        report.ok(data.id());
        for port in &interfaces.rows {
            if port.admin_up != Some(true) || port.oper_up != Some(false) {
                continue;
            }
            // The neighbour has gone from the live table with the link, so the
            // previous collection is what says this port had one.
            let seen = data.neighbors_of(&port.name).next().cloned().or_else(|| {
                data.previous_neighbors.as_ref().and_then(|previous| {
                    previous
                        .rows
                        .iter()
                        .find(|neighbor| {
                            crate::world::canonical(&neighbor.local_interface)
                                == crate::world::canonical(&port.name)
                        })
                        .cloned()
                })
            });
            let Some(neighbor) = seen else {
                continue;
            };
            report.found(
                Finding::new(
                    "down_uplink",
                    data.id(),
                    &port.name,
                    format!(
                        "{} is up but has no link, and {} was plugged into it",
                        port.name,
                        if neighbor.device.is_empty() {
                            "a network device".to_string()
                        } else {
                            neighbor.device.clone()
                        }
                    ),
                )
                .detail(
                    "The port is not shut, so nobody turned this off on purpose: the cable, the \
                     transceiver or the device at the far end has failed. Where this was a \
                     redundant path, the network is now running without its backup.",
                )
                .evidence(
                    "Port",
                    format!(
                        "admin {}, link {}",
                        state(port.admin_up),
                        state(port.oper_up)
                    ),
                )
                .evidence(
                    "Neighbour last seen",
                    format!(
                        "{} {} ({})",
                        neighbor.device, neighbor.remote_interface, neighbor.protocol
                    ),
                )
                .suggest(
                    Suggestion::text(
                        "Check the far end is powered and its port is up, then the cable and the \
                         transceivers. If the far end is gone for good, shut the port so it stops \
                         being reported.",
                    )
                    .commands([format!("show interfaces {}", port.name)]),
                ),
            );
        }
    }
    skip_rest(world, report, "no interface data collected");
}

// -------------------------------------------------------- high utilisation

/// A port that spent a long stretch over the utilisation threshold.
///
/// Look-alike: **one busy poll** is not congestion, so a port must be over
/// the threshold for most of a window of at least the sustained period, with
/// enough samples in it to mean anything.
pub fn high_utilisation(world: &World, settings: &Settings, report: &mut CheckReport) {
    let thresholds = &settings.thresholds;
    let window = i64::from(thresholds.sustained_minutes.max(15)) * 4 * 60_000;
    let since = world.now - window;
    let history = world.now - i64::from(thresholds.history_hours) * 3_600_000;

    for data in world.targets() {
        if !world.has_metric(data.id(), "if.util_pct") {
            report.skip(
                data.id(),
                "no interface utilisation samples for this device",
            );
            continue;
        }
        let Ok(summary) = metrics::summarise(
            world.store(),
            data.id(),
            "if.util_pct",
            since,
            thresholds.utilisation_pct,
        ) else {
            report.skip(data.id(), "the utilisation samples could not be read");
            continue;
        };
        if summary.is_empty() {
            report.skip(data.id(), "no utilisation samples inside the window");
            continue;
        }
        report.ok(data.id());
        for series in summary {
            if series.points < 5
                || series.span_minutes() < i64::from(thresholds.sustained_minutes)
                || series.over_fraction() < 0.75
            {
                continue;
            }
            let name = data
                .interface(&series.instance)
                .map(|port| port.name.clone())
                .unwrap_or_else(|| series.instance.clone());
            let peak = metrics::busiest_hour(
                world.store(),
                data.id(),
                "if.util_pct",
                &series.instance,
                history,
            );
            let mut finding = Finding::new(
                "high_utilisation",
                data.id(),
                &name,
                format!(
                    "{name} has been over {:.0} % for {} minutes (average {:.0} %, peak {:.0} %)",
                    thresholds.utilisation_pct,
                    series.span_minutes(),
                    series.avg,
                    series.max
                ),
            )
            .detail(
                "A link this busy is dropping or delaying traffic at every burst, and the \
                 applications on it will be blamed for it. Either the traffic has somewhere else \
                 to go, or the link needs to be faster or bundled.",
            )
            .evidence(
                "Window",
                format!(
                    "{} samples over {} minutes, {:.0} % of them above the threshold",
                    series.points,
                    series.span_minutes(),
                    series.over_fraction() * 100.0
                ),
            )
            .evidence("Peak", format!("{:.1} %", series.max))
            .suggest(
                Suggestion::text(
                    "Look at what is on the link before changing it: a backup job on the wrong \
                     path is cheaper to move than a port is to upgrade.",
                )
                .commands([format!("show interfaces {name}")]),
            );
            match peak {
                Ok(Some((hour, average))) => {
                    finding = finding.evidence(
                        "Busiest hour",
                        format!("{hour:02}:00 UTC, averaging {average:.0} %"),
                    );
                }
                Ok(None) => {}
                Err(_) => finding = finding.evidence("Busiest hour", "could not be read"),
            }
            report.found(finding);
        }
    }
    skip_rest(world, report, "no utilisation samples for this device");
}

#[cfg(test)]
#[path = "physical_tests.rs"]
mod tests;
