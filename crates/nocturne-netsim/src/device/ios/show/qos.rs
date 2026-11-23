//! `show policy-map` and `show policy-map interface`.
//!
//! The layout, the wording and the indentation are taken line by line from
//! the sample output in Cisco's command reference ("Cisco IOS Quality of
//! Service Solutions Command Reference", chapter "show mls qos through
//! wrr-queue threshold", commands `show policy-map` and `show policy-map
//! interface`, the examples for hierarchical queueing):
//! <https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos/command/qos-cr-book/qos-s4.html>.
//! `docs/dev/spec-netsim.md` lists which example each line comes from, and the
//! three places where the reference shows no example and the line is built by
//! analogy.

use std::fmt::Write as _;

use crate::device::config::{CLASS_DEFAULT, ClassMatch, PolicyClass, Share};
use crate::device::ios::runcfg::match_text;
use crate::ifname::IfId;
use crate::sim::node::Node;
use crate::sim::qos::{
    self, ClassStats, Level, PACKET_BYTES, PlanClass, PolicyStats, QUEUE_LIMIT_PACKETS,
};
use crate::util::dscp;

/// A policer's default burst: a quarter of a second at the rate, in bytes,
/// and never less than one large packet.
fn police_burst(cir: u64) -> u64 {
    (cir / 32).max(1500)
}

/// A shaper's default burst: what the rate sends in a 4 ms interval, in bits.
fn shape_burst(rate: u64) -> u64 {
    rate / 250
}

/// A priority queue's default burst: 200 ms at the rate, in bytes, in steps
/// of 50 and never less than one large packet. The step is read off the
/// reference's samples, which state no rule: 20 % of 1536000 bit/s prints as
/// `(307 kbps), burst bytes 7650`, where 200 ms would be 7675.
fn priority_burst(kbps: u64) -> u64 {
    (kbps * 25 / 50 * 50).max(1500)
}

/// Whole units of a counter kept as a fraction. The counters are sums of
/// products, so a value that is whole on paper can sit a hair below it; the
/// margin is far smaller than anything that could be a real fraction.
fn whole(amount: f64) -> u64 {
    (amount + 1e-3) as u64
}

fn packets(octets: f64) -> u64 {
    whole(octets / PACKET_BYTES)
}

/// Rates are shown to the nearest kilobit below, as the samples show them.
fn kilo(rate: f64) -> u64 {
    (rate / 1000.0) as u64 * 1000
}

// ------------------------------------------------------------ configuration

/// `show policy-map [NAME]`: the policy maps as configured.
pub(crate) fn policy_map(node: &Node, only: Option<&str>) -> String {
    let mut out = String::new();
    let mut first = true;
    for map in node
        .cfg
        .policy_maps
        .iter()
        .filter(|map| only.is_none_or(|name| map.name == name))
    {
        if !std::mem::take(&mut first) {
            out.push('\n');
        }
        let _ = writeln!(out, "  Policy Map {}", map.name);
        for class in &map.classes {
            let _ = writeln!(out, "    Class {}", class.name);
            match class.priority {
                Some(Some(Share::Kbps(kbps))) => {
                    let _ = writeln!(out, "      priority {kbps} (kbps)");
                }
                Some(Some(Share::Percent(percent))) => {
                    let _ = writeln!(out, "      priority {percent} (%)");
                }
                Some(None) => out.push_str("      priority\n"),
                None => {}
            }
            match class.bandwidth {
                Some(Share::Kbps(kbps)) => {
                    let _ = writeln!(out, "      bandwidth {kbps} (kbps)");
                }
                Some(Share::Percent(percent)) => {
                    let _ = writeln!(out, "      bandwidth {percent} (%)");
                }
                None => {}
            }
            if let Some(police) = class.police {
                let _ = writeln!(
                    out,
                    "     police cir {} bc {}",
                    police.cir,
                    police_burst(police.cir)
                );
                out.push_str("       conform-action transmit\n       exceed-action drop\n");
            }
            if let Some(rate) = class.shape {
                out.push_str("      Average Rate Traffic Shaping\n");
                let _ = writeln!(out, "      cir {rate} (bps)");
            }
            if let Some(value) = class.set_dscp {
                let _ = writeln!(out, "      set dscp {}", dscp::config_text(value));
            }
            if let Some(child) = &class.child {
                let _ = writeln!(out, "      service-policy {child}");
            }
        }
    }
    out
}

// --------------------------------------------------------------- statistics

