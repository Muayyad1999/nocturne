//! Quality of service: what the stored configuration says about its
//! policies, and what the class counters say about congestion.
//!
//! Contract: `docs/dev/spec-qos.md` ("Insights").
//!
//! The two configuration checks read **IOS-style MQC and nothing else**:
//! `policy-map NAME`, `service-policy input|output NAME` under an interface,
//! and `service-policy NAME` inside a class. A configuration in another
//! grammar — typed QoS policies (`policy-map type qos`, the way NX-OS and EOS
//! write them), IOS XR's — and one that is not whole — a section capture, or
//! a listing that stops before its closing `end` — are **not evaluated**,
//! each with its own reason, never reported clean.
//!
//! Which grammar a listing is in is read from the listing, not from the
//! device's vendor or profile: a device type is data, and anybody's profile
//! can say `syntax: ios`.
//!
//! They are deliberately lopsided. To call a policy *missing*, the line that
//! names it has to sit somewhere whose grammar this module knows. To call a
//! policy *unattached*, nothing anywhere in the configuration may name it
//! after `service-policy`, in any form at all. Both lean towards saying
//! nothing.

use std::collections::{BTreeMap, HashSet};

use nocturne_ai::Redactor;
use nocturne_drivers::hier::{Node, Syntax};

use crate::checks::skip_rest;
use crate::metrics::{self, Series};
use crate::model::{Finding, Severity, Suggestion};
use crate::norm;
use crate::report::CheckReport;
use crate::settings::Settings;
use crate::world::{Config, DeviceData, World};

const NO_CONFIG: &str = "no stored running configuration";
const NOT_IOS: &str = "the stored configuration is in a grammar these checks do not read (they \
     understand IOS-style `policy-map` and `service-policy` only)";
const PARTIAL: &str = "the stored configuration is only part of one (no `hostname` line: an \
     interface-only or section capture), so a policy defined or attached in the rest cannot be \
     ruled out";
const CUT_OFF: &str = "the stored configuration does not close with `end`, as a whole IOS \
     configuration does: it stops before its last line, or it is in a grammar these checks do \
     not read (NX-OS prints no `end`). What was not printed cannot be ruled out";
const TYPED: &str = "the configuration uses typed QoS policies (`policy-map type qos`, `type \
     queuing`, `type network-qos`: NX-OS and EOS style), which these checks do not read";
const XR_STYLE: &str = "the configuration is written the IOS XR way (sections closed with \
     `end-policy-map`, `end-policy`, `end-set`), a grammar these checks do not read";
const UNREAD: &str =
    "an interface carries a `service-policy` line in a form these checks do not read";
const UNREAD_MAP: &str = "the configuration defines policy maps in a form these checks do not \
     read (`policy-map NAME` is the only one that is IOS-style)";
const TOO_DEEP: &str =
    "the stored configuration nests deeper than any configuration these checks read";
const SESSIONS: &str = "the device authorises sessions from RADIUS (a virtual template, a dynamic \
     template, `ip subscriber`, `aaa authorization network`), which can apply a policy to a \
     session by name with no line in the configuration";

/// `policy-map type <these>` is QoS in a grammar this module does not read.
/// Every other type (`inspect`, `control`, `access-control`,
/// `performance-monitor` …) is a different kind of policy in a namespace of
/// its own, and is left alone.
const QOS_TYPES: [&str; 6] = [
    "qos",
    "queuing",
    "queueing",
    "lan-queuing",
    "network-qos",
    "quality-of-service",
];

/// How many referencing lines a finding quotes before it starts counting.
const QUOTED_LINES: usize = 8;

/// MQC nests five or six levels at its deepest (interface, service instance,
/// class, action, child). A stored "configuration" that nests far beyond
/// that is not one, and walking it level by level is how hostile or garbled
/// device output would exhaust the stack.
const MAX_DEPTH: usize = 32;

/// A line that names a policy, somewhere whose grammar is known.
struct Reference<'a> {
    policy: &'a str,
    /// The sections the line sits in, outermost first.
    place: String,
    line: &'a str,
}

