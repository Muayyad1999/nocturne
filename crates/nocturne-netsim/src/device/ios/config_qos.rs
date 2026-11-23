//! Class maps, policy maps and service policies: the modular QoS command line.
//!
//! The syntax is Cisco's, from the "Cisco IOS Quality of Service Solutions
//! Command Reference" (`class-map`, `match dscp`, `match access-group`,
//! `policy-map`, `class`, `priority`, `bandwidth`, `police`, `shape`,
//! `set dscp`, `service-policy`):
//! <https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos/command/qos-cr-book/>.
//! Only what the drop model in `sim::qos` acts on is accepted. Everything else
//! is refused like any other unknown command, so a change rehearsed in a lab
//! never looks better than it is.
//!
//! The refusals are the simulator's own wording, except `% class map NAME not
//! configured`, which the reference quotes.

use crate::device::cli::grammar::Args;
use crate::device::config::{
    CLASS_DEFAULT, ClassMap, ClassMatch, DeviceConfig, POLICY_CLASSES, POLICY_DEPTH, Police,
    PolicyClass, PolicyMap, Share,
};
use crate::device::ios::{Cx, Handler, Mode, Table, c, n};
use crate::ifname::IfId;
use crate::sim::qos;
use crate::util::dscp;

/// Most code points one `match dscp` statement lists.
const MATCH_DSCP_VALUES: usize = 8;

fn end(cx: &mut Cx, _: &Args) {
    let reply = cx.sh.leave_config(cx.n, cx.fx);
    cx.out.push_str(&reply.out);
}

fn exit_to_config(cx: &mut Cx, _: &Args) {
    cx.sh.mode = Mode::Config;
}

/// Leaving a class goes back to its policy map, not all the way out.
fn exit_to_policy(cx: &mut Cx, _: &Args) {
    if let Mode::PolicyClass(policy, _) = cx.sh.mode.clone() {
        cx.sh.mode = Mode::PolicyMap(policy);
    }
}

fn common(exit: Handler) -> Table {
    vec![
        c("exit", "Exit from the current configuration mode", exit),
        c("end", "Exit from configure mode", end),
    ]
}

// ------------------------------------------------------------------ global

pub(crate) fn global_table() -> Table {
    vec![
        n(
            "class-map [match-all | match-any] <word:name;class-map name>",
            "Configure QoS Class Map",
            class_map,
        ),
        n(
            "policy-map <word:name;policy-map name>",
            "Configure QoS Policy Map",
            policy_map,
        ),
    ]
}

fn class_map(cx: &mut Cx, a: &Args) {
    let name = a.text("name").unwrap_or("").to_string();
    let any = a.has("match-any");
    let explicit = any || a.has("match-all");
    // With the name left off, the keyword itself is read as the name.
    let keyword = ["match-all", "match-any"]
        .iter()
        .any(|k| name.eq_ignore_ascii_case(k));
    if name == CLASS_DEFAULT || (keyword && !explicit) {
        return cx.invalid();
    }
    if a.no {
        let used =
            cx.n.cfg
                .policy_maps
                .iter()
                .any(|policy| policy.classes.iter().any(|class| class.name == name));
        if used {
            return cx.fail(format!("%Class-map {name} is being used"));
        }
        cx.cfg().class_maps.retain(|map| map.name != name);
        return;
    }
    match cx.n.cfg.class_map(&name).map(|map| map.any) {
        Some(existing) if explicit && existing != any => {
            if let Some(map) = cx.cfg().class_maps.iter_mut().find(|m| m.name == name) {
                map.any = any;
            }
        }
        Some(_) => {}
        None => cx.cfg().class_maps.push(ClassMap {
            name: name.clone(),
            any,
            description: String::new(),
            matches: Vec::new(),
        }),
    }
    cx.sh.mode = Mode::ClassMap(name);
}