struct Render<'a> {
    node: &'a Node,
    stats: Option<&'a PolicyStats>,
    /// Output direction: classes have queues.
    output: bool,
    out: String,
}

impl Render<'_> {
    fn line(&mut self, indent: usize, text: &str) {
        let _ = writeln!(self.out, "{:indent$}{text}", "");
    }

    fn stats(&self, path: &[String]) -> ClassStats {
        self.stats
            .and_then(|stats| stats.classes.get(path))
            .cloned()
            .unwrap_or_default()
    }

    fn match_lines(&mut self, indent: usize, class: &PolicyClass) {
        if class.name == CLASS_DEFAULT {
            return self.line(indent, "Match: any");
        }
        let Some(map) = self.node.cfg.class_map(&class.name) else {
            return;
        };
        for statement in &map.matches {
            let text = match statement {
                // Statistics give the number beside the name: `dscp ef (46)`.
                ClassMatch::Dscp { values, ip } => format!(
                    "{}dscp {}",
                    if *ip { "ip " } else { "" },
                    values
                        .iter()
                        .map(|value| dscp::show_text(*value))
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
                other => match_text(other).trim_start_matches("match ").to_string(),
            };
            self.line(indent, &format!("Match: {text}"));
        }
    }

    /// The three lines every queue prints.
    fn queue_lines(&mut self, indent: usize, full: bool, dropped: f64, output: f64) {
        self.line(
            indent,
            &format!("queue limit {QUEUE_LIMIT_PACKETS} packets"),
        );
        self.line(
            indent,
            &format!(
                "(queue depth/total drops/no-buffer drops) {}/{}/0",
                if full { QUEUE_LIMIT_PACKETS } else { 0 },
                packets(dropped)
            ),
        );
        self.line(
            indent,
            &format!(
                "(pkts output/bytes output) {}/{}",
                packets(output),
                whole(output)
            ),
        );
    }

    /// One policy map: its classes at `indent`, their details two further in.
    /// `reference` is what a percentage is a percentage of.
    fn level(&mut self, level: &Level, indent: usize, reference: f64, prefix: &mut Vec<String>) {
        let body = indent + 2;
        let queueing = self.output && level.queues();
        let priority: Vec<&PlanClass> = level
            .classes
            .iter()
            .filter(|class| class.class.priority.is_some())
            .collect();
        if queueing && !priority.is_empty() {
            let (mut dropped, mut output) = (0.0, 0.0);
            for class in &priority {
                prefix.push(class.class.name.clone());
                let stats = self.stats(prefix);
                prefix.pop();
                dropped += stats.priority_drops;
                output += stats.queue_output;
            }
            self.out.push('\n');
            self.line(indent, "queue stats for all priority classes:");
            self.out.push('\n');
            // A priority queue is emptied first, so it never stands full: what
            // it loses is what its own rate limit turns away.
            self.queue_lines(body, false, dropped, output);
        }
        for class in &level.classes {
            prefix.push(class.class.name.clone());
            let stats = self.stats(prefix);
            let cfg = &class.class;
            let kind = match self.node.cfg.class_map(&cfg.name) {
                Some(map) if !map.any => "match-all",
                _ => "match-any",
            };
            self.out.push('\n');
            self.line(indent, &format!("Class-map: {} ({kind})", cfg.name));
            self.line(
                body,
                &format!(
                    "{} packets, {} bytes",
                    packets(stats.offered),
                    whole(stats.offered)
                ),
            );
            self.line(
                body,
                &format!(
                    "5 minute offered rate {} bps, drop rate {} bps",
                    kilo(stats.offered_rate),
                    kilo(stats.drop_rate)
                ),
            );
            self.match_lines(body, cfg);
            if queueing {
                match cfg.priority {
                    Some(limit) => {
                        let dropped = packets(stats.priority_drops);
                        let text = match limit {
                            Some(Share::Kbps(kbps)) => format!(
                                "Priority: {kbps} kbps, burst bytes {}, b/w exceed drops: {dropped}",
                                priority_burst(u64::from(kbps))
                            ),
                            Some(share @ Share::Percent(percent)) => {
                                let kbps = qos::share_kbps(share, reference);
                                format!(
                                    "Priority: {percent}% ({kbps} kbps), burst bytes {}, b/w exceed drops: {dropped}",
                                    priority_burst(kbps)
                                )
                            }
                            None => format!(
                                "Priority: 0 kbps, burst bytes 1500, b/w exceed drops: {dropped}"
                            ),
                        };
                        self.line(body, &text);
                    }
                    None if cfg.bandwidth.is_some() || cfg.shape.is_some() => {
                        self.line(body, "Queueing");
                        self.queue_lines(
                            body,
                            stats.queue_full,
                            stats.queue_drops,
                            stats.queue_output,
                        );
                        if let Some(rate) = cfg.shape {
                            let burst = shape_burst(rate);
                            self.line(
                                body,
                                &format!("shape (average) cir {rate}, bc {burst}, be {burst}"),
                            );
                            self.line(body, &format!("target shape rate {rate}"));
                        }
                        match cfg.bandwidth {
                            Some(share @ Share::Percent(percent)) => self.line(
                                body,
                                &format!(
                                    "bandwidth {percent}% ({} kbps)",
                                    qos::share_kbps(share, reference)
                                ),
                            ),
                            Some(Share::Kbps(kbps)) => {
                                self.line(body, &format!("bandwidth {kbps} kbps"))
                            }
                            None => {}
                        }
                    }
                    // The default class has a queue of its own even when
                    // nothing was configured for it.
                    None if cfg.name == CLASS_DEFAULT => {
                        self.queue_lines(
                            body,
                            stats.queue_full,
                            stats.queue_drops,
                            stats.queue_output,
                        );
                    }
                    None => {}
                }
            }
            if let Some(police) = cfg.police {
                self.line(body, "police:");
                self.line(
                    body + 4,
                    &format!(
                        "cir {} bps, bc {} bytes",
                        police.cir,
                        police_burst(police.cir)
                    ),
                );
                self.line(
                    body + 2,
                    &format!(
                        "conformed {} packets, {} bytes; actions:",
                        packets(stats.conformed),
                        whole(stats.conformed)
                    ),
                );
                self.line(body + 4, "transmit");
                self.line(
                    body + 2,
                    &format!(
                        "exceeded {} packets, {} bytes; actions:",
                        packets(stats.exceeded),
                        whole(stats.exceeded)
                    ),
                );
                self.line(body + 4, "drop");
                self.line(
                    body + 2,
                    &format!(
                        "conformed {} bps, exceed {} bps",
                        kilo(stats.conform_rate),
                        kilo(stats.exceed_rate)
                    ),
                );
            }
            if let Some(value) = cfg.set_dscp {
                self.line(body, "QoS Set");
                self.line(body + 2, &format!("dscp {}", dscp::config_text(value)));
                self.line(
                    body + 4,
                    &format!("Packets marked {}", packets(stats.marked)),
                );
            }
            if let Some(child) = &class.child {
                self.out.push('\n');
                self.line(body, &format!("Service-policy : {}", child.policy));
                // A child's percentages are of the nearest shape rate above it.
                let reference = cfg.shape.map_or(reference, |rate| rate as f64);
                self.level(child, indent + 4, reference, prefix);
            }
            prefix.pop();
        }
    }
}

