//! Quality of service: sorting flow traffic into the classes of an attached
//! policy map, and the drop model behind `show policy-map interface`.
//!
//! The model is analytic. Once a tick, every point where traffic can be held
//! back (an interface and a direction) is offered the rates of the flows that
//! cross it and answers with the share of each that gets through:
//!
//! * a policer passes its rate and drops the rest;
//! * a shaper passes its rate and, having nowhere to keep the rest for a whole
//!   tick, drops it from the queue;
//! * when a level is offered more than it can carry (the interface speed, or
//!   the shape rate of the class the policy hangs under), `priority` classes
//!   are served first, classes with a `bandwidth` guarantee keep their share,
//!   and classes without one, `class-default` among them, lose first.
//!
//! Nothing here reads a clock or draws a random number: every counter is a
//! function of the flows, the configuration and simulated time.

use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;

use crate::device::config::{
    CLASS_DEFAULT, ClassMap, ClassMatch, DeviceConfig, POLICY_DEPTH, PolicyClass, Share,
};
use crate::ifname::IfId;
use crate::sim::node::Node;

/// Packets a class queue holds (the platform default; `queue-limit` is not configurable here).
pub(crate) const QUEUE_LIMIT_PACKETS: u64 = 64;
/// Bytes per packet assumed when a flow's octets are shown as packets.
pub(crate) const PACKET_BYTES: f64 = 800.0;

/// What a class map can see of a flow's traffic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Traffic {
    /// `None`: the flow states no marking, and no `match dscp` applies to it.
    pub(crate) dscp: Option<u8>,
    pub(crate) src: Option<Ipv4Addr>,
    pub(crate) dst: Option<Ipv4Addr>,
}

// ------------------------------------------------------------------ counters

/// Counters of one class of an attached policy. Octets are kept as fractions
/// so nothing is lost to rounding between ticks; the command line prints whole
/// packets and bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ClassStats {
    /// Classified into the class, before any action.
    pub(crate) offered: f64,
    /// Policer: sent on / dropped.
    pub(crate) conformed: f64,
    pub(crate) exceeded: f64,
    /// Dropped from the class's queue: a shaper's excess or congestion. A class
    /// with a child policy counts what the child's queues dropped, and
    /// `class-default` what was dropped of classes that have no queue of
    /// their own.
    pub(crate) queue_drops: f64,
    /// A `priority` class held to its rate while the level was congested.
    pub(crate) priority_drops: f64,
    /// What the class sent on.
    pub(crate) output: f64,
    /// What left through the class's queue (see [`Amounts::queue_output`]).
    pub(crate) queue_output: f64,
    /// Re-marked by `set dscp`.
    pub(crate) marked: f64,
    /// Smoothed rates in bits per second, like an interface's 5 minute rates.
    pub(crate) offered_rate: f64,
    pub(crate) drop_rate: f64,
    pub(crate) conform_rate: f64,
    pub(crate) exceed_rate: f64,
    /// The queue overflowed during the last tick.
    pub(crate) queue_full: bool,
}