/// Is the policy attached to an interface, or applied inside another policy?
fn policy_in_use(cfg: &DeviceConfig, name: &str) -> bool {
    cfg.interfaces.values().any(|iface| {
        iface.policy_in.as_deref() == Some(name) || iface.policy_out.as_deref() == Some(name)
    }) || cfg.policy_maps.iter().any(|policy| {
        policy
            .classes
            .iter()
            .any(|class| class.child.as_deref() == Some(name))
    })
}

fn policy_map(cx: &mut Cx, a: &Args) {
    let name = a.text("name").unwrap_or("").to_string();
    if a.no {
        if policy_in_use(&cx.n.cfg, &name) {
            return cx.fail(format!("%Policy map {name} is being used"));
        }
        cx.cfg().policy_maps.retain(|policy| policy.name != name);
        return;
    }
    if cx.n.cfg.policy_map(&name).is_none() {
        cx.cfg().policy_maps.push(PolicyMap {
            name: name.clone(),
            description: String::new(),
            classes: Vec::new(),
        });
    }
    cx.sh.mode = Mode::PolicyMap(name);
}

// --------------------------------------------------------------- class map

fn class_map_mut<'c>(cx: &'c mut Cx) -> Option<&'c mut ClassMap> {
    let Mode::ClassMap(name) = cx.sh.mode.clone() else {
        return None;
    };
    cx.cfg().class_maps.iter_mut().find(|map| map.name == name)
}

/// Add a `match` statement, or with `no` take it away.
fn set_match(cx: &mut Cx, a: &Args, statement: ClassMatch) {
    let Some(map) = class_map_mut(cx) else { return };
    map.matches.retain(|existing| *existing != statement);
    if !a.no {
        map.matches.push(statement);
    }
}

pub(crate) fn class_map_table() -> Table {
    let mut t = common(exit_to_config);
    t.extend(vec![
        n(
            "match [ip] dscp <word:dscp;Differentiated services codepoint value> {<word:dscp>}",
            "classification criteria",
            |cx, a| {
                let typed = a.texts("dscp");
                let values: Option<Vec<u8>> = typed.iter().map(|text| dscp::parse(text)).collect();
                let Some(mut values) = values.filter(|v| v.len() <= MATCH_DSCP_VALUES) else {
                    return cx.invalid();
                };
                values.sort_unstable();
                values.dedup();
                let ip = a.has("ip");
                if a.no {
                    // `no match dscp ef` takes the value out of whichever statement lists it.
                    let Some(map) = class_map_mut(cx) else { return };
                    for statement in &mut map.matches {
                        if let ClassMatch::Dscp { values: listed, ip: same } = statement
                            && *same == ip
                        {
                            listed.retain(|value| !values.contains(value));
                        }
                    }
                    map.matches.retain(
                        |m| !matches!(m, ClassMatch::Dscp { values, .. } if values.is_empty()),
                    );
                    return;
                }
                set_match(cx, a, ClassMatch::Dscp { values, ip });
            },
        ),
        n(
            "match access-group (<1-2699:number;Access list index> | name <word:acl;Access list name>)",
            "classification criteria",
            |cx, a| {
                let statement = match (a.num("number"), a.text("acl")) {
                    (Some(number), _) => ClassMatch::AccessGroup {
                        acl: number.to_string(),
                        named: false,
                    },
                    (None, Some(name)) => ClassMatch::AccessGroup {
                        acl: name.to_string(),
                        named: true,
                    },
                    (None, None) => return cx.invalid(),
                };
                set_match(cx, a, statement);
            },
        ),
        n("match any", "classification criteria", |cx, a| {
            set_match(cx, a, ClassMatch::Any)
        }),
        n(
            "description ~ <line:text;Up to 200 characters describing this class-map>",
            "Class-Map description",
            |cx, a| {
                let text = if a.no {
                    String::new()
                } else {
                    a.text("text").unwrap_or("").to_string()
                };
                if let Some(map) = class_map_mut(cx) {
                    map.description = text;
                }
            },
        ),
    ]);
    t
}

// -------------------------------------------------------------- policy map

fn policy_name(cx: &Cx) -> Option<String> {
    match &cx.sh.mode {
        Mode::PolicyMap(policy) | Mode::PolicyClass(policy, _) => Some(policy.clone()),
        _ => None,
    }
}