/// `show policy-map interface [NAME] [input | output]`: every attached
/// policy with its counters. A device with nothing attached prints nothing.
/// `None` when the interface named does not exist.
pub(crate) fn policy_map_interface(
    node: &Node,
    only: Option<IfId>,
    direction: Option<bool>,
) -> Option<String> {
    if only.is_some_and(|id| !node.cfg.interfaces.contains_key(&id)) {
        return None;
    }
    let mut out = String::new();
    for (id, iface) in node
        .cfg
        .interfaces
        .iter()
        .filter(|(id, _)| only.is_none_or(|o| o == **id))
    {
        let attached: Vec<(bool, Level)> = [(false, &iface.policy_in), (true, &iface.policy_out)]
            .into_iter()
            .filter(|(output, _)| direction.is_none_or(|d| d == *output))
            .filter_map(|(output, name)| Some((output, qos::plan(&node.cfg, name.as_deref()?)?)))
            .collect();
        if attached.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        let _ = writeln!(out, " {}", id.name());
        let speed = node
            .ifs
            .get(id)
            .map_or(0.0, |state| f64::from(state.speed) * 1e6);
        let reference = iface
            .bandwidth
            .map_or(speed, |kbps| f64::from(kbps) * 1000.0);
        for (output, level) in attached {
            let mut render = Render {
                node,
                stats: node.qos_stats(*id, output, &level.policy),
                output,
                out: String::new(),
            };
            render.out.push('\n');
            render.line(
                2,
                &format!(
                    "Service-policy {}: {}",
                    if output { "output" } else { "input" },
                    level.policy
                ),
            );
            render.level(&level, 4, reference, &mut Vec::new());
            out.push_str(&render.out);
        }
    }
    Some(out)
}