/// What a configuration says about its MQC policies.
#[derive(Default)]
struct Mqc<'a> {
    /// `policy-map NAME` sections, in the order the configuration has them.
    defined: Vec<(&'a str, &'a Node)>,
    /// Names defined as `policy-map type … NAME`. Never judged; they only
    /// keep `qos_policy_missing` quiet where a platform defines a policy with
    /// a type and attaches it without one.
    typed: HashSet<&'a str>,
    /// References `qos_policy_missing` may judge.
    references: Vec<Reference<'a>>,
    /// Every word that follows `service-policy` on any line of the
    /// configuration, whatever the line is. A policy named here is in use as
    /// far as `qos_policy_unattached` is concerned.
    used: HashSet<&'a str>,
    /// The device has sessions that RADIUS can hand a policy to by name: PPP
    /// and tunnel sessions cloned from an `interface Virtual-Template…`, IP
    /// sessions on an interface with `ip subscriber`, IOS-XR sessions built
    /// from a `dynamic-template`, and anything authorised through `aaa
    /// authorization network` (802.1X and MAB ports, VPN users).
    sessions: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Context {
    /// Inside an `interface …` section, at any depth (`pvc`, `service
    /// instance` and the like nest).
    Interface,
    /// Inside a class of an untyped policy-map.
    Class,
    /// Anywhere else: `control-plane`, templates, map-classes, wireless
    /// profiles. Counted as use, never judged — too many platforms attach a
    /// policy of their own making there.
    Elsewhere,
}

/// What follows `service-policy` on a line that applies a policy.
enum Mention<'a> {
    /// `service-policy input|output NAME`
    Attached(&'a str),
    /// `service-policy NAME`: how a class names its child policy.
    Child(&'a str),
    TypedQos,
    OtherType,
    Unread,
}

fn mention<'a>(after: &[&'a str]) -> Mention<'a> {
    match after {
        // Both orders exist: IOS and NX-OS put the type first, OS10 after
        // the direction.
        ["type", kind, ..] | ["input" | "output", "type", kind, ..] => {
            if QOS_TYPES.contains(kind) {
                Mention::TypedQos
            } else {
                Mention::OtherType
            }
        }
        ["input" | "output", policy, ..] => Mention::Attached(policy),
        ["input" | "output"] => Mention::Unread,
        [policy] => Mention::Child(policy),
        _ => Mention::Unread,
    }
}

/// The words after `service-policy`, on a line whose job is to apply one.
/// A description that happens to contain the word is not such a line.
fn applies(line: &str) -> Option<Vec<&str>> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let keyword = match words.as_slice() {
        ["service-policy", ..] => 0,
        // Per-tunnel QoS on a DMVPN hub names its policy inside an NHRP line.
        ["ip", "nhrp", "map", "group", _, "service-policy", ..] => 5,
        ["nhrp", "map", "group", _, "service-policy", ..] => 4,
        _ => return None,
    };
    Some(words.get(keyword + 1..).unwrap_or_default().to_vec())
}

fn visit<'a>(
    node: &'a Node,
    context: Context,
    path: &mut Vec<&'a str>,
    mqc: &mut Mqc<'a>,
) -> Result<(), &'static str> {
    if path.len() >= MAX_DEPTH {
        return Err(TOO_DEEP);
    }
    // IOS XR closes its maps, route policies and sets with lines IOS does
    // not have. Its MQC looks like IOS's and is not the grammar these checks
    // were written against, so the configuration says which it is — not the
    // device's vendor.
    if matches!(
        node.text.as_str(),
        "end-policy-map" | "end-class-map" | "end-policy" | "end-set"
    ) {
        return Err(XR_STYLE);
    }
    if context == Context::Interface && node.text.starts_with("ip subscriber") {
        mqc.sessions = true;
    }
    if node.text.contains("service-policy") {
        let mut words = node
            .text
            .split_whitespace()
            .skip_while(|word| *word != "service-policy");
        if words.next().is_some() {
            mqc.used.extend(words.map(|word| word.trim_matches('"')));
        }
        if let Some(after) = applies(&node.text) {
            match (context, mention(&after)) {
                (_, Mention::TypedQos) => return Err(TYPED),
                (Context::Interface, Mention::Attached(policy))
                | (Context::Class, Mention::Child(policy)) => mqc.references.push(Reference {
                    policy,
                    place: path.join(" / "),
                    line: &node.text,
                }),
                (Context::Interface, Mention::Child(_) | Mention::Unread) => return Err(UNREAD),
                _ => {}
            }
        }
    }
    path.push(&node.text);
    for child in &node.children {
        let inner = match context {
            // A class of an untyped policy-map is where a child policy is named.
            Context::Elsewhere
                if path.len() == 1
                    && is_policy_map(&node.text)
                    && child.text.starts_with("class ") =>
            {
                Context::Class
            }
            other => other,
        };
        visit(child, inner, path, mqc)?;
    }
    path.pop();
    Ok(())
}