/// `class NAME`: open (or with `no`, remove) a class of the policy map.
fn class(cx: &mut Cx, a: &Args) {
    let Some(policy) = policy_name(cx) else {
        return;
    };
    let name = a.text("name").unwrap_or("").to_string();
    if a.no {
        if let Some(map) = cx.cfg().policy_maps.iter_mut().find(|p| p.name == policy) {
            map.classes.retain(|class| class.name != name);
        }
        cx.sh.mode = Mode::PolicyMap(policy);
        return;
    }
    if name != CLASS_DEFAULT && cx.n.cfg.class_map(&name).is_none() {
        return cx.fail(format!("% class map {name} not configured"));
    }
    let known =
        cx.n.cfg
            .policy_map(&policy)
            .is_some_and(|map| map.classes.iter().any(|class| class.name == name));
    if !known && let Some(map) = cx.cfg().policy_maps.iter_mut().find(|p| p.name == policy) {
        // `class-default` stays last, wherever in the policy it was written.
        let at = map
            .classes
            .iter()
            .position(|class| class.name == CLASS_DEFAULT)
            .unwrap_or(map.classes.len());
        map.classes.insert(at, PolicyClass::new(&name));
        // The class is one more in every policy this one is applied inside.
        if let Some(reason) = too_many_classes(&cx.n.cfg) {
            if let Some(map) = cx.cfg().policy_maps.iter_mut().find(|p| p.name == policy) {
                map.classes.retain(|class| class.name != name);
            }
            cx.sh.mode = Mode::PolicyMap(policy);
            return cx.fail(reason);
        }
    }
    cx.sh.mode = Mode::PolicyClass(policy, name);
}

/// The refusal for a policy that has grown past what one policy may hold.
/// Worded with "cannot be" like the other refusals here: that is what a
/// driver's error patterns recognise a refused line by.
fn too_many_classes(cfg: &DeviceConfig) -> Option<String> {
    cfg.oversized_policy().map(|policy| {
        format!(
            "% policy map {policy} cannot be extended: a policy holds {POLICY_CLASSES} classes at most, those of its child policies included"
        )
    })
}

const CLASS: &str = "class <word:name;class-map name>";

pub(crate) fn policy_map_table() -> Table {
    let mut t = common(exit_to_config);
    t.extend(vec![
        n(CLASS, "policy criteria", class),
        n(
            "description ~ <line:text;Up to 200 characters describing this policy-map>",
            "Policy-Map description",
            |cx, a| {
                let Some(policy) = policy_name(cx) else {
                    return;
                };
                let text = if a.no {
                    String::new()
                } else {
                    a.text("text").unwrap_or("").to_string()
                };
                if let Some(map) = cx.cfg().policy_maps.iter_mut().find(|p| p.name == policy) {
                    map.description = text;
                }
            },
        ),
    ]);
    t
}

// ------------------------------------------------------------ policy class

fn class_ref<'c>(cx: &'c Cx) -> Option<&'c PolicyClass> {
    let Mode::PolicyClass(policy, class) = &cx.sh.mode else {
        return None;
    };
    cx.n.cfg
        .policy_map(policy)?
        .classes
        .iter()
        .find(|c| c.name == *class)
}

fn class_mut<'c>(cx: &'c mut Cx) -> Option<&'c mut PolicyClass> {
    let Mode::PolicyClass(policy, class) = cx.sh.mode.clone() else {
        return None;
    };
    cx.cfg()
        .policy_maps
        .iter_mut()
        .find(|p| p.name == policy)?
        .classes
        .iter_mut()
        .find(|c| c.name == class)
}

/// Is the policy (directly, or as a child) attached to an interface for input?
fn used_on_input(cfg: &DeviceConfig, policy: &str) -> bool {
    cfg.interfaces.values().any(|iface| {
        iface
            .policy_in
            .as_deref()
            .is_some_and(|top| cfg.policy_reaches(top, policy))
    })
}