impl ClassStats {
    fn absorb(&mut self, tick: &ClassTick, alpha: f64) {
        let o = &tick.octets;
        self.offered += o.offered;
        self.conformed += o.conformed;
        self.exceeded += o.exceeded;
        self.queue_drops += o.queue_drops;
        self.priority_drops += o.priority_drops;
        self.output += o.output;
        self.queue_output += o.queue_output;
        self.marked += o.marked;
        let r = &tick.rate;
        self.offered_rate += (r.offered - self.offered_rate) * alpha;
        self.drop_rate += ((r.offered - r.output).max(0.0) - self.drop_rate) * alpha;
        self.conform_rate += (r.conformed - self.conform_rate) * alpha;
        self.exceed_rate += (r.exceeded - self.exceed_rate) * alpha;
        self.queue_full = r.queue_drops + r.priority_drops > 0.0;
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PolicyStats {
    /// The policy the counters belong to: attaching another one starts over.
    pub(crate) policy: String,
    /// By class path: `[class]`, `[class, class of its child policy]`, ...
    pub(crate) classes: BTreeMap<Vec<String>, ClassStats>,
}

// ---------------------------------------------------------------------- plan

/// A policy map resolved against the configuration: its classes in the order
/// they are tried, `class-default` last whether it was written or not.
#[derive(Clone, Debug)]
pub(crate) struct Level {
    pub(crate) policy: String,
    pub(crate) classes: Vec<PlanClass>,
}

#[derive(Clone, Debug)]
pub(crate) struct PlanClass {
    pub(crate) class: PolicyClass,
    pub(crate) child: Option<Level>,
}

impl Level {
    /// Some class of this policy keeps a queue (so the whole level is scheduled).
    pub(crate) fn queues(&self) -> bool {
        self.classes.iter().any(|c| c.class.queues())
    }

    /// Queueing anywhere in the policy, children included.
    pub(crate) fn queues_anywhere(&self) -> bool {
        self.classes
            .iter()
            .any(|c| c.class.queues() || c.child.as_ref().is_some_and(Level::queues_anywhere))
    }

    /// Every class path of the policy, parents before their children.
    pub(crate) fn paths(&self) -> Vec<Vec<String>> {
        fn walk(level: &Level, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
            for class in &level.classes {
                prefix.push(class.class.name.clone());
                out.push(prefix.clone());
                if let Some(child) = &class.child {
                    walk(child, prefix, out);
                }
                prefix.pop();
            }
        }
        let mut out = Vec::new();
        walk(self, &mut Vec::new(), &mut out);
        out
    }
}

/// Resolve a policy map. `None` when it is not defined.
pub(crate) fn plan(cfg: &DeviceConfig, policy: &str) -> Option<Level> {
    plan_at(cfg, policy, POLICY_DEPTH)
}

fn plan_at(cfg: &DeviceConfig, policy: &str, budget: usize) -> Option<Level> {
    let map = cfg.policy_map(policy)?;
    let resolve = |class: &PolicyClass| PlanClass {
        class: class.clone(),
        // A child deeper than the platform nests is not consulted at all.
        child: class
            .child
            .as_deref()
            .filter(|_| budget > 1)
            .and_then(|child| plan_at(cfg, child, budget - 1)),
    };
    let mut classes: Vec<PlanClass> = map
        .classes
        .iter()
        .filter(|class| class.name != CLASS_DEFAULT)
        .map(resolve)
        .collect();
    classes.push(
        map.classes
            .iter()
            .find(|class| class.name == CLASS_DEFAULT)
            .map_or_else(
                || PlanClass {
                    class: PolicyClass::new(CLASS_DEFAULT),
                    child: None,
                },
                resolve,
            ),
    );
    Some(Level {
        policy: map.name.clone(),
        classes,
    })
}

// ------------------------------------------------------------ classification

/// Would this access list permit the traffic? A flow is plain IP with no
/// protocol or ports, so only entries that look at addresses alone can match.
fn acl_permits(cfg: &DeviceConfig, name: &str, traffic: &Traffic) -> bool {
    use crate::device::config::{AclProto, AddrSpec};
    let Some(acl) = cfg.acl(name) else {
        return false;
    };
    let covers = |spec: AddrSpec, ip: Option<Ipv4Addr>| match (spec, ip) {
        (AddrSpec::Any, _) => true,
        (spec, Some(ip)) => spec.matches(ip),
        (_, None) => false,
    };
    acl.rules()
        .find(|rule| {
            if !covers(rule.src, traffic.src) {
                return false;
            }
            if !acl.extended {
                return true;
            }
            rule.proto == AclProto::Ip
                && covers(rule.dst, traffic.dst)
                && rule.sport.is_none()
                && rule.dport.is_none()
        })
        .is_some_and(|rule| rule.permit)
}

fn statement_matches(cfg: &DeviceConfig, statement: &ClassMatch, traffic: &Traffic) -> bool {
    match statement {
        ClassMatch::Dscp { values, .. } => traffic.dscp.is_some_and(|d| values.contains(&d)),
        ClassMatch::AccessGroup { acl, .. } => acl_permits(cfg, acl, traffic),
        ClassMatch::Any => true,
    }
}

fn class_matches(cfg: &DeviceConfig, map: &ClassMap, traffic: &Traffic) -> bool {
    // A class map with nothing to match on matches nothing.
    if map.matches.is_empty() {
        return false;
    }
    let mut results = map
        .matches
        .iter()
        .map(|statement| statement_matches(cfg, statement, traffic));
    if map.any {
        results.any(|hit| hit)
    } else {
        results.all(|hit| hit)
    }
}

/// The class the traffic falls into at each level of the policy, and the
/// marking it leaves with.
pub(crate) fn classify(
    cfg: &DeviceConfig,
    level: &Level,
    traffic: &Traffic,
) -> (Vec<usize>, Option<u8>) {
    let mut path = Vec::new();
    let mut dscp = traffic.dscp;
    let mut current = Some(level);
    while let Some(level) = current {
        let last = level.classes.len().saturating_sub(1);
        let index = level
            .classes
            .iter()
            .position(|c| {
                c.class.name == CLASS_DEFAULT
                    || cfg
                        .class_map(&c.class.name)
                        .is_some_and(|map| class_matches(cfg, map, traffic))
            })
            .unwrap_or(last);
        path.push(index);
        let Some(class) = level.classes.get(index) else {
            break;
        };
        // Every level sorts the packet as it arrived; the innermost `set` wins.
        if let Some(value) = class.class.set_dscp {
            dscp = Some(value);
        }
        current = class.child.as_ref();
    }
    (path, dscp)
}

// ---------------------------------------------------------------- scheduling

/// How a class is treated when its level has less to give than is wanted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Service {
    /// Served first; held to the rate (if it has one) while there is congestion.
    Priority(Option<f64>),
    /// Guaranteed this many bits per second.
    Bandwidth(f64),
    /// No guarantee: `class-default`, and classes that only mark or police.
    BestEffort,
}

/// Hand `capacity` bits per second to classes that each want `demand`.
pub(crate) fn schedule(wants: &[(f64, Service)], capacity: f64) -> Vec<f64> {
    let total: f64 = wants.iter().map(|(demand, _)| demand).sum();
    if total <= capacity {
        return wants.iter().map(|(demand, _)| *demand).collect();
    }
    let mut grant = vec![0.0; wants.len()];
    let mut left = capacity.max(0.0);
    for (i, (demand, service)) in wants.iter().enumerate() {
        if let Service::Priority(limit) = service {
            let served = demand.min(limit.unwrap_or(f64::INFINITY)).min(left);
            grant[i] = served;
            left -= served;
        }
    }
    let queued = |service: &Service| !matches!(service, Service::Priority(_));
    let others: f64 = wants
        .iter()
        .filter(|(_, service)| queued(service))
        .map(|(demand, _)| demand)
        .sum();
    if others <= left {
        for (i, (demand, service)) in wants.iter().enumerate() {
            if queued(service) {
                grant[i] = *demand;
            }
        }
        return grant;
    }
    let weight = |service: &Service| match service {
        Service::Bandwidth(bps) => *bps,
        _ => 0.0,
    };
    // Guarantees shrink together when they promise more than there is.
    let reserved: f64 = wants.iter().map(|(_, service)| weight(service)).sum();
    let scale = if reserved > left {
        left / reserved
    } else {
        1.0
    };
    let unreserved = (left - reserved * scale).max(0.0);
    // Classes without a guarantee live on what nobody reserved, and on what
    // the guaranteed classes leave unused: they are the first to lose.
    let best_effort: f64 = wants
        .iter()
        .filter(|(_, service)| *service == Service::BestEffort)
        .map(|(demand, _)| demand)
        .sum();
    let cut = (others - left).min((best_effort - unreserved).max(0.0));
    if best_effort > 0.0 {
        let keep = 1.0 - cut / best_effort;
        for (i, (demand, service)) in wants.iter().enumerate() {
            if *service == Service::BestEffort {
                grant[i] = demand * keep;
            }
        }
    }
    // What remains is shared among the guaranteed classes by weight; a class
    // that wants less than its share leaves the difference to the others.
    let mut remaining = (left - (best_effort - cut)).max(0.0);
    let mut open: Vec<usize> = (0..wants.len())
        .filter(|i| matches!(wants[*i].1, Service::Bandwidth(_)))
        .collect();
    while !open.is_empty() {
        let weights: f64 = open.iter().map(|i| weight(&wants[*i].1)).sum();
        let share = |i: usize| {
            if weights > 0.0 {
                remaining * weight(&wants[i].1) / weights
            } else {
                remaining / open.len() as f64
            }
        };
        let content: Vec<usize> = open
            .iter()
            .copied()
            .filter(|i| wants[*i].0 <= share(*i))
            .collect();
        if content.is_empty() {
            for i in &open {
                grant[*i] = share(*i);
            }
            break;
        }
        for i in content {
            grant[i] = wants[i].0;
            remaining = (remaining - wants[i].0).max(0.0);
            open.retain(|o| *o != i);
        }
    }
    grant
}

// ---------------------------------------------------------------- evaluation

/// One stream of traffic arriving at a policy.
#[derive(Clone, Debug)]
pub(crate) struct Member {
    /// Bits per second right now.
    pub(crate) rate: f64,
    /// Octets it brings in this tick.
    pub(crate) octets: f64,
    /// The class it falls into at each level (see [`classify`]).
    pub(crate) path: Vec<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Amounts {
    pub(crate) offered: f64,
    pub(crate) conformed: f64,
    pub(crate) exceeded: f64,
    pub(crate) queue_drops: f64,
    pub(crate) priority_drops: f64,
    pub(crate) output: f64,
    /// What left through the class's queue: for `class-default` that
    /// includes the classes that have no queue of their own.
    pub(crate) queue_output: f64,
    pub(crate) marked: f64,
}

/// What one class did during a tick, as rates and as octets.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ClassTick {
    pub(crate) rate: Amounts,
    pub(crate) octets: Amounts,
}

impl ClassTick {
    fn set(&mut self, field: fn(&mut Amounts) -> &mut f64, value: Pair) {
        *field(&mut self.rate) = value.rate;
        *field(&mut self.octets) = value.octets;
    }
}

/// What a policy did with the traffic offered to it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Outcome {
    /// Share of each member that got through, in the order given.
    pub(crate) pass: Vec<f64>,
    pub(crate) classes: BTreeMap<Vec<String>, ClassTick>,
    /// Octets dropped from queues, for the interface's own drop counter.
    pub(crate) queue_drop_octets: f64,
}