/// `policy-map NAME`, and not `policy-map type … NAME`.
fn is_policy_map(line: &str) -> bool {
    matches!(
        line.split_whitespace().collect::<Vec<_>>().as_slice(),
        ["policy-map", _]
    )
}

/// Read a configuration's MQC statements, or say why it cannot be judged.
fn read(config: &Config) -> Result<Mqc<'_>, &'static str> {
    if config.syntax != Syntax::Ios {
        return Err(NOT_IOS);
    }
    // `show running-config interface …` and `… | section …` print sections of
    // a configuration, and the one line every whole IOS-style configuration
    // has and no section capture does is the hostname.
    if !config.has_line_starting("hostname ") {
        return Err(PARTIAL);
    }
    let mut mqc = Mqc::default();
    let walked = config.tree.roots.iter().try_for_each(|root| {
        let words: Vec<&str> = root.text.split_whitespace().collect();
        let context = match words.as_slice() {
            ["policy-map", name] => {
                mqc.defined.push((name, root));
                Context::Elsewhere
            }
            ["policy-map", "type", kind, rest @ ..] => {
                if QOS_TYPES.contains(kind) {
                    return Err(TYPED);
                }
                mqc.typed.extend(rest.last());
                Context::Elsewhere
            }
            // A dialect that defines its maps another way (`policy-map-input
            // NAME`) and attaches them the IOS way: read as IOS, every
            // attachment would name a policy that is "not defined".
            [first, ..] if first.starts_with("policy-map") => return Err(UNREAD_MAP),
            ["interface", name, ..] => {
                if name.to_ascii_lowercase().starts_with("virtual-template") {
                    mqc.sessions = true;
                }
                Context::Interface
            }
            ["dynamic-template", ..] | ["aaa", "authorization", "network", ..] => {
                mqc.sessions = true;
                Context::Elsewhere
            }
            _ => Context::Elsewhere,
        };
        visit(root, context, &mut Vec::new(), &mut mqc)
    });
    match walked {
        // Another MQC grammar is named for what it is, whole listing or not.
        Err(reason) if reason == TYPED || reason == XR_STYLE => Err(reason),
        // Before any other complaint: the cut can be what left a line
        // unreadable, and it is the reason nothing absent can be trusted.
        _ if !config.closed => Err(CUT_OFF),
        Err(reason) => Err(reason),
        Ok(()) => Ok(mqc),
    }
}

/// A name that can go into a suggested command as it stands. The name is
/// device text and the command is something a person or the assistant may
/// send: one with a pipe, a quote or a question mark in it would be a
/// different command from the one the suggestion means.
fn plain(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:/+@".contains(c))
}

/// Policies the platform writes into the configuration itself — AutoQoS and
/// AutoConf sets, the control-plane policer of a Catalyst 9000, the wireless
/// port policy of a 3850. Most of an AutoQoS set is never attached, and with
/// `auto qos global compact` its definitions are not printed at all, so
/// neither check has anything to say about them.
fn generated(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("autoqos")
        || name.starts_with("autoconf-")
        || name == "system-cpp-policy"
        || name == "port_child_policy"
}