const INPUT_QUEUEING: &str =
    "% Queueing (priority, bandwidth, shape) is not supported in an input service policy";

/// Refuse a queueing action where there is no queue: in a policy that is
/// applied to arriving traffic.
fn refuse_queueing_on_input(cx: &mut Cx) -> bool {
    let attached = policy_name(cx).is_some_and(|policy| used_on_input(&cx.n.cfg, &policy));
    if attached {
        cx.fail(INPUT_QUEUEING);
    }
    attached
}

fn share(a: &Args) -> Option<Share> {
    match (a.num("percent"), a.num("kbps")) {
        (Some(percent), _) => Some(Share::Percent(percent as u8)),
        (None, Some(kbps)) => Some(Share::Kbps(kbps as u32)),
        (None, None) => None,
    }
}

fn priority(cx: &mut Cx, a: &Args) {
    let Some(current) = class_ref(cx).cloned() else {
        return;
    };
    if !a.no {
        if current.name == CLASS_DEFAULT {
            return cx.fail("% The default class cannot be a priority class");
        }
        if current.bandwidth.is_some() || current.shape.is_some() {
            return cx
                .fail("% priority cannot be configured in a class that has bandwidth or shape");
        }
        if refuse_queueing_on_input(cx) {
            return;
        }
    }
    let value = (!a.no).then(|| share(a));
    if let Some(class) = class_mut(cx) {
        class.priority = value;
    }
}

fn bandwidth(cx: &mut Cx, a: &Args) {
    let Some(current) = class_ref(cx).cloned() else {
        return;
    };
    if !a.no {
        if current.priority.is_some() {
            return cx.fail("% bandwidth cannot be configured in a class that has priority");
        }
        if refuse_queueing_on_input(cx) {
            return;
        }
    }
    let value = if a.no { None } else { share(a) };
    if let Some(class) = class_mut(cx) {
        class.bandwidth = value;
    }
}

fn shape(cx: &mut Cx, a: &Args) {
    let Some(current) = class_ref(cx).cloned() else {
        return;
    };
    if !a.no {
        if current.priority.is_some() {
            return cx.fail("% shape cannot be configured in a class that has priority");
        }
        if refuse_queueing_on_input(cx) {
            return;
        }
    }
    let value = if a.no { None } else { a.num("bps") };
    if let Some(class) = class_mut(cx) {
        class.shape = value;
    }
}

fn police(cx: &mut Cx, a: &Args) {
    let value = match (a.no, a.num("bps")) {
        (true, _) => None,
        (false, Some(cir)) => Some(Police {
            cir,
            cir_keyword: a.has("cir"),
            actions: a.has("conform-action"),
        }),
        (false, None) => return cx.invalid(),
    };
    if let Some(class) = class_mut(cx) {
        class.police = value;
    }
}

fn set_dscp(cx: &mut Cx, a: &Args) {
    let value = match (a.no, a.text("dscp").map(dscp::parse)) {
        (true, _) => None,
        (false, Some(Some(value))) => Some(value),
        _ => return cx.invalid(),
    };
    if let Some(class) = class_mut(cx) {
        class.set_dscp = value;
    }
}

/// `service-policy CHILD` inside a class: a policy for the class's own traffic.
fn child_policy(cx: &mut Cx, a: &Args) {
    let Some(policy) = policy_name(cx) else {
        return;
    };
    if a.no {
        if let Some(class) = class_mut(cx) {
            class.child = None;
        }
        return;
    }
    let child = a.text("child").unwrap_or("").to_string();
    let cfg = &cx.n.cfg;
    if cfg.policy_map(&child).is_none() {
        return cx.fail(format!("% policy map {child} not configured"));
    }
    if cfg.policy_reaches(&child, &policy) {
        return cx.fail(format!(
            "% policy map {child} cannot be applied inside {policy}: it contains it"
        ));
    }
    if cfg.policy_height(&policy) + cfg.policy_depth(&child) > POLICY_DEPTH {
        return cx.fail(format!(
            "% policy map {child} cannot be nested: service policies go {POLICY_DEPTH} levels deep at most"
        ));
    }
    let queues = qos::plan(cfg, &child).is_some_and(|level| level.queues_anywhere());
    if queues && used_on_input(cfg, &policy) {
        return cx.fail(INPUT_QUEUEING);
    }
    let before = class_ref(cx).and_then(|class| class.child.clone());
    if let Some(class) = class_mut(cx) {
        class.child = Some(child);
    }
    // The child's classes now stand under this class, wherever this policy
    // is applied in turn.
    if let Some(reason) = too_many_classes(&cx.n.cfg) {
        if let Some(class) = class_mut(cx) {
            class.child = before;
        }
        cx.fail(reason);
    }
}