/// A rate and the octets it stands for in this tick, scaled together.
#[derive(Clone, Copy, Debug, Default)]
struct Pair {
    rate: f64,
    octets: f64,
}

impl Pair {
    fn scaled(self, factor: f64) -> Pair {
        Pair {
            rate: self.rate * factor,
            octets: self.octets * factor,
        }
    }

    fn plus(self, other: Pair) -> Pair {
        Pair {
            rate: self.rate + other.rate,
            octets: self.octets + other.octets,
        }
    }
}

struct Stream {
    index: usize,
    load: Pair,
}

/// What a level did with what it was offered.
#[derive(Clone, Copy, Default)]
struct LevelTotals {
    output: Pair,
    queue_drops: Pair,
}

/// Share of an offered rate a policer lets through.
fn police_pass(class: &PolicyClass, offered: f64) -> f64 {
    match class.police {
        Some(police) if offered > police.cir as f64 => police.cir as f64 / offered,
        _ => 1.0,
    }
}

/// What a level would send if nothing above it held it back: its classes'
/// traffic after their policers, each capped by its shaper.
fn demand(
    level: &Level,
    streams: &[Stream],
    members: &[Member],
    depth: usize,
    shaping: bool,
) -> f64 {
    level
        .classes
        .iter()
        .enumerate()
        .map(|(k, class)| {
            let mine: Vec<Stream> = streams
                .iter()
                .filter(|s| members[s.index].path.get(depth) == Some(&k))
                .map(|s| Stream {
                    index: s.index,
                    load: s.load,
                })
                .collect();
            let offered: f64 = mine.iter().map(|s| s.load.rate).sum();
            let pass = police_pass(&class.class, offered);
            let wanted = match &class.child {
                Some(child) => {
                    let passed: Vec<Stream> = mine
                        .iter()
                        .map(|s| Stream {
                            index: s.index,
                            load: s.load.scaled(pass),
                        })
                        .collect();
                    demand(child, &passed, members, depth + 1, shaping)
                }
                None => offered * pass,
            };
            match class.class.shape {
                Some(rate) if shaping => wanted.min(rate as f64),
                _ => wanted,
            }
        })
        .sum()
}

struct Serve<'a> {
    members: &'a [Member],
    /// Output direction: queues exist. On input only policing and marking apply.
    output: bool,
    pass: Vec<f64>,
    classes: BTreeMap<Vec<String>, ClassTick>,
}