/// Run `judge` over every target whose configuration can be read.
fn for_each_mqc(
    world: &World,
    report: &mut CheckReport,
    judge: impl Fn(&DeviceData, &Mqc<'_>, &mut CheckReport),
) {
    for data in world.targets() {
        let Some(config) = data.config.as_ref() else {
            report.skip(data.id(), NO_CONFIG);
            continue;
        };
        match read(config) {
            Ok(mqc) => judge(data, &mqc, report),
            Err(reason) => report.skip(data.id(), reason),
        }
    }
    skip_rest(world, report, NO_CONFIG);
}

// ----------------------------------------------------------- policy missing

/// A `service-policy` line names a policy-map the configuration does not define.
///
/// Look-alikes this stays quiet about:
///
/// * **A partial configuration.** `show running-config interface …` prints
///   attachments and no definitions; every policy in it would look missing.
///   A configuration without a `hostname` line is not judged.
/// * **Policies the platform supplies.** Generated names ([`generated`]) are
///   never called missing, and a reference from outside an interface or a
///   class — `control-plane`, a wireless profile, a template — is not judged
///   at all: that is where platforms attach policies of their own that no
///   configuration defines.
/// * **A policy defined with a type.** `policy-map type control-plane X`
///   attached as plain `service-policy input X` is defined.
/// * **A listing that stops early or is in another MQC grammar** (NX-OS, EOS,
///   IOS XR): not evaluated, see [`read`].
///
/// Names are compared exactly, because IOS does: `WAN-OUT` and `wan-out` are
/// two policies. Where the only difference is the case, the finding says so —
/// that is a typing mistake, and it reads differently from a policy that is
/// simply gone.
pub fn qos_policy_missing(world: &World, _settings: &Settings, report: &mut CheckReport) {
    let redactor = Redactor::new();
    for_each_mqc(world, report, |data, mqc, report| {
        report.ok(data.id());
        let defined: HashSet<&str> = mqc.defined.iter().map(|(name, _)| *name).collect();
        // One finding per policy, however many lines name it: the fix is one
        // definition, not one per interface.
        let mut missing: BTreeMap<&str, Vec<&Reference<'_>>> = BTreeMap::new();
        for reference in &mqc.references {
            if defined.contains(reference.policy)
                || mqc.typed.contains(reference.policy)
                || generated(reference.policy)
            {
                continue;
            }
            missing.entry(reference.policy).or_default().push(reference);
        }
        for (policy, references) in missing {
            let mut finding = Finding::new(
                "qos_policy_missing",
                data.id(),
                format!("policy-map {policy}"),
                format!(
                    "{} references policy-map {policy}, which its configuration does not define",
                    data.name()
                ),
            )
            .detail(
                "A `service-policy` line names a policy-map that is not in the same \
                 configuration, so the classification, queueing or policing it was meant to apply \
                 is not there to apply. A device normally refuses such a line; where it is in the \
                 configuration anyway, either the policy went missing some other way or the \
                 stored configuration is not the whole of what the device runs.",
            );
            for reference in references.iter().take(QUOTED_LINES) {
                finding = finding.evidence(
                    redactor.redact(&reference.place),
                    redactor.redact(reference.line),
                );
            }
            if references.len() > QUOTED_LINES {
                finding = finding.evidence(
                    "Further references",
                    format!("{} more lines name it", references.len() - QUOTED_LINES),
                );
            }
            finding = finding.evidence(
                "Definition",
                format!("no `policy-map {policy}` in the stored configuration"),
            );
            let mut alike: Vec<&str> = mqc
                .defined
                .iter()
                .map(|(name, _)| *name)
                .chain(mqc.typed.iter().copied())
                .filter(|name| name.eq_ignore_ascii_case(policy))
                .collect();
            alike.sort_unstable();
            alike.dedup();
            if !alike.is_empty() {
                finding = finding.evidence(
                    "Defined under a similar name",
                    format!(
                        "`policy-map {}` — policy names are case-sensitive, so that is a \
                         different policy",
                        alike.join("`, `policy-map ")
                    ),
                );
            }
            let mut commands = Vec::new();
            if plain(policy) {
                commands.push(format!("show policy-map {policy}"));
            }
            commands.push("show running-config | include policy-map|service-policy".to_string());
            report.found(
                finding.suggest(
                    Suggestion::text(
                        "Ask the device first. If it has the policy, the stored configuration \
                         is incomplete and a fresh backup settles it; if it does not, define \
                         the policy or take the reference away.",
                    )
                    .commands(commands),
                ),
            );
        }
    });
}

// -------------------------------------------------------- policy unattached

/// A policy-map that nothing in the configuration references.
///
/// Look-alikes this stays quiet about:
///
/// * **A child policy.** `service-policy CHILD` inside a class of another
///   policy is a reference like any other.
/// * **Any other mention.** `control-plane`, a template, a map-class, an NHRP
///   group map, a wireless profile: whatever follows `service-policy` on any
///   line of the configuration counts as use, whether or not this module
///   knows the grammar around it.
/// * **Generated policies** ([`generated`]): an AutoQoS set is mostly
///   unattached by design, and nobody wrote it.
/// * **A partial configuration**, where the attachment may be in the part
///   that is not there.
/// * **Session policies.** A BNG, a VPN headend or an 802.1X switch can be
///   told by RADIUS to apply a policy to a session by name; on such a device
///   (one with a virtual template, a dynamic template, `ip subscriber` on an
///   interface or `aaa authorization network`) "nothing references it" is
///   not knowable from the configuration.
/// * **A listing that stops early.** IOS prints its policy-maps before its
///   interfaces, so a listing cut anywhere after them has every definition
///   and none of the attachments. Without its closing `end` it is not judged.
///
/// Names are compared exactly, because IOS does. A policy that differs only
/// in case from one that *is* named is reported — it is a different policy,
/// usually a typing mistake left behind — and the finding says which.
pub fn qos_policy_unattached(world: &World, _settings: &Settings, report: &mut CheckReport) {
    let redactor = Redactor::new();
    for_each_mqc(world, report, |data, mqc, report| {
        if mqc.sessions {
            report.skip(data.id(), SESSIONS);
            return;
        }
        report.ok(data.id());
        let mut seen = HashSet::new();
        for (policy, node) in &mqc.defined {
            // One finding a policy, also where a listing repeats a section.
            if mqc.used.contains(policy) || generated(policy) || !seen.insert(*policy) {
                continue;
            }
            let classes: Vec<String> = node
                .children
                .iter()
                .filter(|child| child.text.starts_with("class "))
                .map(|child| redactor.redact(&child.text))
                .collect();
            let shown = classes
                .iter()
                .take(QUOTED_LINES)
                .cloned()
                .collect::<Vec<_>>()
                .join(" / ");
            let classes = match classes.len() {
                0 => "none".to_string(),
                count if count > QUOTED_LINES => {
                    format!("{shown} … and {} more", count - QUOTED_LINES)
                }
                _ => shown,
            };
            let mut finding = Finding::new(
                "qos_policy_unattached",
                data.id(),
                format!("policy-map {policy}"),
                format!("policy-map {policy} on {} is attached nowhere", data.name()),
            )
            .detail(
                "No interface and no class of another policy references it, so it does \
                 nothing. Either it is left over from something that was removed, or it was \
                 written and never attached — in which case the QoS it describes is not in \
                 effect. A policy that RADIUS or a controller applies by name does not show in \
                 the configuration and would look the same.",
            )
            .evidence("Configuration", redactor.redact(&node.text))
            .evidence("Classes", classes)
            .evidence(
                "References",
                "no `service-policy` line in the stored configuration names it".to_string(),
            );
            let mut alike: Vec<&str> = mqc
                .used
                .iter()
                .copied()
                .filter(|name| name.eq_ignore_ascii_case(policy))
                .collect();
            alike.sort_unstable();
            if !alike.is_empty() {
                finding = finding.evidence(
                    "Named under a similar name",
                    format!(
                        "a `service-policy` line names `{}` — policy names are case-sensitive, \
                         so that is a different policy",
                        alike.join("`, `")
                    ),
                );
            }
            let mut commands = Vec::new();
            if plain(policy) {
                commands.push(format!("show policy-map {policy}"));
            }
            commands.push("show running-config | include service-policy".to_string());
            report.found(
                finding.suggest(
                    Suggestion::text(
                        "Decide which it is. If it should be in effect, attach it where it \
                         belongs; if it is left over, remove it so the next person does not have \
                         to ask.",
                    )
                    .commands(commands),
                ),
            );
        }
    });
}

// ----------------------------------------------------------- class dropping

/// Congestion drops per second and class — only where the device reports
/// them apart from policer drops.
const QUEUE_DROPS: &str = "qos.queue_drop_ps";
/// Every drop per second and class, whatever the reason.
const ALL_DROPS: &str = "qos.drop_ps";
const DROP_SHARE: &str = "qos.drop_pct";
const TRAFFIC: &str = "qos.bps";

const NO_SAMPLES: &str = "no QoS samples for this device: its device type has no QoS read, \
     monitoring is not polling it, the read has never succeeded, or no policy is attached";
const NONE_IN_WINDOW: &str = "no QoS samples inside the window";
const STOPPED: &str = "the newest QoS sample is older than the sustained period \
     (`thresholds.sustained_minutes`, at least 15 minutes): the read is failing, monitoring has \
     stopped or it reads QoS less often than that, so what the classes are doing now is not known";
const NO_THRESHOLD: &str = "`thresholds.qos_drop_ps` is not above zero, so every measured class \
     would count as dropping";
const UNREADABLE: &str = "the QoS samples could not be read from the store, so whether any class \
     is dropping is not known";
const UNDECIDED: &str = "no class could be judged yet: since the last gap in its samples each was \
     measured too briefly to tell sustained dropping from a burst (fewer than five samples, or \
     less than `thresholds.sustained_minutes`), or its own samples have stopped";
/// A figure beside a finding whose series is not there…
const NOT_RECORDED: &str = "not recorded for this class";
/// …and one whose series could not be read, which is another thing.
const NOT_READ: &str = "could not be read";

/// The subject of a class's finding: the series' own name with the
/// separators made readable. It is what keeps the finding's identity however
/// the policy and class are told apart, and what a class that could not be
/// judged is held by.
fn subject_of(series: &Series) -> String {
    series.instance.replace('|', " ")
}

/// A class as monitoring names its series: `<interface>|<in|out>|<policy>/<class>`.
struct Class<'a> {
    interface: &'a str,
    direction: &'a str,
    policy: &'a str,
    class: &'a str,
}