pub(crate) fn policy_class_table() -> Table {
    let mut t = common(exit_to_policy);
    t.extend(vec![
        // Naming the next class from inside one moves on to it, as on real devices.
        n(CLASS, "policy criteria", class),
        n(
            "priority ~ [<1-2000000:kbps;Kilo Bits per second> | percent <1-100:percent;percentage>]",
            "Strict Scheduling Priority for this Class",
            priority,
        ),
        n(
            "bandwidth ~ (<1-2000000:kbps;Kilo Bits per second> | percent <1-100:percent;percentage>)",
            "Bandwidth",
            bandwidth,
        ),
        n(
            "police ~ [cir] <8000-128000000000:bps;Bits per second> [conform-action transmit exceed-action drop]",
            "Police",
            police,
        ),
        n(
            "shape ~ average <8000-128000000000:bps;Target Bit Rate (bits/sec)>",
            "Traffic Shaping",
            shape,
        ),
        n(
            "set dscp ~ <word:dscp;Differentiated services codepoint value>",
            "Set QoS values",
            set_dscp,
        ),
        n(
            "service-policy ~ <word:child;policy-map name>",
            "Configure QoS Service Policy",
            child_policy,
        ),
    ]);
    t
}

// --------------------------------------------------------------- interface

/// `service-policy {input | output} NAME` on an interface.
pub(crate) fn service_policy(cx: &mut Cx, a: &Args) {
    let Mode::If(ids) = cx.sh.mode.clone() else {
        return;
    };
    let output = a.has("output");
    let name = a.text("name").map(str::to_string);
    let attached = |cfg: &DeviceConfig, id: IfId| {
        let iface = cfg.iface(id)?;
        if output {
            iface.policy_out.clone()
        } else {
            iface.policy_in.clone()
        }
    };
    if !a.no {
        let Some(name) = &name else {
            return cx.invalid();
        };
        // Flow traffic is accounted on the ports it crosses; a policy on a
        // logical interface would sit there counting nothing.
        if let Some(logical) = ids.iter().find(|id| !id.is_physical()) {
            return cx.fail(format!(
                "% Service policies are not supported on {} in this simulator: attach to the physical port",
                logical.name()
            ));
        }
        let Some(level) = qos::plan(&cx.n.cfg, name) else {
            return cx.fail(format!("% policy map {name} not configured"));
        };
        if !output && level.queues_anywhere() {
            return cx.fail(INPUT_QUEUEING);
        }
        if let Some(other) = ids
            .iter()
            .filter_map(|id| attached(&cx.n.cfg, *id))
            .find(|other| other != name)
        {
            return cx.fail(format!("Policy map {other} is already attached"));
        }
    }
    for id in ids {
        let current = attached(&cx.n.cfg, id);
        let wanted = match (&name, a.no) {
            (Some(name), false) => Some(name.clone()),
            // `no service-policy output OTHER` leaves what is attached alone.
            (Some(name), true) if current.as_ref() != Some(name) => current.clone(),
            _ => None,
        };
        if wanted == current {
            continue;
        }
        if let Some(iface) = cx.cfg().interfaces.get_mut(&id) {
            if output {
                iface.policy_out = wanted;
            } else {
                iface.policy_in = wanted;
            }
        }
        // A policy starts counting when it is attached.
        cx.n.qos.remove(&(id, output));
    }
}