impl Serve<'_> {
    /// Run one level. `capacity` is what it may send in total (`None`: no
    /// limit); `reference` is what a percentage is a percentage of.
    fn level(
        &mut self,
        level: &Level,
        streams: &[Stream],
        depth: usize,
        capacity: Option<f64>,
        reference: f64,
        prefix: &mut Vec<String>,
    ) -> LevelTotals {
        // What each class is offered, lets through its policer, and then wants to send.
        let mut per_class: Vec<(Vec<Stream>, Pair, f64, f64)> = Vec::new();
        for (k, class) in level.classes.iter().enumerate() {
            let mine: Vec<Stream> = streams
                .iter()
                .filter(|s| self.members[s.index].path.get(depth) == Some(&k))
                .map(|s| Stream {
                    index: s.index,
                    load: s.load,
                })
                .collect();
            let offered = mine.iter().fold(Pair::default(), |sum, s| sum.plus(s.load));
            let policed = police_pass(&class.class, offered.rate);
            let passed: Vec<Stream> = mine
                .iter()
                .map(|s| Stream {
                    index: s.index,
                    load: s.load.scaled(policed),
                })
                .collect();
            let mut wanted = match &class.child {
                Some(child) => demand(child, &passed, self.members, depth + 1, self.output),
                None => offered.rate * policed,
            };
            if let (Some(rate), true) = (class.class.shape, self.output) {
                wanted = wanted.min(rate as f64);
            }
            per_class.push((passed, offered, policed, wanted));
        }
        let grants: Vec<f64> = match capacity.filter(|_| self.output) {
            Some(capacity) => {
                let wants: Vec<(f64, Service)> = level
                    .classes
                    .iter()
                    .zip(&per_class)
                    .map(|(class, (_, _, _, wanted))| {
                        let service = match (class.class.priority, class.class.bandwidth) {
                            (Some(limit), _) => {
                                Service::Priority(limit.map(|share| share.bps(reference)))
                            }
                            (None, Some(share)) => Service::Bandwidth(share.bps(reference)),
                            (None, None) => Service::BestEffort,
                        };
                        (*wanted, service)
                    })
                    .collect();
                schedule(&wants, capacity)
            }
            None => per_class.iter().map(|(_, _, _, wanted)| *wanted).collect(),
        };

        let queueing = self.output && level.queues();
        let mut totals = LevelTotals::default();
        // What classes without a queue of their own put through the default queue.
        let mut lodgers = LevelTotals::default();
        for ((class, (passed, offered, policed, _)), grant) in
            level.classes.iter().zip(per_class).zip(grants)
        {
            prefix.push(class.class.name.clone());
            let conformed = offered.scaled(policed);
            let mut tick = ClassTick::default();
            tick.set(|a| &mut a.offered, offered);
            if class.class.police.is_some() {
                tick.set(|a| &mut a.conformed, conformed);
                tick.set(|a| &mut a.exceeded, offered.scaled(1.0 - policed));
            }
            if class.class.set_dscp.is_some() {
                tick.set(|a| &mut a.marked, offered);
            }
            let (sent, dropped) = match &class.child {
                Some(child) => {
                    // The child shares out what this class was granted. Its
                    // percentages are of the nearest shape rate above it.
                    let limited =
                        self.output && (capacity.is_some() || class.class.shape.is_some());
                    let below = self.level(
                        child,
                        &passed,
                        depth + 1,
                        limited.then_some(grant),
                        class.class.shape.map_or(reference, |rate| rate as f64),
                        prefix,
                    );
                    for stream in &passed {
                        self.pass[stream.index] *= policed;
                    }
                    (below.output, below.queue_drops)
                }
                None => {
                    let queued = if conformed.rate > 0.0 {
                        (grant / conformed.rate).min(1.0)
                    } else {
                        1.0
                    };
                    for stream in &passed {
                        self.pass[stream.index] *= policed * queued;
                    }
                    (conformed.scaled(queued), conformed.scaled(1.0 - queued))
                }
            };
            let cfg = &class.class;
            let leaf = class.child.is_none();
            if !queueing && leaf {
                // No queues at this level: what does not fit is lost behind
                // the policy, in the queue of the class above or of the
                // interface, and that is where it is counted.
                tick.set(|a| &mut a.output, conformed);
            } else if cfg.priority.is_some() {
                // With or without a policy of its own inside: what a priority
                // class loses is what its rate turned away, and the line it
                // prints has one counter for that.
                tick.set(|a| &mut a.output, sent);
                tick.set(|a| &mut a.queue_output, sent);
                tick.set(|a| &mut a.priority_drops, dropped);
            } else if cfg.name == CLASS_DEFAULT && queueing {
                // The default class is last, so by now it is known what the
                // classes without a queue of their own put through its queue.
                tick.set(|a| &mut a.output, sent);
                tick.set(|a| &mut a.queue_output, sent.plus(lodgers.output));
                tick.set(|a| &mut a.queue_drops, dropped.plus(lodgers.queue_drops));
            } else if cfg.queues() || !leaf {
                tick.set(|a| &mut a.output, sent);
                tick.set(|a| &mut a.queue_output, sent);
                tick.set(|a| &mut a.queue_drops, dropped);
            } else {
                // A class that only marks or polices has no queue of its own:
                // its packets wait in the default class's queue, and that
                // queue's counters are where their drops show.
                tick.set(|a| &mut a.output, sent);
                lodgers.output = lodgers.output.plus(sent);
                lodgers.queue_drops = lodgers.queue_drops.plus(dropped);
            }
            totals.output = totals.output.plus(sent);
            totals.queue_drops = totals.queue_drops.plus(dropped);
            self.classes.insert(prefix.clone(), tick);
            prefix.pop();
        }
        totals
    }
}

/// Run a policy over the traffic offered to it. `capacity` is what the
/// interface can carry in the output direction, in bits per second.
pub(crate) fn evaluate(
    level: &Level,
    members: &[Member],
    output: bool,
    capacity: Option<f64>,
    reference: f64,
) -> Outcome {
    let streams: Vec<Stream> = members
        .iter()
        .enumerate()
        .map(|(index, member)| Stream {
            index,
            load: Pair {
                rate: member.rate,
                octets: member.octets,
            },
        })
        .collect();
    let mut serve = Serve {
        members,
        output,
        pass: vec![1.0; members.len()],
        classes: BTreeMap::new(),
    };
    let totals = serve.level(level, &streams, 0, capacity, reference, &mut Vec::new());
    Outcome {
        pass: serve.pass,
        classes: serve.classes,
        queue_drop_octets: totals.queue_drops.octets,
    }
}