fn class_of(instance: &str) -> Option<Class<'_>> {
    let mut parts = instance.splitn(3, '|');
    let (interface, direction, rest) = (parts.next()?, parts.next()?, parts.next()?);
    let direction = match direction {
        "in" => "input",
        "out" => "output",
        _ => return None,
    };
    // Split at the last slash: a policy is more often named after an
    // interface (`AutoQoS-Policy-Se0/0`) than a class is.
    let (policy, class) = rest.rsplit_once('/')?;
    Some(Class {
        interface,
        direction,
        policy,
        class,
    })
}

/// A class of a QoS policy that kept dropping packets.
///
/// Look-alikes this stays quiet about:
///
/// * **A policer doing its job.** Where a device reports congestion (queue)
///   drops apart from the rest, only those count. A class whose every drop is
///   a policer's — including a class with no queue at all, which has no
///   congestion series — is the policy working, not a fault.
/// * **One burst.** A class must be over the threshold for most of a window
///   of at least the sustained period, with enough samples in it to mean
///   anything.
/// * **A device nobody measures.** No QoS samples is "not evaluated", never
///   "not dropping" — and so is a device whose samples *stop*: a read that
///   started failing twenty minutes ago leaves older samples in the window,
///   and they say nothing about now, neither "clean" nor "still dropping".
/// * **A series with a hole in it.** A failed read writes no sample. Only
///   the run of samples since the last hole is judged, so that the minutes a
///   finding quotes are minutes that were measured.
/// * **A class that cannot be judged now** — measured too briefly since its
///   last hole, with drops in what was measured, or whose own series stopped
///   while the device's goes on — is neither dropping nor clean. It is
///   marked undecided, so that a finding about it is held as it is rather
///   than resolved on no measurement and opened again as a second
///   occurrence when the run is long enough once more. A device with no
///   class that could be judged is not evaluated.
/// * **A read that failed** is not an empty one: the device is not
///   evaluated, and a figure beside a finding that could not be read says so
///   rather than "not recorded".
///
/// Where a device has no congestion series at all — its platform keeps one
/// drop counter for both reasons, or none of its classes has a queue — there
/// is nothing to tell the two apart with. The finding then says exactly that
/// and is one step lower, rather than calling a rate limit congestion.
pub fn qos_class_dropping(world: &World, settings: &Settings, report: &mut CheckReport) {
    let thresholds = &settings.thresholds;
    // Zero or less would call every class that was measured at all a
    // dropping one. That is a setting to put right, not a result.
    if thresholds.qos_drop_ps.is_nan() || thresholds.qos_drop_ps <= 0.0 {
        skip_rest(world, report, NO_THRESHOLD);
        return;
    }
    let sustained = i64::from(thresholds.sustained_minutes.max(15)) * 60_000;
    let since = world.now - sustained * 4;
    // One length for both questions: two samples further apart than this
    // have a hole between them, and a newest sample older than this says
    // nothing about now. With monitoring's default of a read every five
    // minutes that is three reads missed in a row.
    let hole = sustained;
    let fresh_from = world.now - hole;
    let history = world.now - i64::from(thresholds.history_hours) * 3_600_000;

    for data in world.targets() {
        // Decided per device, not per class: on a device that separates the
        // two, a class with drops and no congestion series is a class
        // without a queue, and its drops are a policer's.
        let separated = world.has_metric(data.id(), QUEUE_DROPS);
        if !separated && !world.has_metric(data.id(), ALL_DROPS) {
            report.skip(data.id(), NO_SAMPLES);
            continue;
        }
        let metric = if separated { QUEUE_DROPS } else { ALL_DROPS };
        let Ok(runs) = metrics::summarise_unbroken(
            world.store(),
            data.id(),
            metric,
            since,
            thresholds.qos_drop_ps,
            hole,
        ) else {
            report.skip(data.id(), UNREADABLE);
            continue;
        };
        let mut newest = runs.iter().map(|series| series.last_at).max();
        // Classes that are measured now and have no congestion series have no
        // queue to overflow: that is a verdict, not a gap. So a device whose
        // queueing policy is gone is still being measured for as long as its
        // other classes are.
        let mut queueless = false;
        if separated && newest.is_none_or(|at| at < fresh_from) {
            let Ok(any) = metrics::summarise(world.store(), data.id(), ALL_DROPS, since, 0.0)
            else {
                report.skip(data.id(), UNREADABLE);
                continue;
            };
            let measured = any.iter().map(|series| series.last_at).max();
            queueless = measured.is_some_and(|at| at >= fresh_from);
            newest = newest.max(measured);
        }
        match newest {
            None => {
                report.skip(data.id(), NONE_IN_WINDOW);
                continue;
            }
            Some(at) if at < fresh_from => {
                report.skip(data.id(), STOPPED);
                continue;
            }
            Some(_) => {}
        }
        let mut decided = queueless;
        let mut dropping: Vec<&Series> = Vec::new();
        for series in &runs {
            let long_enough = series.points >= 5
                && series.span_minutes() >= i64::from(thresholds.sustained_minutes);
            // A class whose own series stopped while the device's did not is
            // gone, or its counters could not be compared; one measured too
            // briefly since a hole, with drops in what was measured, may be
            // dropping or may have had a burst. A run of samples all under
            // the threshold is a class not dropping now, however short.
            if series.last_at < fresh_from || (!long_enough && series.over > 0) {
                report.undecided(data.id(), &subject_of(series));
                continue;
            }
            decided = true;
            if long_enough && series.over_fraction() >= 0.75 {
                dropping.push(series);
            }
        }
        if !decided {
            report.skip(data.id(), UNDECIDED);
            continue;
        }
        report.ok(data.id());

        for series in dropping {
            let class = class_of(&series.instance);
            // The other figures of the class over the stretch its drops were
            // judged over — read only for a class that is reported, so a
            // healthy device costs one query.
            let beside =
                |metric: &str, shown: &dyn Fn(&Series) -> String| match metrics::summarise_one(
                    world.store(),
                    data.id(),
                    metric,
                    &series.instance,
                    series.first_at,
                ) {
                    Ok(Some(figures)) => shown(&figures),
                    Ok(None) => NOT_RECORDED.to_string(),
                    Err(_) => NOT_READ.to_string(),
                };
            let what = match &class {
                Some(class) => format!(
                    "Class {} of policy {} ({} on {})",
                    class.class, class.policy, class.direction, class.interface
                ),
                None => format!("QoS class {}", series.instance),
            };
            let (title, severity, detail, drops) = if separated {
                (
                    format!(
                        "{what} has been dropping {:.0} packets a second to congestion for {} \
                         minutes",
                        series.avg,
                        series.span_minutes()
                    ),
                    Severity::Medium,
                    "The class's queue is overflowing: more traffic is being offered to it than \
                     the policy lets through, and the excess is discarded. These are congestion \
                     drops, counted apart from anything a policer discards on purpose. Either \
                     the class is given too little, or something is in it that should not be.",
                    "Congestion drops",
                )
            } else {
                (
                    format!(
                        "{what} has been dropping {:.0} packets a second for {} minutes \
                         (congestion or a rate limit: the counters do not say which)",
                        series.avg,
                        series.span_minutes()
                    ),
                    Severity::Low,
                    "No congestion counter is recorded for this device's classes — its platform \
                     keeps one drop counter for a class, or these classes have no queue — so \
                     drops caused by congestion cannot be told from drops a policer or a rate \
                     limit makes on purpose. If the class is meant to be held to its rate, this \
                     is the policy working and can be dismissed; if it is not, the class is \
                     congested.",
                    "Drops (congestion and policing together)",
                )
            };
            let mut finding =
                Finding::new("qos_class_dropping", data.id(), subject_of(series), title)
                    .severity(severity)
                    .detail(detail)
                    .evidence(
                        drops,
                        format!(
                            "average {:.1}/s, peak {:.1}/s (threshold {:.1}/s)",
                            series.avg, series.max, thresholds.qos_drop_ps
                        ),
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
                    .evidence(
                        "Share of offered packets dropped",
                        beside(DROP_SHARE, &|share| {
                            format!(
                                "average {:.1} %, peak {:.1} % (every drop, whatever the reason)",
                                share.avg, share.max
                            )
                        }),
                    )
                    .evidence(
                        "Traffic in the class",
                        beside(TRAFFIC, &|traffic| {
                            format!(
                                "average {}, peak {}",
                                norm::speed(traffic.avg.max(0.0).round() as u64),
                                norm::speed(traffic.max.max(0.0).round() as u64)
                            )
                        }),
                    );
            match metrics::busiest_hour(world.store(), data.id(), metric, &series.instance, history)
            {
                Ok(Some((hour, average))) => {
                    finding = finding.evidence(
                        "Peak hour",
                        format!("{hour:02}:00 UTC, averaging {average:.1} drops/s"),
                    );
                }
                // No hour with samples in it is no peak to speak of.
                Ok(None) => {}
                Err(_) => finding = finding.evidence("Peak hour", NOT_READ),
            }
            let mut suggestion = Suggestion::text(if separated {
                "Look at what is in the class before giving it more: traffic marked into the \
                 wrong class is cheaper to fix than a bigger queue. If the class is right and \
                 the link is simply full, the policy's shares or the link have to change."
            } else {
                "Check whether the class is meant to be held to a rate. If it is, the drops are \
                 the limit being enforced; if not, look at what is in the class and at the rate \
                 it is given."
            });
            // The command is IOS grammar, so it is offered only where the
            // device's own grammar is.
            if let (Some(class), Syntax::Ios) = (&class, data.syntax)
                && plain(class.interface)
            {
                suggestion = suggestion.commands([format!(
                    "show policy-map interface {} {}",
                    class.interface, class.direction
                )]);
            }
            report.found(finding.suggest(suggestion));
        }
    }
    skip_rest(world, report, NO_SAMPLES);
}

#[cfg(test)]
#[path = "qos_tests.rs"]
mod tests;