/// An interface with no policy: one queue, first in first out. What does not
/// fit is dropped from every stream alike.
pub(crate) fn fifo(members: &[Member], capacity: f64) -> Outcome {
    let offered: f64 = members.iter().map(|m| m.rate).sum();
    let pass = if offered > capacity && offered > 0.0 {
        capacity.max(0.0) / offered
    } else {
        1.0
    };
    Outcome {
        pass: vec![pass; members.len()],
        classes: BTreeMap::new(),
        queue_drop_octets: members.iter().map(|m| m.octets).sum::<f64>() * (1.0 - pass),
    }
}

// ---------------------------------------------------------------- accounting

impl Node {
    /// Fold one tick into the policy counters. `ticks` holds what each
    /// attached policy did, by interface and direction (`true` = output);
    /// a policy that saw no traffic only has its averages decay.
    pub(crate) fn qos_account(
        &mut self,
        ticks: &BTreeMap<(IfId, bool), BTreeMap<Vec<String>, ClassTick>>,
        alpha: f64,
    ) {
        if self.cfg.policy_maps.is_empty() && self.qos.is_empty() {
            return;
        }
        let attached: Vec<((IfId, bool), Level)> = self
            .cfg
            .interfaces
            .iter()
            .flat_map(|(id, iface)| {
                [(false, &iface.policy_in), (true, &iface.policy_out)]
                    .into_iter()
                    .filter_map(|(output, name)| {
                        let level = plan(&self.cfg, name.as_deref()?)?;
                        Some(((*id, output), level))
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        self.qos.retain(|key, stats| {
            attached
                .iter()
                .any(|(k, level)| k == key && level.policy == stats.policy)
        });
        let idle = ClassTick::default();
        for (key, level) in attached {
            let stats = self.qos.entry(key).or_insert_with(|| PolicyStats {
                policy: level.policy.clone(),
                classes: BTreeMap::new(),
            });
            let paths = level.paths();
            let known: BTreeSet<&Vec<String>> = paths.iter().collect();
            stats.classes.retain(|path, _| known.contains(path));
            let ticked = ticks.get(&key);
            for path in paths {
                let tick = ticked.and_then(|t| t.get(&path)).unwrap_or(&idle);
                stats.classes.entry(path).or_default().absorb(tick, alpha);
            }
        }
    }

    /// The counters of an attached policy, if they are for the policy named.
    pub(crate) fn qos_stats(
        &self,
        iface: IfId,
        output: bool,
        policy: &str,
    ) -> Option<&PolicyStats> {
        self.qos
            .get(&(iface, output))
            .filter(|stats| stats.policy == policy)
    }

    /// Some class still shows traffic: a full queue, or an average that has
    /// not decayed to nothing yet.
    pub(crate) fn qos_lingers(&self) -> bool {
        self.qos.values().any(|stats| {
            stats.classes.values().any(|class| {
                class.queue_full
                    || class.offered_rate >= 1.0
                    || class.drop_rate >= 1.0
                    || class.conform_rate >= 1.0
                    || class.exceed_rate >= 1.0
            })
        })
    }

    /// `clear counters`: start the policy counters of one or all interfaces over.
    pub(crate) fn qos_clear(&mut self, only: Option<IfId>) {
        self.qos
            .retain(|(iface, _), _| only.is_some_and(|o| o != *iface));
    }
}

/// What a share of the reference rate comes to, for display.
pub(crate) fn share_kbps(share: Share, reference: f64) -> u64 {
    (share.bps(reference) / 1000.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::config::{Police, PolicyMap};

    fn class(name: &str) -> PolicyClass {
        PolicyClass::new(name)
    }

    fn level(classes: Vec<PolicyClass>) -> Level {
        Level {
            policy: "P".to_string(),
            classes: classes
                .into_iter()
                .map(|class| PlanClass { class, child: None })
                .collect(),
        }
    }

    fn member(rate: f64, class: usize) -> Member {
        Member {
            rate,
            octets: rate / 8.0,
            path: vec![class],
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6 * b.abs().max(1.0)
    }

    #[test]
    fn nothing_is_dropped_while_everything_fits() {
        let grants = schedule(
            &[
                (300.0, Service::Priority(Some(100.0))),
                (400.0, Service::Bandwidth(100.0)),
                (200.0, Service::BestEffort),
            ],
            1000.0,
        );
        // A priority class may exceed its rate when nobody else needs the room.
        assert_eq!(grants, [300.0, 400.0, 200.0]);
    }

    #[test]
    fn a_priority_class_is_served_first_and_held_to_its_rate_under_congestion() {
        let grants = schedule(
            &[
                (300.0, Service::Priority(Some(200.0))),
                (900.0, Service::BestEffort),
            ],
            1000.0,
        );
        assert_eq!(grants, [200.0, 800.0]);
        // Without a rate it takes all it wants, and the rest starve.
        let grants = schedule(
            &[
                (1500.0, Service::Priority(None)),
                (900.0, Service::BestEffort),
            ],
            1000.0,
        );
        assert_eq!(grants, [1000.0, 0.0]);
    }

    #[test]
    fn class_default_loses_first_and_guaranteed_classes_keep_their_share() {
        // 800 to share: DATA is guaranteed 400, the rest is unreserved.
        let both_over = schedule(
            &[
                (200.0, Service::Priority(Some(200.0))),
                (900.0, Service::Bandwidth(400.0)),
                (600.0, Service::BestEffort),
            ],
            1000.0,
        );
        assert!(
            close(both_over[1], 400.0) && close(both_over[2], 400.0),
            "{both_over:?}"
        );
        // A default class inside the unreserved room is left alone; the
        // guaranteed class takes what it leaves.
        let data_over = schedule(
            &[
                (200.0, Service::Priority(Some(200.0))),
                (900.0, Service::Bandwidth(400.0)),
                (300.0, Service::BestEffort),
            ],
            1000.0,
        );
        assert!(
            close(data_over[1], 500.0) && close(data_over[2], 300.0),
            "{data_over:?}"
        );
        // A default class may borrow what a guaranteed class does not use.
        let default_over = schedule(
            &[
                (100.0, Service::Bandwidth(400.0)),
                (900.0, Service::BestEffort),
            ],
            800.0,
        );
        assert!(
            close(default_over[0], 100.0) && close(default_over[1], 700.0),
            "{default_over:?}"
        );
    }

    #[test]
    fn bandwidth_classes_share_in_proportion_to_their_guarantees() {
        let grants = schedule(
            &[
                (900.0, Service::Bandwidth(600.0)),
                (900.0, Service::Bandwidth(200.0)),
            ],
            400.0,
        );
        assert!(
            close(grants[0], 300.0) && close(grants[1], 100.0),
            "{grants:?}"
        );
        // One that wants less than its share leaves the difference to the other.
        let grants = schedule(
            &[
                (50.0, Service::Bandwidth(600.0)),
                (900.0, Service::Bandwidth(200.0)),
            ],
            400.0,
        );
        assert!(
            close(grants[0], 50.0) && close(grants[1], 350.0),
            "{grants:?}"
        );
        let total: f64 = grants.iter().sum();
        assert!(close(total, 400.0));
    }

    #[test]
    fn a_policer_passes_its_rate_and_counts_the_rest_as_exceeded() {
        let mut limited = class("GUEST");
        limited.police = Some(Police {
            cir: 2_000_000,
            cir_keyword: false,
            actions: false,
        });
        let level = level(vec![limited, class(CLASS_DEFAULT)]);
        let outcome = evaluate(
            &level,
            &[member(5_000_000.0, 0), member(1_000_000.0, 1)],
            false,
            None,
            1e9,
        );
        assert!(close(outcome.pass[0], 0.4), "{:?}", outcome.pass);
        assert_eq!(outcome.pass[1], 1.0);
        let guest = outcome.classes[&vec!["GUEST".to_string()]];
        assert!(
            close(guest.rate.conformed, 2_000_000.0) && close(guest.rate.exceeded, 3_000_000.0)
        );
        assert!(close(guest.octets.exceeded, 375_000.0), "{guest:?}");
        assert_eq!(
            guest.rate.queue_drops, 0.0,
            "a policer drop is not a queue drop"
        );
        assert_eq!(outcome.queue_drop_octets, 0.0);
    }

    #[test]
    fn a_shaped_parent_hands_its_rate_to_the_child_which_drops_class_default_first() {
        let mut voice = class("VOICE");
        voice.priority = Some(Some(Share::Kbps(1000)));
        let mut video = class("VIDEO");
        video.bandwidth = Some(Share::Percent(40));
        let child = level(vec![voice, video, class(CLASS_DEFAULT)]);
        let mut parent = class(CLASS_DEFAULT);
        parent.shape = Some(5_000_000);
        let top = Level {
            policy: "PARENT".to_string(),
            classes: vec![PlanClass {
                class: parent,
                child: Some(child),
            }],
        };
        let members = [
            Member {
                rate: 800_000.0,
                octets: 100_000.0,
                path: vec![0, 0],
            },
            Member {
                rate: 1_500_000.0,
                octets: 187_500.0,
                path: vec![0, 1],
            },
            Member {
                rate: 4_000_000.0,
                octets: 500_000.0,
                path: vec![0, 2],
            },
        ];
        let outcome = evaluate(&top, &members, true, Some(1e9), 1e9);
        // Voice and video fit inside their guarantees (video: 40 % of the
        // 5 Mbit/s shape rate = 2 Mbit/s); class-default gets what is left.
        assert_eq!(outcome.pass[0], 1.0);
        assert_eq!(outcome.pass[1], 1.0);
        assert!(
            close(outcome.pass[2], 2_700_000.0 / 4_000_000.0),
            "{:?}",
            outcome.pass
        );
        let path = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let default = outcome.classes[&path(&[CLASS_DEFAULT, CLASS_DEFAULT])];
        assert!(close(default.rate.queue_drops, 1_300_000.0), "{default:?}");
        let shaped = outcome.classes[&path(&[CLASS_DEFAULT])];
        assert!(close(shaped.rate.output, 5_000_000.0), "{shaped:?}");
        assert!(
            close(shaped.rate.queue_drops, 1_300_000.0),
            "the parent counts its child's drops"
        );
        assert!(close(outcome.queue_drop_octets, 162_500.0));
    }

    #[test]
    fn a_class_without_a_queue_is_counted_in_the_default_queue_whatever_that_queue_is() {
        let mut capped = class("CAPPED");
        capped.police = Some(Police {
            cir: 3_000_000,
            cir_keyword: false,
            actions: false,
        });
        let mut gold = class("GOLD");
        gold.bandwidth = Some(Share::Kbps(2000));
        // Once with a plain default class, once with one that has a guarantee
        // of its own: either way it is the default queue the capped class
        // waits in.
        for guaranteed in [None, Some(Share::Kbps(500))] {
            let mut default = class(CLASS_DEFAULT);
            default.bandwidth = guaranteed;
            let level = level(vec![gold.clone(), capped.clone(), default]);
            let outcome = evaluate(
                &level,
                &[
                    member(1_000_000.0, 0),
                    member(5_000_000.0, 1),
                    member(2_000_000.0, 2),
                ],
                true,
                Some(4_000_000.0),
                4_000_000.0,
            );
            let of = |name: &str| outcome.classes[&vec![name.to_string()]];
            let (capped, default) = (of("CAPPED"), of(CLASS_DEFAULT));
            assert_eq!(capped.rate.queue_drops, 0.0, "{guaranteed:?}");
            assert_eq!(capped.rate.queue_output, 0.0, "{guaranteed:?}");
            // Everything the level dropped from queues is in the default
            // queue's count, and what both sent left through it.
            let lost = 6_000_000.0 - 4_000_000.0;
            assert!(
                close(default.rate.queue_drops, lost),
                "{guaranteed:?} {default:?}"
            );
            assert!(
                close(
                    default.rate.queue_output,
                    default.rate.output + capped.rate.output
                ),
                "{guaranteed:?} {default:?}"
            );
            assert!(
                close(outcome.queue_drop_octets, lost / 8.0),
                "{guaranteed:?}"
            );
        }
    }

    #[test]
    fn an_input_policy_never_queues() {
        let mut shaped = class(CLASS_DEFAULT);
        shaped.shape = Some(1_000_000);
        let outcome = evaluate(
            &level(vec![shaped]),
            &[member(5_000_000.0, 0)],
            false,
            None,
            1e9,
        );
        assert_eq!(outcome.pass, [1.0]);
    }

    #[test]
    fn one_queue_without_a_policy_drops_from_everyone_alike() {
        let outcome = fifo(&[member(60.0, 0), member(140.0, 0)], 100.0);
        assert_eq!(outcome.pass, [0.5, 0.5]);
        assert!(close(outcome.queue_drop_octets, 12.5));
        assert_eq!(fifo(&[member(60.0, 0)], 100.0).pass, [1.0]);
    }

    /// The same sequence every run: the model itself draws nothing.
    struct Dice(u64);

    impl Dice {
        fn roll(&mut self, sides: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % sides
        }

        fn rate(&mut self) -> u64 {
            [8_000, 64_000, 500_000, 2_000_000, 7_000_000][self.roll(5) as usize]
        }

        /// A policy of up to four classes and the default, `depth` levels deep.
        fn policy(&mut self, depth: usize) -> Level {
            let mut classes = Vec::new();
            let named = self.roll(4);
            for k in 0..=named {
                let last = k == named;
                let mut class = class(&if last {
                    CLASS_DEFAULT.to_string()
                } else {
                    format!("C{k}")
                });
                match self.roll(6) {
                    0 if !last => {
                        class.priority = Some(match self.roll(3) {
                            0 => None,
                            1 => Some(Share::Kbps(self.rate() as u32 / 1000)),
                            _ => Some(Share::Percent(1 + self.roll(60) as u8)),
                        });
                    }
                    1 => class.bandwidth = Some(Share::Percent(1 + self.roll(60) as u8)),
                    2 => class.bandwidth = Some(Share::Kbps(self.rate() as u32 / 1000)),
                    3 => class.shape = Some(self.rate()),
                    _ => {}
                }
                if self.roll(3) == 0 {
                    class.police = Some(Police {
                        cir: self.rate(),
                        cir_keyword: false,
                        actions: false,
                    });
                }
                let child = (depth > 1 && self.roll(3) == 0).then(|| self.policy(depth - 1));
                classes.push(PlanClass { class, child });
            }
            Level {
                policy: format!("P{depth}"),
                classes,
            }
        }

        /// A stream that falls into some class at every level.
        fn member(&mut self, level: &Level) -> Member {
            let mut path = Vec::new();
            let mut at = Some(level);
            while let Some(level) = at {
                let index = self.roll(level.classes.len() as u64) as usize;
                path.push(index);
                at = level.classes[index].child.as_ref();
            }
            let rate = self.rate() as f64 * (1 + self.roll(4)) as f64 / 2.0;
            Member {
                rate,
                // Ticks are not all a second long.
                octets: rate / 8.0 * (1 + self.roll(3)) as f64 / 2.0,
                path,
            }
        }
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
    }

    /// Check one level of a policy against its counters, and the levels
    /// under it. Returns what the level's policers dropped, children included.
    fn audit(
        level: &Level,
        outcome: &Outcome,
        output: bool,
        prefix: &mut Vec<String>,
        context: &str,
    ) -> f64 {
        let queueing = output && level.queues();
        let (mut offered, mut accounted, mut policed) = (0.0, 0.0, 0.0);
        for class in &level.classes {
            prefix.push(class.class.name.clone());
            let tick = outcome.classes[&*prefix];
            let o = tick.octets;
            for amounts in [tick.rate, tick.octets] {
                for value in [
                    amounts.offered,
                    amounts.conformed,
                    amounts.exceeded,
                    amounts.queue_drops,
                    amounts.priority_drops,
                    amounts.output,
                    amounts.queue_output,
                    amounts.marked,
                ] {
                    assert!(value.is_finite() && value >= -1e-9, "{context}");
                }
            }
            assert!(o.output <= o.offered * (1.0 + 1e-9) + 1e-9, "{context}");
            if let (Some(rate), true) = (class.class.shape, output) {
                assert!(
                    tick.rate.output <= rate as f64 * (1.0 + 1e-9),
                    "a shaper sends its rate at most\n{context}"
                );
            }
            match class.class.police {
                Some(police) => {
                    assert!(near(o.offered, o.conformed + o.exceeded), "{context}");
                    assert!(
                        tick.rate.conformed <= police.cir as f64 * (1.0 + 1e-9),
                        "a policer passes its rate at most\n{context}"
                    );
                }
                None => assert_eq!((o.conformed, o.exceeded), (0.0, 0.0), "{context}"),
            }
            if !output {
                assert_eq!(
                    (o.queue_drops, o.priority_drops),
                    (0.0, 0.0),
                    "arriving traffic is never queued\n{context}"
                );
            }
            let below = match &class.child {
                // A class with a policy inside: its classes are offered what
                // its own policer let through, and what they sent and lost
                // is what it sent and lost.
                Some(child) => {
                    let inner: f64 = child
                        .classes
                        .iter()
                        .map(|c| {
                            prefix.push(c.class.name.clone());
                            let offered = outcome.classes[&*prefix].octets.offered;
                            prefix.pop();
                            offered
                        })
                        .sum();
                    assert!(near(inner, o.offered - o.exceeded), "{context}");
                    let below = audit(child, outcome, output, prefix, context);
                    // (The default class of a policy that queues also carries
                    // the queue losses of classes without a queue: the sum
                    // over the level, below, is the check for it.)
                    if !(queueing && class.class.name == CLASS_DEFAULT) {
                        assert!(
                            near(
                                o.offered,
                                o.exceeded + below + o.output + o.queue_drops + o.priority_drops
                            ),
                            "classified != sent + dropped in {prefix:?}\n{context}"
                        );
                    }
                    below
                }
                None => {
                    if !queueing {
                        // No queue at this level: the class sends on all its
                        // policer let through (what does not fit after that
                        // is the queue above's to count, or the interface's).
                        assert!(near(o.offered, o.exceeded + o.output), "{context}");
                        assert_eq!((o.queue_drops, o.priority_drops), (0.0, 0.0), "{context}");
                    } else if !class.class.queues() && class.class.name != CLASS_DEFAULT {
                        // A class without a queue of its own in a policy that
                        // queues: its losses are on the default queue.
                        assert_eq!(
                            (o.queue_drops, o.priority_drops, o.queue_output),
                            (0.0, 0.0, 0.0),
                            "{context}"
                        );
                    } else if class.class.name != CLASS_DEFAULT {
                        assert!(
                            near(
                                o.offered,
                                o.exceeded + o.output + o.queue_drops + o.priority_drops
                            ),
                            "classified != sent + dropped in {prefix:?}\n{context}"
                        );
                    }
                    0.0
                }
            };
            offered += o.offered;
            accounted += o.exceeded + below + o.output + o.queue_drops + o.priority_drops;
            policed += o.exceeded + below;
            prefix.pop();
        }
        // Over the whole level the sums meet even where the default queue
        // carries the losses of other classes.
        if queueing {
            assert!(
                near(offered, accounted),
                "{offered} classified, {accounted} sent or dropped at {prefix:?}\n{context}"
            );
        }
        policed
    }

    #[test]
    fn every_octet_offered_is_sent_on_or_counted_as_exactly_one_kind_of_drop() {
        let mut dice = Dice(0x00C0_FFEE);
        let mut congested = 0;
        for round in 0..4000 {
            let level = dice.policy(3);
            let members: Vec<Member> = (0..1 + dice.roll(6)).map(|_| dice.member(&level)).collect();
            let output = dice.roll(4) != 0;
            let capacity = (output && dice.roll(5) != 0).then(|| dice.rate() as f64 * 2.0);
            let outcome = evaluate(&level, &members, output, capacity, capacity.unwrap_or(1e7));
            let context = format!("round {round}: {level:#?}\n{members:#?}\n{outcome:#?}");

            // A stream is thinned, never grown.
            for pass in &outcome.pass {
                assert!((0.0..=1.0 + 1e-12).contains(pass), "{context}");
            }
            let offered: f64 = members.iter().map(|m| m.octets).sum();
            let passed: f64 = members
                .iter()
                .zip(&outcome.pass)
                .map(|(m, pass)| m.octets * pass)
                .sum();
            let top: f64 = level
                .classes
                .iter()
                .map(|c| outcome.classes[&vec![c.class.name.clone()]].octets.offered)
                .sum();
            assert!(near(top, offered), "everything is classified\n{context}");

            // What did not get through is in the counters exactly once: a
            // policer counted it, or a queue did.
            let policed = audit(&level, &outcome, output, &mut Vec::new(), &context);
            assert!(
                near(offered - passed, policed + outcome.queue_drop_octets),
                "{} lost, {policed} policed, {} dropped from queues\n{context}",
                offered - passed,
                outcome.queue_drop_octets
            );
            if outcome.queue_drop_octets > 1.0 {
                congested += 1;
            }

            // The policy sends no more than the interface carries.
            let sent: f64 = members
                .iter()
                .zip(&outcome.pass)
                .map(|(m, pass)| m.rate * pass)
                .sum();
            if let Some(capacity) = capacity {
                assert!(
                    sent <= capacity * (1.0 + 1e-9),
                    "{sent} > {capacity}\n{context}"
                );
            }
        }
        assert!(
            congested > 500,
            "the generator has to congest something: {congested}"
        );
    }

    #[test]
    fn class_default_is_always_there_and_always_last() {
        let mut cfg = DeviceConfig::factory(crate::catalog::model("router").unwrap(), "R1");
        cfg.class_maps.push(ClassMap {
            name: "VOICE".to_string(),
            any: true,
            description: String::new(),
            matches: vec![ClassMatch::Dscp {
                values: vec![46],
                ip: false,
            }],
        });
        cfg.policy_maps.push(PolicyMap {
            name: "WAN".to_string(),
            description: String::new(),
            classes: vec![class("VOICE")],
        });
        let level = plan(&cfg, "WAN").unwrap();
        let names: Vec<&str> = level
            .classes
            .iter()
            .map(|c| c.class.name.as_str())
            .collect();
        assert_eq!(names, ["VOICE", CLASS_DEFAULT]);
        let marked = Traffic {
            dscp: Some(46),
            src: None,
            dst: None,
        };
        assert_eq!(classify(&cfg, &level, &marked).0, [0]);
        // A flow that states no marking matches no `match dscp`.
        let unmarked = Traffic {
            dscp: None,
            ..marked
        };
        assert_eq!(classify(&cfg, &level, &unmarked).0, [1]);
        assert!(plan(&cfg, "MISSING").is_none());
    }
}
