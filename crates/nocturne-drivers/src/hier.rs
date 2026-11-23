//! Hierarchical configuration: parse, compare, and compute remediation.
//!
//! Given the configuration a device *has* and the configuration it *should
//! have*, [`remediate`] works out the exact commands that close the gap — the
//! same idea as Ansible's resource modules or `hier_config`, but as a pure
//! function over text. It is what powers "edit the config as a document and
//! push the difference", drift repair, compliance fixes and rollback (the
//! rollback of a change is simply the remediation in the other direction).
//!
//! The engine knows three things about a platform's grammar:
//!
//! * how to **negate** a line (`no …`, `undo …`, `delete …`) including the
//!   lines that need a shorter negation (`no username bob`, not the full line);
//! * which commands are **replace-style** — setting a new value overwrites the
//!   old one, so the old line must *not* be negated first (negating
//!   `ip address` on the interface you are managing the device through is how
//!   people lock themselves out);
//! * which sections are **order-sensitive** (access lists) and so are rebuilt
//!   rather than patched.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Configuration grammar family.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Syntax {
    /// Indented blocks negated with `no` (IOS, IOS-XE, NX-OS, EOS, AOS-CX, OS10 …).
    #[default]
    Ios,
    /// Indented blocks negated with `undo`, sections left with `quit` (Huawei VRP, H3C Comware).
    Vrp,
    /// Flat `set …` lines removed with `delete …` (Junos `display set`, VyOS, EdgeOS).
    Set,
    /// RouterOS `/export terse`: one self-contained command per line.
    Routeros,
    /// Unknown grammar: text diff only, no remediation.
    Flat,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub text: String,
    pub children: Vec<Node>,
}

/// A configuration parsed into its block structure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigTree {
    pub roots: Vec<Node>,
}

impl ConfigTree {
    pub fn parse(text: &str, syntax: Syntax) -> ConfigTree {
        match syntax {
            Syntax::Ios | Syntax::Vrp => {
                let mut roots = parse_indented(text, syntax);
                if syntax == Syntax::Ios {
                    // A configuration somebody wrote reads like the one the
                    // device prints, so the two compare as equal.
                    canonical_qos(&mut roots);
                }
                ConfigTree { roots }
            }
            Syntax::Set | Syntax::Routeros | Syntax::Flat => ConfigTree {
                roots: text
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty() && !line.starts_with('#'))
                    .map(|line| Node {
                        text: line.to_string(),
                        children: Vec::new(),
                    })
                    .collect(),
            },
        }
    }

    /// Render back to indented text (one space per level).
    pub fn render(&self) -> String {
        fn walk(nodes: &[Node], depth: usize, out: &mut String) {
            for node in nodes {
                for (index, line) in node.text.lines().enumerate() {
                    // Continuation lines of a banner keep their own layout.
                    if index == 0 {
                        out.push_str(&" ".repeat(depth));
                    }
                    out.push_str(line);
                    out.push('\n');
                }
                walk(&node.children, depth + 1, out);
            }
        }
        let mut out = String::new();
        walk(&self.roots, 0, &mut out);
        out
    }

    /// The node at `path` (parent lines, outermost first), if present.
    pub fn find(&self, path: &[&str]) -> Option<&Node> {
        let mut nodes = &self.roots;
        let mut found = None;
        for segment in path {
            let node = nodes.iter().find(|node| node.text == *segment)?;
            nodes = &node.children;
            found = Some(node);
        }
        found
    }

    /// Top-level sections whose first line matches `pattern`, e.g. every `interface …` block.
    pub fn sections(&self, pattern: &Regex) -> Vec<&Node> {
        self.roots
            .iter()
            .filter(|node| pattern.is_match(&node.text))
            .collect()
    }
}

static BANNER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^banner\s+\S+\s+(\^C|\S)").expect("banner pattern"));

fn parse_indented(text: &str, syntax: Syntax) -> Vec<Node> {
    let comment = if syntax == Syntax::Vrp { '#' } else { '!' };
    // (indent, node) stack of open blocks.
    let mut roots: Vec<Node> = Vec::new();
    let mut stack: Vec<(usize, Node)> = Vec::new();

    fn close(stack: &mut Vec<(usize, Node)>, roots: &mut Vec<Node>, down_to: usize) {
        while stack.last().is_some_and(|(indent, _)| *indent >= down_to) {
            let Some((_, node)) = stack.pop() else { break };
            match stack.last_mut() {
                Some((_, parent)) => parent.children.push(node),
                None => roots.push(node),
            }
        }
    }

    let mut lines = text.lines().peekable();
    while let Some(raw) = lines.next() {
        let line = raw.trim_end();
        let content = line.trim_start();
        if content.is_empty() || content.starts_with(comment) || is_noise(content, syntax) {
            continue;
        }
        let indent = line.len() - content.len();
        let mut text = content.to_string();

        // A banner is one opaque value that may span lines up to its delimiter.
        if syntax == Syntax::Ios
            && let Some(delimiter) = BANNER.captures(content).and_then(|caps| caps.get(1))
        {
            let after = &content[delimiter.end()..];
            if !after.contains(delimiter.as_str()) {
                for next in lines.by_ref() {
                    text.push('\n');
                    text.push_str(next.trim_end());
                    if next.contains(delimiter.as_str()) {
                        break;
                    }
                }
            }
        }

        close(&mut stack, &mut roots, indent);
        stack.push((
            indent,
            Node {
                text,
                children: Vec::new(),
            },
        ));
    }
    close(&mut stack, &mut roots, 0);
    roots
}

/// Lines that are part of the display, not of the configuration.
fn is_noise(line: &str, syntax: Syntax) -> bool {
    match syntax {
        Syntax::Ios => {
            line == "end"
                || line.starts_with("Building configuration")
                || line.starts_with("Current configuration")
                || line.starts_with("Last configuration change")
                || line.starts_with("NVRAM config last updated")
        }
        Syntax::Vrp => line == "return",
        _ => false,
    }
}

/// The commands that turn one configuration into another.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remediation {
    /// Lines to send in configuration mode, in order.
    pub commands: Vec<String>,
    /// Things a human should look at before applying.
    pub warnings: Vec<String>,
    /// False when the grammar is unknown and no commands could be derived.
    pub supported: bool,
}

impl Remediation {
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

/// Compute the commands that transform `running` into `intended`.
pub fn remediate(running: &str, intended: &str, syntax: Syntax) -> Remediation {
    match syntax {
        Syntax::Ios | Syntax::Vrp => {
            let grammar = Grammar::for_syntax(syntax);
            let mut running = ConfigTree::parse(running, syntax);
            let intended = ConfigTree::parse(intended, syntax);
            let mut out = Output { grammar: &grammar, commands: Vec::new(), warnings: Vec::new(), open: Vec::new() };
            if syntax == Syntax::Ios {
                // What the device refuses to remove while it is in use is
                // taken off and removed first, in an order it accepts; the
                // difference that is left is then worked out from there.
                let teardown = mqc_teardown(&running.roots, &intended.roots, &mut out.warnings);
                if !teardown.is_empty() {
                    running.apply(&teardown, syntax);
                    out.commands.extend(teardown);
                }
            }
            diff_level(&running.roots, &intended.roots, &mut Vec::new(), &mut out);
            out.leave_to(0);
            Remediation { commands: out.commands, warnings: out.warnings, supported: true }
        }
        Syntax::Set => remediate_set(running, intended),
        Syntax::Routeros => remediate_routeros(running, intended),
        Syntax::Flat => Remediation {
            commands: Vec::new(),
            warnings: vec![
                "This platform's configuration grammar is not modelled, so the change cannot be derived automatically. \
                 Provide the commands to send instead."
                    .into(),
            ],
            supported: false,
        },
    }
}

/// How one line is undone.
enum Negation {
    Command(String),
    /// The line cannot (or must not) be removed; say why.
    Skip(String),
}

struct Grammar {
    syntax: Syntax,
    negate_word: &'static str,
    exit_word: &'static str,
    /// (pattern, replacement) — first match wins; an empty replacement means "cannot be removed".
    negations: Vec<(Regex, &'static str)>,
    /// Commands where a new value replaces the old one.
    replace_style: Vec<Regex>,
    /// Sections whose child order matters.
    ordered_sections: Vec<Regex>,
}

fn compile(patterns: &[&str]) -> Vec<Regex> {
    patterns
        .iter()
        .map(|pattern| Regex::new(pattern).expect("built-in grammar pattern"))
        .collect()
}

impl Grammar {
    fn for_syntax(syntax: Syntax) -> Grammar {
        match syntax {
            Syntax::Vrp => Grammar {
                syntax,
                negate_word: "undo",
                exit_word: "quit",
                negations: [
                    (r"^sysname ", ""),
                    (r"^(interface (?:LoopBack|Vlanif|Tunnel|Eth-Trunk|Bridge-Aggregation|Vlan-interface|NULL)\S*)", "undo $1"),
                    (r"^interface ", ""),
                    (r"^(local-user \S+)", "undo $1"),
                    (r"^description ", "undo description"),
                    (r"^(vlan \d+)$", "undo $1"),
                    (r"^(vlan batch) ", "undo $0"),
                    (r"^(ip address \S+ \S+)", "undo $1"),
                    (r"^(port link-type) ", "undo $1"),
                    (r"^(port default vlan) ", "undo $1"),
                    (r"^(port trunk pvid vlan)", "undo $1"),
                    (r"^user-interface ", ""),
                    (r"^aaa$", ""),
                ]
                .into_iter()
                .map(|(pattern, replacement)| (Regex::new(pattern).expect("vrp negation pattern"), replacement))
                .collect(),
                replace_style: compile(&[
                    r"^sysname ",
                    r"^description ",
                    r"^(ip address) \S+ \S+$",
                    r"^port link-type ",
                    r"^port default vlan ",
                    r"^port trunk pvid vlan ",
                    r"^speed ",
                    r"^duplex ",
                    r"^mtu ",
                    r"^name ",
                    r"^stp mode ",
                    r"^stp priority ",
                    r"^ospf cost ",
                    r"^router id ",
                    r"^clock timezone ",
                    r"^idle-timeout ",
                    r"^authentication-mode ",
                ]),
                ordered_sections: compile(&[r"^acl "]),
            },
            _ => Grammar {
                syntax,
                negate_word: "no",
                exit_word: "exit",
                negations: [
                    (r"^hostname ", ""),
                    (r"^version ", ""),
                    (r"^boot-(?:start|end)-marker", ""),
                    (r"^line (?:con|vty|aux|console) ", ""),
                    (r"^crypto key ", ""),
                    (r"^crypto pki ", ""),
                    (
                        r"^(interface (?:Loopback|Vlan|Tunnel|Port-channel|BVI|Dialer|Virtual-Template|nve|Bundle-Ether|port-channel|loopback|vlan)\S*)$",
                        "no $1",
                    ),
                    // Sub-interfaces are created by configuration, so they can be removed.
                    (r"^(interface \S+\.\d+)$", "no $1"),
                    (r"^interface (\S+)$", "default interface $1"),
                    (r"^(username \S+)\b.*", "no $1"),
                    (r"^(enable (?:secret|password))\b.*", "no $1"),
                    (r"^(banner \S+)\b[\s\S]*", "no $1"),
                    (r"^(snmp-server community \S+)\b.*", "no $1"),
                    (r"^description\b.*", "no description"),
                    (r"^(spanning-tree vlan \S+ priority) .*", "no $1"),
                    (r"^switchport trunk allowed vlan .*", "no switchport trunk allowed vlan"),
                    (r"^(switchport access vlan) .*", "no $1"),
                    (r"^(switchport trunk native vlan) .*", "no $1"),
                    (r"^(switchport mode) .*", "no $1"),
                    (r"^(channel-group) .*", "no $1"),
                    (r"^(encapsulation dot1[qQ]) .*", "no $1"),
                    (r"^(ip ospf cost) .*", "no $1"),
                    (r"^(speed|duplex|mtu|bandwidth) .*", "no $1"),
                    (r"^(exec-timeout) .*", "no $1"),
                    // Sequence-numbered ACL entries are removed by number.
                    (r"^(\d+) (?:permit|deny|remark)\b.*", "no $1"),
                    (r"^(name) .*", "no $1"),
                    // A class of a policy map holds one of each of these, so
                    // the keyword alone names the one to take away.
                    (r"^(priority)(?: .*)?$", "no $1"),
                    (r"^(police)(?: .*)?$", "no $1"),
                    (r"^(shape (?:average|peak))\b.*", "no $1"),
                    (r"^(set (?:ip )?dscp)\b.*", "no $1"),
                ]
                .into_iter()
                .map(|(pattern, replacement)| (Regex::new(pattern).expect("ios negation pattern"), replacement))
                .collect(),
                replace_style: compile(&[
                    r"^hostname ",
                    r"^(ip address) (?:dhcp\b.*|\S+ \S+)$",
                    r"^ip access-group \S+ (in|out)$",
                    r"^description ",
                    r"^switchport mode ",
                    r"^switchport access vlan ",
                    r"^switchport trunk native vlan ",
                    r"^switchport trunk encapsulation ",
                    r"^switchport voice vlan ",
                    r"^speed ",
                    r"^duplex ",
                    r"^mtu ",
                    r"^ip mtu ",
                    r"^bandwidth ",
                    r"^delay ",
                    r"^encapsulation ",
                    r"^channel-group ",
                    r"^ip ospf cost ",
                    r"^ip ospf priority ",
                    r"^ip ospf hello-interval ",
                    r"^ip ospf dead-interval ",
                    r"^ip ospf network ",
                    r"^spanning-tree mode ",
                    r"^spanning-tree vlan \S+ priority ",
                    r"^spanning-tree cost ",
                    r"^spanning-tree port-priority ",
                    r"^standby \d+ priority ",
                    r"^standby \d+ ip ",
                    r"^standby version ",
                    r"^router-id ",
                    r"^enable secret ",
                    r"^enable password ",
                    r"^username \S+ ",
                    r"^ip default-gateway ",
                    r"^ip domain[- ]name ",
                    r"^clock timezone ",
                    r"^snmp-server location ",
                    r"^snmp-server contact ",
                    r"^logging buffered ",
                    r"^logging trap ",
                    r"^exec-timeout ",
                    r"^transport input ",
                    r"^password ",
                    r"^vtp mode ",
                    r"^vtp domain ",
                    r"^name ",
                    r"^banner \S+ ",
                    r"^ntp source ",
                    r"^ip ssh version ",
                    r"^service timestamps (?:debug|log) ",
                    r"^default-router ",
                    r"^lease ",
                    r"^domain-name ",
                    r"^auto-cost reference-bandwidth ",
                    r"^maximum-paths ",
                    r"^ip tftp source-interface ",
                    r"^ip http client source-interface ",
                    r"^login",
                    // A class of a policy map: one priority (with or without a
                    // rate), one policer, one shaper, one marking and one
                    // child policy each; typing another replaces it, on IOS as
                    // in the simulator. `priority-queue` is another command.
                    r"^(priority)(?: |$)",
                    r"^(police)(?: |$)",
                    r"^(shape) (?:average|peak)\b",
                    r"^(set) (?:ip )?dscp ",
                    // `service-policy CHILD`; an interface's `service-policy
                    // input|output NAME` has three words and is not replaced
                    // but refused by the device while another is attached.
                    r"^(service-policy) \S+$",
                ]),
                ordered_sections: compile(&[r"^ip(?:v6)? access-list ", r"^access-list ", r"^class-map ", r"^policy-map "]),
            },
        }
    }

    fn negate(&self, line: &str) -> Negation {
        if let Some(positive) = line
            .strip_prefix(self.negate_word)
            .and_then(|rest| rest.strip_prefix(' '))
        {
            return Negation::Command(positive.to_string());
        }
        for (pattern, replacement) in &self.negations {
            if let Some(caps) = pattern.captures(line) {
                if replacement.is_empty() {
                    return Negation::Skip(format!(
                        "`{}` cannot be removed automatically",
                        first_line(line)
                    ));
                }
                let mut expanded = String::new();
                caps.expand(replacement, &mut expanded);
                return Negation::Command(expanded);
            }
        }
        Negation::Command(format!("{} {}", self.negate_word, first_line(line)))
    }

    /// The "slot" a replace-style command occupies: which rule matched, and
    /// the part of the line that identifies the setting (the rule's capture
    /// group if it has one, otherwise everything it matched) — so that
    /// `spanning-tree vlan 10 priority …` and `spanning-tree vlan 20 priority …`
    /// are different slots, while two `ip address` lines are the same one.
    fn slot<'a>(&self, line: &'a str) -> Option<(usize, &'a str)> {
        self.replace_style
            .iter()
            .enumerate()
            .find_map(|(index, pattern)| {
                let caps = pattern.captures(line)?;
                let key = caps.get(1).or_else(|| caps.get(0))?;
                Some((index, key.as_str()))
            })
    }

    fn is_ordered(&self, parent: &str) -> bool {
        self.ordered_sections
            .iter()
            .any(|pattern| pattern.is_match(parent))
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

struct Output<'g> {
    grammar: &'g Grammar,
    commands: Vec<String>,
    warnings: Vec<String>,
    /// The section path currently entered on the device.
    open: Vec<String>,
}

impl Output<'_> {
    /// Leave sections until at most `depth` remain open.
    fn leave_to(&mut self, depth: usize) {
        while self.open.len() > depth {
            self.open.pop();
            self.commands.push(self.grammar.exit_word.to_string());
        }
    }

    /// Make `path` the current section, entering and leaving blocks as needed.
    fn enter(&mut self, path: &[String]) {
        let shared = self
            .open
            .iter()
            .zip(path)
            .take_while(|(open, wanted)| open == wanted)
            .count();
        self.leave_to(shared);
        for segment in &path[shared..] {
            self.commands.push(segment.clone());
            self.open.push(segment.clone());
        }
    }

    fn emit(&mut self, path: &[String], command: String) {
        self.enter(path);
        self.commands.push(command);
    }

    /// Add a whole subtree.
    fn add(&mut self, path: &mut Vec<String>, node: &Node) {
        // A section with nothing in it is still entered on the device (a
        // class of a policy map that only classifies, an empty port), so what
        // follows has to leave it: it is entered here too. Below the top
        // level only a nested section is one (`ip vrf forwarding X` in an
        // interface reads like a section header and is a setting).
        let syntax = self.grammar.syntax;
        let enters = opens_section(&node.text, syntax)
            && (path.is_empty() || opens_nested(&node.text, syntax));
        if node.children.is_empty() && !enters {
            self.emit(path, node.text.clone());
            return;
        }
        path.push(node.text.clone());
        self.enter(path);
        for child in &node.children {
            self.add(path, child);
        }
        path.pop();
    }

    fn remove(&mut self, path: &[String], node: &Node) {
        match self.grammar.negate(&node.text) {
            Negation::Command(command) => self.emit(path, command),
            Negation::Skip(reason) => self.warnings.push(reason),
        }
    }
}

fn diff_level(running: &[Node], intended: &[Node], path: &mut Vec<String>, out: &mut Output<'_>) {
    let ios = out.grammar.syntax == Syntax::Ios;
    // Inside a class of a policy map the actions exclude each other (a
    // priority class cannot be given a bandwidth or a shaper, nor the other
    // way round), so what goes is taken away before what comes is added.
    // Everywhere else additions come first — make before break: replacing a
    // default route, a logging host or an admin user by removing the old
    // entry first leaves a window with neither, and if that window cuts the
    // management session the second half never arrives. That order fails
    // safe: at worst the device rejects an addition that conflicts with the
    // entry about to be removed.
    if ios && is_policy_class(path) {
        remove_level(running, intended, path, out, false);
        add_level(running, intended, path, out, false);
        return;
    }
    // The allowed-VLAN list is a set spread over several lines; patch it as a set.
    let trunk_handled = diff_allowed_vlans(running, intended, path, out);
    add_level(running, intended, path, out, trunk_handled);
    remove_level(running, intended, path, out, trunk_handled);
}

/// Add what is missing and descend into what both sides have.
fn add_level(
    running: &[Node],
    intended: &[Node],
    path: &mut Vec<String>,
    out: &mut Output<'_>,
    trunk_handled: bool,
) {
    let is_trunk_line = |text: &str| trunk_handled && text.starts_with(TRUNK_ALLOWED);
    // A class map before the policy maps that name it, a policy map before
    // the classes and interfaces that attach it: the device refuses a name it
    // has not seen yet.
    let order: Vec<&Node> = if out.grammar.syntax == Syntax::Ios && path.is_empty() {
        qos_first(intended)
    } else {
        intended.iter().collect()
    };
    for node in order {
        if is_trunk_line(&node.text) {
            continue;
        }
        match running.iter().find(|other| other.text == node.text) {
            None => out.add(path, node),
            Some(existing) if existing.children == node.children => {}
            Some(existing) => {
                let patchable = if out.grammar.syntax == Syntax::Ios
                    && path.is_empty()
                    && policy_map_name(&node.text).is_some()
                {
                    policy_patchable(existing, node)
                } else {
                    is_patchable_in_place(existing, node)
                };
                if out.grammar.is_ordered(&node.text) && !patchable {
                    out.warnings.push(format!(
                        "`{}` is order-sensitive and is rebuilt from scratch; it is briefly absent while the new entries are applied",
                        first_line(&node.text)
                    ));
                    out.remove(path, existing);
                    out.add(path, node);
                } else {
                    path.push(node.text.clone());
                    diff_level(&existing.children, &node.children, path, out);
                    path.pop();
                }
            }
        }
    }
}

/// Remove what should no longer be there — unless a replace-style command in
/// the intended config has already overwritten it.
fn remove_level(
    running: &[Node],
    intended: &[Node],
    path: &mut Vec<String>,
    out: &mut Output<'_>,
    trunk_handled: bool,
) {
    let is_trunk_line = |text: &str| trunk_handled && text.starts_with(TRUNK_ALLOWED);
    for node in running {
        if is_trunk_line(&node.text) || intended.iter().any(|other| other.text == node.text) {
            continue;
        }
        let replaced = out.grammar.slot(&node.text).is_some_and(|slot| {
            intended
                .iter()
                .any(|other| out.grammar.slot(&other.text) == Some(slot))
        });
        if !replaced {
            out.remove(path, node);
        }
    }
}

/// An ordered section can be edited in place when every entry carries its own
/// sequence number, or when the new version only appends entries.
fn is_patchable_in_place(existing: &Node, intended: &Node) -> bool {
    let numbered = |node: &Node| {
        node.children.iter().all(|child| {
            child
                .text
                .split_whitespace()
                .next()
                .is_some_and(|word| word.parse::<u32>().is_ok())
        })
    };
    if numbered(existing) && numbered(intended) {
        return true;
    }
    intended.children.len() >= existing.children.len()
        && existing
            .children
            .iter()
            .zip(&intended.children)
            .all(|(old, new)| old == new)
}

const TRUNK_ALLOWED: &str = "switchport trunk allowed vlan ";

/// Patch `switchport trunk allowed vlan` with `add`/`remove` instead of
/// restating the list. Restating it on a live trunk drops every VLAN that is
/// not in the new statement for the instant between the two commands, and a
/// mistyped list takes the uplink down outright.
fn diff_allowed_vlans(
    running: &[Node],
    intended: &[Node],
    path: &[String],
    out: &mut Output<'_>,
) -> bool {
    let collect = |nodes: &[Node]| -> Option<BTreeSet<u16>> {
        let mut set = BTreeSet::new();
        let mut seen = false;
        for node in nodes {
            let Some(list) = node.text.strip_prefix(TRUNK_ALLOWED) else {
                continue;
            };
            seen = true;
            let list = list.strip_prefix("add ").unwrap_or(list);
            match list.trim() {
                "all" => return None,
                "none" => {}
                ranges => set.extend(parse_vlan_ranges(ranges)?),
            }
        }
        seen.then_some(set)
    };
    let (Some(old), Some(new)) = (collect(running), collect(intended)) else {
        return false;
    };
    let added: Vec<u16> = new.difference(&old).copied().collect();
    let removed: Vec<u16> = old.difference(&new).copied().collect();
    if !added.is_empty() {
        out.emit(
            path,
            format!("{TRUNK_ALLOWED}add {}", format_vlan_ranges(&added)),
        );
    }
    if !removed.is_empty() {
        out.emit(
            path,
            format!("{TRUNK_ALLOWED}remove {}", format_vlan_ranges(&removed)),
        );
    }
    true
}

/// `"10,20,30-32"` → `{10, 20, 30, 31, 32}`.
pub fn parse_vlan_ranges(text: &str) -> Option<BTreeSet<u16>> {
    let mut set = BTreeSet::new();
    for part in text
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        match part.split_once('-') {
            Some((low, high)) => {
                let (low, high) = (
                    low.trim().parse::<u16>().ok()?,
                    high.trim().parse::<u16>().ok()?,
                );
                if low > high || high > 4094 {
                    return None;
                }
                set.extend(low..=high);
            }
            None => {
                set.insert(part.parse::<u16>().ok().filter(|vlan| *vlan <= 4094)?);
            }
        }
    }
    Some(set)
}

/// `[10, 20, 30, 31, 32]` → `"10,20,30-32"`.
pub fn format_vlan_ranges(vlans: &[u16]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut iter = vlans.iter().copied().peekable();
    while let Some(start) = iter.next() {
        let mut end = start;
        while iter.peek() == Some(&(end + 1)) {
            end += 1;
            iter.next();
        }
        parts.push(if end == start {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
    }
    parts.join(",")
}

fn remediate_set(running: &str, intended: &str) -> Remediation {
    let lines = |text: &str| -> Vec<String> {
        text.lines()
            .map(str::trim)
            .filter(|line| line.starts_with("set "))
            .map(str::to_string)
            .collect()
    };
    let (old, new) = (lines(running), lines(intended));
    let mut commands: Vec<String> = old
        .iter()
        .filter(|line| !new.contains(line))
        .map(|line| format!("delete {}", &line["set ".len()..]))
        .collect();
    commands.extend(new.iter().filter(|line| !old.contains(line)).cloned());
    Remediation {
        commands,
        warnings: Vec::new(),
        supported: true,
    }
}

static ROS_ADD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(/\S.*?) add (.+)$").expect("routeros add pattern"));
static ROS_PAIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"([A-Za-z0-9.\-]+)=("(?:[^"\\]|\\.)*"|\S+)"#).expect("routeros pair pattern")
});

fn remediate_routeros(running: &str, intended: &str) -> Remediation {
    let lines = |text: &str| -> Vec<String> {
        text.lines()
            .map(str::trim)
            .filter(|line| line.starts_with('/'))
            .map(str::to_string)
            .collect()
    };
    let (old, new) = (lines(running), lines(intended));
    let mut commands = Vec::new();
    let mut warnings = Vec::new();

    // Make before break, as for the indented grammars.
    commands.extend(new.iter().filter(|line| !old.contains(line)).cloned());

    for line in old.iter().filter(|line| !new.contains(line)) {
        match ROS_ADD.captures(line) {
            Some(caps) => {
                // Identify the entry by its properties; secrets cannot be searched for.
                let filter: Vec<String> = ROS_PAIR
                    .captures_iter(&caps[2])
                    .filter(|pair| !matches!(&pair[1], "password" | "secret" | "passphrase" | "comment" | "private-key"))
                    .map(|pair| format!("{}={}", &pair[1], &pair[2]))
                    .collect();
                if filter.is_empty() {
                    warnings.push(format!("could not derive a removal for `{line}`"));
                } else {
                    commands.push(format!("{} remove [find {}]", &caps[1], filter.join(" ")));
                }
            }
            None => warnings.push(format!(
                "`{line}` is no longer wanted, but a `set` cannot be undone without knowing the default value"
            )),
        }
    }
    if !commands.is_empty() {
        warnings.push(
            "RouterOS remediation is derived line by line: an entry whose properties changed is removed and re-added. \
             Review the order if it touches the address or interface you manage the router through."
                .into(),
        );
    }
    Remediation {
        commands,
        warnings,
        supported: true,
    }
}

// ------------------------------------------------------ applying commands

/// What happened while applying configuration lines to a [`ConfigTree`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyReport {
    /// Things the caller should tell the user about the prediction.
    pub warnings: Vec<String>,
    /// Lines whose effect on the configuration could not be modelled; the
    /// prediction is incomplete where they appear.
    pub unmodelled: Vec<String>,
    /// False when the grammar has no block structure to apply lines to.
    pub supported: bool,
}

/// Does this line open a configuration section (so that what follows is
/// indented under it)?
pub fn opens_section(line: &str, syntax: Syntax) -> bool {
    let line = line.trim();
    match syntax {
        Syntax::Ios => IOS_SECTION.is_match(line) || IOS_NESTED.is_match(line),
        Syntax::Vrp => VRP_SECTION.is_match(line),
        _ => false,
    }
}

/// Does this line only make sense at the top level, so that typing it inside
/// a section is a mistake (or means the section was meant to be left first)?
pub fn is_global(line: &str, syntax: Syntax) -> bool {
    let line = line.trim();
    let line = line
        .strip_prefix("no ")
        .or_else(|| line.strip_prefix("undo "))
        .unwrap_or(line);
    match syntax {
        Syntax::Ios => IOS_GLOBAL.is_match(line) || IOS_SECTION.is_match(line),
        Syntax::Vrp => VRP_GLOBAL.is_match(line) || VRP_SECTION.is_match(line),
        _ => false,
    }
}

/// A section that is opened *inside* another one (`address-family` under
/// `router bgp`), rather than replacing it.
pub fn opens_nested(line: &str, syntax: Syntax) -> bool {
    syntax == Syntax::Ios && IOS_NESTED.is_match(line.trim())
}

static IOS_SECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:interface \S|router \S|line (?:con|vty|aux|console)\b|vlan \d|ip(?:v6)? access-list \S|class-map \S|policy-map \S|route-map \S|vrf definition \S|ip vrf \S|key chain \S|ip dhcp pool \S|crypto map \S|archive$|spanning-tree mst configuration$)",
    )
    .expect("ios section pattern")
});

static IOS_NESTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:address-family \S|class \S|template \S)").expect("ios nested pattern")
});

/// Commands that unambiguously belong at the top level: none of them is also
/// a sub-command of an interface, a line or a routing process.
static IOS_GLOBAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:hostname |sysname |username |enable (?:secret|password)\b|aaa |snmp-server |banner |clock |vtp |access-list |ip route |ipv6 route |ip domain|ip name-server |ip http |ip ssh |ip default-gateway |ip routing\b|ip forward-protocol |spanning-tree mode |spanning-tree vlan |spanning-tree portfast default|lldp run\b|cdp run\b|vlan internal |mac address-table |errdisable |boot system |crypto key |license |service (?:password-encryption|timestamps|compress|sequence|unsupported)\b)",
    )
    .expect("ios global pattern")
});

static VRP_SECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:interface \S|vlan \d|user-interface \S|acl \S|aaa$|ospf \d|bgp \d|isis \d)",
    )
    .expect("vrp section pattern")
});

static VRP_GLOBAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:sysname |vlan batch |ip route-static |snmp-agent |info-center |dns )")
        .expect("vrp global pattern")
});

/// The children at `path`, creating the sections on the way if they are missing.
fn children_at<'a>(roots: &'a mut Vec<Node>, path: &[String]) -> &'a mut Vec<Node> {
    let mut nodes = roots;
    for segment in path {
        let index = match nodes.iter().position(|node| node.text == *segment) {
            Some(index) => index,
            None => {
                nodes.push(Node {
                    text: segment.clone(),
                    children: Vec::new(),
                });
                nodes.len() - 1
            }
        };
        nodes = &mut nodes[index].children;
    }
    nodes
}

/// Add or replace one line among `nodes`, the way a device would: a
/// replace-style command takes over the line that holds the same setting,
/// anything else is appended unless it is already there.
fn set_line(nodes: &mut Vec<Node>, line: &str, grammar: &Grammar) -> bool {
    if let Some(rest) = line.strip_prefix(TRUNK_ALLOWED) {
        return set_allowed_vlans(nodes, rest.trim());
    }
    if let Some(slot) = grammar.slot(line)
        && let Some(position) = nodes
            .iter()
            .position(|node| grammar.slot(&node.text) == Some(slot))
    {
        nodes[position].text = line.to_string();
        return true;
    }
    if !nodes.iter().any(|node| node.text == line) {
        nodes.push(Node {
            text: line.to_string(),
            children: Vec::new(),
        });
    }
    true
}

/// `switchport trunk allowed vlan …` edits one list, wherever the device
/// chooses to print it: `add` and `remove` patch it, a bare list replaces it,
/// and `add` on a trunk that has no list at all (every VLAN allowed) does
/// nothing.
fn set_allowed_vlans(nodes: &mut Vec<Node>, rest: &str) -> bool {
    let had_list = nodes
        .iter()
        .any(|node| node.text.starts_with(TRUNK_ALLOWED));
    let current: BTreeSet<u16> = nodes
        .iter()
        .filter_map(|node| node.text.strip_prefix(TRUNK_ALLOWED))
        .find_map(|list| parse_vlan_ranges(list.trim()))
        .unwrap_or_default();
    let next = if let Some(added) = rest.strip_prefix("add ") {
        if !had_list {
            return true;
        }
        match parse_vlan_ranges(added.trim()) {
            Some(added) => current.union(&added).copied().collect(),
            None => return false,
        }
    } else if let Some(removed) = rest.strip_prefix("remove ") {
        let Some(removed) = parse_vlan_ranges(removed.trim()) else {
            return false;
        };
        let base: BTreeSet<u16> = if had_list {
            current
        } else {
            (1..=4094).collect()
        };
        base.difference(&removed).copied().collect()
    } else if rest == "none" {
        BTreeSet::new()
    } else if rest == "all" {
        nodes.retain(|node| !node.text.starts_with(TRUNK_ALLOWED));
        return true;
    } else {
        match parse_vlan_ranges(rest) {
            Some(list) => list,
            None => return false,
        }
    };
    let list: Vec<u16> = next.into_iter().collect();
    let text = format!("{TRUNK_ALLOWED}{}", format_vlan_ranges(&list));
    let text = text.trim_end().to_string();
    let position = nodes
        .iter()
        .position(|node| node.text.starts_with(TRUNK_ALLOWED));
    nodes.retain(|node| !node.text.starts_with(TRUNK_ALLOWED));
    let node = Node {
        text,
        children: Vec::new(),
    };
    match position {
        Some(position) if position < nodes.len() => nodes.insert(position, node),
        _ => nodes.push(node),
    }
    true
}

impl ConfigTree {
    /// Apply configuration-mode lines to this tree, as the device would.
    ///
    /// This is the other half of [`remediate`]: it predicts what the running
    /// configuration will look like after a list of commands has been sent,
    /// so that a change can be compared with its own prediction afterwards
    /// instead of being taken on trust. Section headers enter blocks, `exit`
    /// and `end` leave them, `no …` removes, replace-style commands take over
    /// the line that holds the same setting, and the allowed-VLAN list of a
    /// trunk is patched as a set.
    ///
    /// Lines whose effect cannot be modelled are listed in the report rather
    /// than guessed at.
    pub fn apply(&mut self, commands: &[String], syntax: Syntax) -> ApplyReport {
        let mut report = ApplyReport {
            supported: matches!(syntax, Syntax::Ios | Syntax::Vrp),
            ..ApplyReport::default()
        };
        if !report.supported {
            report.warnings.push(
                "This platform's configuration grammar is not modelled, so the result of these commands cannot be predicted."
                    .into(),
            );
            return report;
        }
        let grammar = Grammar::for_syntax(syntax);
        let mut path: Vec<String> = Vec::new();
        for raw in commands {
            self.apply_line(&mut path, raw, &grammar, &mut report);
        }
        report
    }

    /// One configuration-mode line; `path` is the section the device is in,
    /// before and after.
    fn apply_line(
        &mut self,
        path: &mut Vec<String>,
        raw: &str,
        grammar: &Grammar,
        report: &mut ApplyReport,
    ) {
        let syntax = grammar.syntax;
        let comment = if syntax == Syntax::Vrp { '#' } else { '!' };
        let line = raw.trim();
        if line.is_empty() || line.starts_with(comment) {
            return;
        }
        if line == grammar.exit_word {
            path.pop();
            return;
        }
        if line == "end" || line == "return" {
            path.clear();
            return;
        }
        if let Some(target) = line
            .strip_prefix(grammar.negate_word)
            .and_then(|rest| rest.strip_prefix(' '))
        {
            let target = target.trim();
            if syntax == Syntax::Ios && remove_qos(&mut self.roots, path, target) {
                return;
            }
            remove_line(&mut self.roots, path, target);
            return;
        }
        if let Some(name) = line.strip_prefix("default interface ") {
            let section = format!("interface {}", name.trim());
            match self
                .roots
                .iter_mut()
                .find(|node| node.text.eq_ignore_ascii_case(&section))
            {
                Some(node) => node.children.clear(),
                None => self.roots.push(Node {
                    text: section.clone(),
                    children: Vec::new(),
                }),
            }
            path.clear();
            report.warnings.push(format!(
                "`{line}` resets the port: the prediction assumes it removes every configuration line of `{section}`, and the device may also put back lines that belong to its defaults."
            ));
            return;
        }
        if line.to_ascii_lowercase().starts_with("interface range ") {
            report.unmodelled.push(line.to_string());
            return;
        }
        if opens_section(line, syntax) {
            enter_section(&mut self.roots, path, line, syntax);
            return;
        }
        let line = if syntax == Syntax::Ios {
            spelled_in(path, line)
        } else {
            line.to_string()
        };
        let nodes = children_at(&mut self.roots, path);
        if !set_line(nodes, &line, grammar) {
            report.unmodelled.push(raw.trim().to_string());
        }
    }
}

/// Open the section `line` starts, the way the device moves between modes.
fn enter_section(roots: &mut Vec<Node>, path: &mut Vec<String>, line: &str, syntax: Syntax) {
    if opens_nested(line, syntax) && !path.is_empty() {
        // A section of the kind that is open already is its sibling: `class
        // VIDEO` typed in class VOICE of a policy map moves to class VIDEO,
        // because IOS reads a command its mode does not have in the mode
        // above. (`address-family` under `router bgp` alike.)
        if path
            .last()
            .is_some_and(|open| opens_nested(open, syntax) && first_word(open) == first_word(line))
        {
            path.pop();
        }
        path.push(line.to_string());
        // IOS keeps `class class-default` the last class of a policy map,
        // wherever a new class is typed.
        if syntax == Syntax::Ios
            && path.len() == 2
            && policy_map_name(&path[0]).is_some()
            && class_of(line).is_some_and(|name| name != CLASS_DEFAULT)
        {
            let classes = children_at(roots, &path[..1]);
            if !classes.iter().any(|node| node.text == line) {
                let at = classes
                    .iter()
                    .position(|node| class_of(&node.text) == Some(CLASS_DEFAULT))
                    .unwrap_or(classes.len());
                classes.insert(
                    at,
                    Node {
                        text: line.to_string(),
                        children: Vec::new(),
                    },
                );
            }
        }
        children_at(roots, path);
        return;
    }
    path.clear();
    let mut header = line.to_string();
    if syntax == Syntax::Ios
        && let Some(name) = class_map_name(line)
    {
        // A class map is one per name: naming it again with a match type
        // changes the type and keeps its statements, naming it without one
        // enters it as it is (a new one is `match-all`).
        let typed = class_map_header(line).unwrap_or_else(|| line.to_string());
        let explicit = line.split_whitespace().count() == 3;
        match roots
            .iter_mut()
            .find(|node| class_map_name(&node.text) == Some(name))
        {
            Some(existing) => {
                if explicit {
                    existing.text = typed;
                }
                header = existing.text.clone();
            }
            None => header = typed,
        }
    }
    path.push(header);
    children_at(roots, path);
}

fn first_word(line: &str) -> &str {
    line.split_whitespace().next().unwrap_or("")
}

/// `no …` / `undo …`: inside the open section first, as the device does, then
/// at the top level.
fn remove_line(roots: &mut Vec<Node>, path: &mut Vec<String>, target: &str) {
    let matches = |text: &str| text == target || text.starts_with(&format!("{target} "));
    if !path.is_empty() {
        let nodes = children_at(roots, path);
        if target.starts_with(TRUNK_ALLOWED.trim_end()) {
            nodes.retain(|node| !node.text.starts_with(TRUNK_ALLOWED));
            return;
        }
        let before = nodes.len();
        nodes.retain(|node| !matches(&node.text));
        if nodes.len() != before || target == "shutdown" {
            return;
        }
    }
    let before = roots.len();
    roots.retain(|node| !matches(&node.text));
    if roots.len() != before {
        path.clear();
    }
}

// ------------------------------------------------------------ modular QoS
//
// Class maps, policy maps and the service policies that attach them (the
// modular QoS command line of the IOS family). Three things about it decide
// whether a plan can be applied in the order it gives and whether a device
// that did what was asked matches its prediction:
//
// * what depends on what: a policy map uses the class maps its classes name,
//   an interface (or a class of a parent policy) uses the policy it attaches.
//   IOS — and the simulator — refuses to remove what is in use, to name a
//   class map or policy map that does not exist yet, and to attach a second
//   policy in one direction;
// * a class holds one priority, bandwidth, policer, shaper, marking and child
//   policy each: typing another replaces it (the replace-style rules above);
// * a marking is kept as a code point and printed back by name where it has
//   one, the values of a `match` statement in ascending order.
//
// Only the plain forms are read here (`class-map [match-any|match-all] NAME`,
// `policy-map NAME`, `service-policy input|output NAME`); `… type …` maps
// (inspection, control plane) are left to the generic rules.

/// The class a policy map always ends with.
const CLASS_DEFAULT: &str = "class-default";

/// `class-map [match-any | match-all] NAME` → `NAME`.
fn class_map_name(text: &str) -> Option<&str> {
    let words: Vec<&str> = text.split_whitespace().collect();
    match words.as_slice() {
        ["class-map", "match-any" | "match-all", name] => Some(name),
        ["class-map", name] if !matches!(*name, "type" | "match-any" | "match-all") => Some(name),
        _ => None,
    }
}

/// The header a device prints a class map with: `match-all` when no type
/// was typed.
fn class_map_header(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    match words.as_slice() {
        ["class-map", kind @ ("match-any" | "match-all"), name] => {
            Some(format!("class-map {kind} {name}"))
        }
        ["class-map", name] if !matches!(*name, "type" | "match-any" | "match-all") => {
            Some(format!("class-map match-all {name}"))
        }
        _ => None,
    }
}

/// `policy-map NAME` → `NAME`.
fn policy_map_name(text: &str) -> Option<&str> {
    let words: Vec<&str> = text.split_whitespace().collect();
    match words.as_slice() {
        ["policy-map", name] if *name != "type" => Some(name),
        _ => None,
    }
}

/// `class NAME` → `NAME`.
fn class_of(text: &str) -> Option<&str> {
    let words: Vec<&str> = text.split_whitespace().collect();
    match words.as_slice() {
        ["class", name] => Some(name),
        _ => None,
    }
}

/// `service-policy input|output NAME` → (`input`|`output`, `NAME`): a policy
/// attached to the section the line is in (an interface, the control plane).
fn attachment(text: &str) -> Option<(&str, &str)> {
    let words: Vec<&str> = text.split_whitespace().collect();
    match words.as_slice() {
        ["service-policy", direction @ ("input" | "output"), name] => Some((direction, name)),
        _ => None,
    }
}

/// `service-policy NAME` in a class: the policy applied to that class's traffic.
fn child_policy(text: &str) -> Option<&str> {
    let words: Vec<&str> = text.split_whitespace().collect();
    match words.as_slice() {
        ["service-policy", name] if !matches!(*name, "input" | "output" | "type") => Some(name),
        _ => None,
    }
}

/// Is `path` a class of a policy map?
fn is_policy_class(path: &[String]) -> bool {
    path.len() == 2 && policy_map_name(&path[0]).is_some() && class_of(&path[1]).is_some()
}

fn find_policy<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Node> {
    nodes
        .iter()
        .find(|node| policy_map_name(&node.text) == Some(name))
}

fn find_class_map<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Node> {
    nodes
        .iter()
        .find(|node| class_map_name(&node.text) == Some(name))
}

/// The class names of a policy map, in order.
fn classes_of(policy: &Node) -> impl Iterator<Item = &str> {
    policy
        .children
        .iter()
        .filter_map(|class| class_of(&class.text))
}

/// The policies applied inside the classes of a policy map.
fn child_policies(policy: &Node) -> impl Iterator<Item = &str> {
    policy
        .children
        .iter()
        .flat_map(|class| class.children.iter())
        .filter_map(|action| child_policy(&action.text))
}

/// The top-level sections in an order a device takes them in: class maps,
/// then policy maps (one applied inside another before it), then the rest.
fn qos_first(nodes: &[Node]) -> Vec<&Node> {
    fn place<'a>(node: &'a Node, policies: &[&'a Node], placed: &mut Vec<&'a Node>, depth: usize) {
        if depth > 8 || placed.iter().any(|done| std::ptr::eq(*done, node)) {
            return;
        }
        for child in child_policies(node) {
            if let Some(found) = policies
                .iter()
                .find(|policy| policy_map_name(&policy.text) == Some(child))
            {
                place(found, policies, placed, depth + 1);
            }
        }
        if !placed.iter().any(|done| std::ptr::eq(*done, node)) {
            placed.push(node);
        }
    }
    let mut out: Vec<&Node> = nodes
        .iter()
        .filter(|node| node.text.starts_with("class-map "))
        .collect();
    let policies: Vec<&Node> = nodes
        .iter()
        .filter(|node| node.text.starts_with("policy-map "))
        .collect();
    let mut placed = Vec::new();
    for policy in &policies {
        place(policy, &policies, &mut placed, 0);
    }
    out.extend(placed);
    out.extend(nodes.iter().filter(|node| {
        !node.text.starts_with("class-map ") && !node.text.starts_with("policy-map ")
    }));
    out
}

/// A policy map can be edited in place when its classes keep their order:
/// the device adds a new class at the end (before `class-default`), so a
/// class that is new may only follow every class that stays. Inside a class
/// the actions are patched: a new value replaces the old one.
fn policy_patchable(existing: &Node, intended: &Node) -> bool {
    let classes = |node: &Node| -> Vec<String> {
        classes_of(node)
            .filter(|name| *name != CLASS_DEFAULT)
            .map(str::to_string)
            .collect()
    };
    let (old, new) = (classes(existing), classes(intended));
    let staying = |list: &[String], other: &[String]| -> Vec<String> {
        list.iter()
            .filter(|name| other.contains(name))
            .cloned()
            .collect()
    };
    if staying(&old, &new) != staying(&new, &old) {
        return false;
    }
    let last_kept = new.iter().rposition(|name| old.contains(name));
    let first_added = new.iter().position(|name| !old.contains(name));
    !matches!((last_kept, first_added), (Some(kept), Some(added)) if added < kept)
}

/// The named code points of RFC 2474, 2597 and 3246, as the IOS family
/// spells them in a configuration (and the simulator, which prints them back
/// the same way).
const DSCP_NAMES: [(u8, &str); 21] = [
    (0, "default"),
    (8, "cs1"),
    (10, "af11"),
    (12, "af12"),
    (14, "af13"),
    (16, "cs2"),
    (18, "af21"),
    (20, "af22"),
    (22, "af23"),
    (24, "cs3"),
    (26, "af31"),
    (28, "af32"),
    (30, "af33"),
    (32, "cs4"),
    (34, "af41"),
    (36, "af42"),
    (38, "af43"),
    (40, "cs5"),
    (46, "ef"),
    (48, "cs6"),
    (56, "cs7"),
];

/// A marking as typed: a name in any case, or a number from 0 to 63.
fn dscp_value(text: &str) -> Option<u8> {
    if let Some((value, _)) = DSCP_NAMES
        .iter()
        .find(|(_, name)| name.eq_ignore_ascii_case(text))
    {
        return Some(*value);
    }
    let digits = !text.is_empty() && text.len() <= 2 && text.bytes().all(|b| b.is_ascii_digit());
    digits
        .then(|| text.parse::<u8>().ok())
        .flatten()
        .filter(|value| *value <= 63)
}

fn dscp_text(value: u8) -> String {
    DSCP_NAMES
        .iter()
        .find(|(code, _)| *code == value)
        .map_or_else(|| value.to_string(), |(_, name)| (*name).to_string())
}

/// A `match [ip] dscp …` statement: whether it says `ip`, and its code
/// points in ascending order. `None` for any other line, and for one the
/// device would refuse (more than eight values, a value it does not know).
fn dscp_statement(line: &str) -> Option<(bool, Vec<u8>)> {
    let mut words = line.split_whitespace();
    if words.next()? != "match" {
        return None;
    }
    let mut word = words.next()?;
    let ip = word == "ip";
    if ip {
        word = words.next()?;
    }
    if word != "dscp" {
        return None;
    }
    let mut values = words.map(dscp_value).collect::<Option<Vec<u8>>>()?;
    if values.is_empty() || values.len() > 8 {
        return None;
    }
    values.sort_unstable();
    values.dedup();
    Some((ip, values))
}

fn dscp_statement_text(ip: bool, values: &[u8]) -> String {
    let names: Vec<String> = values.iter().map(|value| dscp_text(*value)).collect();
    format!(
        "match {}dscp {}",
        if ip { "ip " } else { "" },
        names.join(" ")
    )
}

/// `set [ip] dscp V` the way the device prints it back.
fn canonical_set(line: &str) -> Option<String> {
    let mut words = line.split_whitespace();
    if words.next()? != "set" {
        return None;
    }
    let mut word = words.next()?;
    let ip = word == "ip";
    if ip {
        word = words.next()?;
    }
    if word != "dscp" {
        return None;
    }
    let value = dscp_value(words.next()?)?;
    if words.next().is_some() {
        return None;
    }
    Some(format!(
        "set {}dscp {}",
        if ip { "ip " } else { "" },
        dscp_text(value)
    ))
}

/// The device's spelling of the QoS lines of a parsed configuration.
fn canonical_qos(roots: &mut [Node]) {
    for root in roots.iter_mut() {
        if let Some(header) = class_map_header(&root.text) {
            root.text = header;
            for statement in &mut root.children {
                if let Some((ip, values)) = dscp_statement(&statement.text) {
                    statement.text = dscp_statement_text(ip, &values);
                }
            }
            // The device keeps one of two statements that say the same.
            let mut seen: Vec<String> = Vec::new();
            root.children.retain(|statement| {
                let fresh = !seen.contains(&statement.text);
                seen.push(statement.text.clone());
                fresh
            });
        } else if policy_map_name(&root.text).is_some() {
            for class in root
                .children
                .iter_mut()
                .filter(|class| class_of(&class.text).is_some())
            {
                for action in &mut class.children {
                    if let Some(text) = canonical_set(&action.text) {
                        action.text = text;
                    }
                }
            }
        }
    }
}

/// A line typed in `path`, as the device prints it back.
fn spelled_in(path: &[String], line: &str) -> String {
    if path.len() == 1
        && class_map_name(&path[0]).is_some()
        && let Some((ip, values)) = dscp_statement(line)
    {
        return dscp_statement_text(ip, &values);
    }
    if is_policy_class(path)
        && let Some(text) = canonical_set(line)
    {
        return text;
    }
    line.to_string()
}

/// The `no …` lines of modular QoS that do something other than remove the
/// line they repeat. Returns whether `target` was one.
fn remove_qos(roots: &mut Vec<Node>, path: &mut Vec<String>, target: &str) -> bool {
    // `no match dscp ef` takes the value out of whichever statements list
    // it, and a statement left with none goes.
    if path.len() == 1
        && class_map_name(&path[0]).is_some()
        && let Some((ip, values)) = dscp_statement(target)
    {
        let statements = children_at(roots, path);
        for statement in statements.iter_mut() {
            if let Some((same, listed)) = dscp_statement(&statement.text)
                && same == ip
            {
                let kept: Vec<u8> = listed
                    .into_iter()
                    .filter(|value| !values.contains(value))
                    .collect();
                statement.text = if kept.is_empty() {
                    String::new()
                } else {
                    dscp_statement_text(ip, &kept)
                };
            }
        }
        statements.retain(|statement| !statement.text.is_empty());
        return true;
    }
    // A class map is removed by its name, whatever match type the line says.
    if let Some(name) = class_map_name(target) {
        let before = roots.len();
        roots.retain(|node| class_map_name(&node.text) != Some(name));
        if roots.len() != before {
            path.clear();
        }
        return true;
    }
    false
}

/// What has to come off a device, in an order it accepts, before the rest of
/// a remediation can be made: what is in use cannot be removed, and an
/// interface takes one policy per direction.
///
/// 1. An attachment (`service-policy input|output P`) whose policy goes, or
///    that the intended configuration does not have, is taken off.
/// 2. A policy that goes is taken out of the classes of a parent that stays,
///    and the class of a class map that goes out of a policy that stays.
/// 3. Policy maps that go, a parent before the policies inside it.
/// 4. Class maps that go.
///
/// A policy map "goes" when it is removed, when its classes change order (it
/// is then typed again in full), or when a class map it uses is rebuilt. A
/// class map goes when it is removed, retyped with another match type, or
/// changed other than by statements added at the end (the device takes a
/// value out of every statement that lists it, so statements are not edited
/// one by one). What is put back — the rebuilt maps, the attachments — is
/// then ordinary remediation from the configuration this leaves.
fn mqc_teardown(running: &[Node], intended: &[Node], warnings: &mut Vec<String>) -> Vec<String> {
    let mut maps_removed: Vec<&str> = Vec::new();
    let mut maps_rebuilt: Vec<&str> = Vec::new();
    for node in running {
        let Some(name) = class_map_name(&node.text) else {
            continue;
        };
        match find_class_map(intended, name) {
            None => maps_removed.push(name),
            Some(wanted)
                if wanted.text != node.text
                    || (wanted.children != node.children
                        && !is_patchable_in_place(node, wanted)) =>
            {
                maps_rebuilt.push(name)
            }
            Some(_) => {}
        }
    }
    let mut policies_down: Vec<&str> = Vec::new();
    let mut policies_rebuilt: Vec<&str> = Vec::new();
    for node in running {
        let Some(name) = policy_map_name(&node.text) else {
            continue;
        };
        match find_policy(intended, name) {
            None => policies_down.push(name),
            Some(wanted) => {
                let reordered = wanted.children != node.children && !policy_patchable(node, wanted);
                let uses_rebuilt = classes_of(node).any(|class| maps_rebuilt.contains(&class));
                if reordered || uses_rebuilt {
                    policies_down.push(name);
                    policies_rebuilt.push(name);
                }
            }
        }
    }

    let mut commands: Vec<String> = Vec::new();
    // 1.
    for node in running {
        let wanted = intended.iter().find(|other| other.text == node.text);
        let off: Vec<&Node> = node
            .children
            .iter()
            .filter(|child| {
                attachment(&child.text).is_some_and(|(_, policy)| {
                    policies_down.contains(&policy)
                        || !wanted.is_some_and(|wanted| {
                            wanted.children.iter().any(|kept| kept.text == child.text)
                        })
                })
            })
            .collect();
        if off.is_empty() {
            continue;
        }
        commands.push(node.text.clone());
        for child in &off {
            commands.push(format!("no {}", child.text));
            let Some((direction, old)) = attachment(&child.text) else {
                continue;
            };
            let next = wanted.and_then(|wanted| {
                wanted
                    .children
                    .iter()
                    .filter_map(|kept| attachment(&kept.text))
                    .find(|(dir, policy)| *dir == direction && *policy != old)
            });
            if let Some((_, next)) = next {
                warnings.push(format!(
                    "`{}` has `service-policy {direction} {old}` taken off before `service-policy {direction} {next}` goes on (the device takes one policy per direction), so for a moment no {direction} policy applies there",
                    node.text
                ));
            }
        }
        commands.push("exit".into());
    }
    // 2.
    for node in running {
        let Some(name) = policy_map_name(&node.text) else {
            continue;
        };
        if policies_down.contains(&name) {
            continue;
        }
        let mut lines: Vec<String> = Vec::new();
        for class in &node.children {
            let Some(class_name) = class_of(&class.text) else {
                continue;
            };
            if maps_removed.contains(&class_name) {
                lines.push(format!("no {}", class.text));
                continue;
            }
            let off: Vec<String> = class
                .children
                .iter()
                .filter(|action| {
                    child_policy(&action.text).is_some_and(|child| policies_down.contains(&child))
                })
                .map(|action| format!("no {}", action.text))
                .collect();
            if !off.is_empty() {
                lines.push(class.text.clone());
                lines.extend(off);
                lines.push("exit".into());
            }
        }
        if !lines.is_empty() {
            commands.push(node.text.clone());
            commands.extend(lines);
            commands.push("exit".into());
        }
    }
    // 3.
    let mut left = policies_down.clone();
    while !left.is_empty() {
        let unused = left.iter().position(|name| {
            !left.iter().any(|other| {
                other != name
                    && find_policy(running, other)
                        .is_some_and(|parent| child_policies(parent).any(|child| child == *name))
            })
        });
        // Policies cannot contain each other; were they to, the last one
        // goes and the device says what it thinks of that.
        let at = unused.unwrap_or(left.len() - 1);
        let name = left.remove(at);
        commands.push(format!("no policy-map {name}"));
    }
    // 4.
    for node in running {
        if let Some(name) = class_map_name(&node.text)
            && (maps_removed.contains(&name) || maps_rebuilt.contains(&name))
        {
            commands.push(format!("no {}", node.text));
        }
    }
    for name in &policies_rebuilt {
        warnings.push(format!(
            "`policy-map {name}` is removed and typed again in full (its classes change order, or a class map it uses is rebuilt): what attaches it is taken off first and put back afterwards, so for a moment it applies nowhere"
        ));
    }
    for name in &maps_rebuilt {
        warnings.push(format!(
            "`class-map {name}` is removed and typed again in full (its statements change other than by one added at the end), and with it every policy map that uses it"
        ));
    }
    commands
}

/// Where a QoS policy is attached: by `section` (an interface, the control
/// plane), for traffic leaving (`output`) or arriving.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attached {
    pub section: String,
    pub output: bool,
    pub policy: String,
}

impl ConfigTree {
    /// Every `service-policy input|output NAME` of the configuration.
    pub fn qos_attached(&self) -> Vec<Attached> {
        let mut found = Vec::new();
        for node in &self.roots {
            for child in &node.children {
                if let Some((direction, policy)) = attachment(&child.text) {
                    found.push(Attached {
                        section: node.text.clone(),
                        output: direction == "output",
                        policy: policy.to_string(),
                    });
                }
            }
        }
        found
    }

    /// The attachments through which `policy` applies: its own, and those of
    /// every policy it is applied inside.
    pub fn qos_applies(&self, policy: &str) -> Vec<Attached> {
        let mut names = vec![policy.to_string()];
        let mut at = 0;
        while at < names.len() && names.len() <= 16 {
            let name = names[at].clone();
            for node in &self.roots {
                if let Some(parent) = policy_map_name(&node.text)
                    && child_policies(node).any(|child| child == name)
                    && !names.iter().any(|known| known == parent)
                {
                    names.push(parent.to_string());
                }
            }
            at += 1;
        }
        self.qos_attached()
            .into_iter()
            .filter(|attached| names.contains(&attached.policy))
            .collect()
    }

    /// The classes of `policy`, in order, each with the policy applied
    /// inside it; `None` when the configuration has no such policy map.
    pub fn qos_classes(&self, policy: &str) -> Option<Vec<(String, Option<String>)>> {
        let node = find_policy(&self.roots, policy)?;
        Some(
            node.children
                .iter()
                .filter_map(|class| {
                    let name = class_of(&class.text)?;
                    let child = class
                        .children
                        .iter()
                        .find_map(|action| child_policy(&action.text));
                    Some((name.to_string(), child.map(str::to_string)))
                })
                .collect(),
        )
    }
}

/// A hand-written line the device would refuse for a QoS reason that its
/// answer may not make plain: removing a class map or policy map that is in
/// use, naming one that does not exist yet, or attaching a second policy in
/// one direction. The simulator's refusals of the first and the last are not
/// worded so that the IOS device type's error patterns see them, so such a
/// change would be sent, half refused and only caught by its verification;
/// it is refused before anything is sent instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QosRefusal {
    /// Index into the commands of the line the device would refuse.
    pub index: usize,
    pub message: String,
}

/// The first line of `commands` that the device would refuse on `running`,
/// for one of those reasons, followed through the lines before it (IOS
/// syntax).
pub fn qos_refusal(running: &str, commands: &[String]) -> Option<QosRefusal> {
    let grammar = Grammar::for_syntax(Syntax::Ios);
    let mut tree = ConfigTree::parse(running, Syntax::Ios);
    let mut path: Vec<String> = Vec::new();
    let mut report = ApplyReport::default();
    for (index, raw) in commands.iter().enumerate() {
        if let Some(message) = refusal_of(&tree, &path, raw.trim()) {
            return Some(QosRefusal { index, message });
        }
        tree.apply_line(&mut path, raw, &grammar, &mut report);
    }
    None
}

fn refusal_of(tree: &ConfigTree, path: &[String], line: &str) -> Option<String> {
    if let Some(target) = line.strip_prefix("no ") {
        let target = target.trim();
        if let Some(name) = policy_map_name(target) {
            let user = tree.qos_applies(name).into_iter().find(|attached| attached.policy == name);
            if let Some(user) = user {
                return Some(format!(
                    "`policy-map {name}` is attached to `{}` ({}), and the device refuses to remove a policy map that is in use: take it off there first (`no service-policy {} {name}`)",
                    user.section,
                    if user.output { "output" } else { "input" },
                    if user.output { "output" } else { "input" },
                ));
            }
            let parent = tree.roots.iter().find_map(|node| {
                let parent = policy_map_name(&node.text)?;
                child_policies(node).any(|child| child == name).then_some(parent)
            });
            return parent.map(|parent| format!(
                "`policy-map {name}` is applied inside `policy-map {parent}`, and the device refuses to remove a policy map that is in use: take `service-policy {name}` out of that policy first"
            ));
        }
        if let Some(name) = class_map_name(target) {
            let user = tree.roots.iter().find_map(|node| {
                let policy = policy_map_name(&node.text)?;
                classes_of(node).any(|class| class == name).then_some(policy)
            });
            return user.map(|policy| format!(
                "`class-map {name}` is used by `policy-map {policy}`, and the device refuses to remove a class map that is in use: take `class {name}` out of that policy (or remove the policy) first"
            ));
        }
        return None;
    }
    if path.len() == 1
        && let Some((direction, name)) = attachment(line)
    {
        if find_policy(&tree.roots, name).is_none() {
            return Some(format!(
                "`policy-map {name}` does not exist at this point: define it before the line that attaches it"
            ));
        }
        let section = tree.roots.iter().find(|node| node.text == path[0])?;
        let other = section
            .children
            .iter()
            .filter_map(|child| attachment(&child.text))
            .find(|(dir, other)| *dir == direction && *other != name);
        return other.map(|(_, other)| format!(
            "`{}` already has `service-policy {direction} {other}`, and the device refuses a second policy in the same direction: take that one off first (`no service-policy {direction} {other}`)",
            path[0]
        ));
    }
    if is_policy_class(path)
        && let Some(child) = child_policy(line)
        && find_policy(&tree.roots, child).is_none()
    {
        return Some(format!(
            "`policy-map {child}` does not exist at this point: define it before the class that applies it"
        ));
    }
    if path
        .first()
        .is_some_and(|open| policy_map_name(open).is_some())
        && let Some(class) = class_of(line)
        && class != CLASS_DEFAULT
        && find_class_map(&tree.roots, class).is_none()
    {
        return Some(format!(
            "`class-map {class}` does not exist at this point: define it before the policy that names it"
        ));
    }
    None
}

/// One line of a unified diff, for display.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    /// `" "`, `"+"` or `"-"`.
    pub tag: String,
    pub text: String,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDiff {
    pub lines: Vec<DiffLine>,
    pub added: usize,
    pub removed: usize,
}

/// A line diff of two texts with `context` unchanged lines around each change
/// (`None` keeps everything).
pub fn text_diff(old: &str, new: &str, context: Option<usize>) -> TextDiff {
    use similar::ChangeTag;

    let diff = similar::TextDiff::from_lines(old, new);
    let mut all: Vec<DiffLine> = Vec::new();
    let (mut added, mut removed) = (0, 0);
    for change in diff.iter_all_changes() {
        let tag = match change.tag() {
            ChangeTag::Equal => " ",
            ChangeTag::Insert => {
                added += 1;
                "+"
            }
            ChangeTag::Delete => {
                removed += 1;
                "-"
            }
        };
        all.push(DiffLine {
            tag: tag.to_string(),
            text: change.value().trim_end_matches(['\n', '\r']).to_string(),
            old_line: change.old_index().map(|index| index + 1),
            new_line: change.new_index().map(|index| index + 1),
        });
    }
    let lines = match context {
        None => all,
        Some(radius) => {
            let changed: Vec<usize> = all
                .iter()
                .enumerate()
                .filter(|(_, line)| line.tag != " ")
                .map(|(index, _)| index)
                .collect();
            let keep = |index: usize| changed.iter().any(|at| index.abs_diff(*at) <= radius);
            let mut kept = Vec::new();
            let mut gap = false;
            for (index, line) in all.into_iter().enumerate() {
                if keep(index) {
                    if gap && !kept.is_empty() {
                        kept.push(DiffLine {
                            tag: "@".into(),
                            text: "…".into(),
                            old_line: None,
                            new_line: None,
                        });
                    }
                    gap = false;
                    kept.push(line);
                } else {
                    gap = true;
                }
            }
            kept
        }
    };
    TextDiff {
        lines,
        added,
        removed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const RUNNING: &str = "\
Building configuration...

Current configuration : 2048 bytes
!
version 15.2
hostname SW1
!
enable secret 5 $1$old$hash
username admin privilege 15 secret 5 $1$a$b
username olduser privilege 1 secret 5 $1$c$d
!
vlan 10
 name Staff
!
vlan 20
 name Guests
!
interface GigabitEthernet0/1
 description to-core
 switchport trunk allowed vlan 10,20
 switchport mode trunk
!
interface GigabitEthernet0/2
 description printer
 switchport access vlan 10
 switchport mode access
 spanning-tree portfast
!
interface Vlan10
 ip address 10.0.10.2 255.255.255.0
 no shutdown
!
interface Loopback9
 ip address 9.9.9.9 255.255.255.255
!
ip route 0.0.0.0 0.0.0.0 10.0.10.1
!
line vty 0 4
 exec-timeout 5 0
 transport input telnet ssh
!
end
";

    fn fix(intended: &str) -> Vec<String> {
        let result = remediate(RUNNING, intended, Syntax::Ios);
        assert!(result.supported);
        result.commands
    }

    #[test]
    fn identical_configs_need_nothing() {
        let result = remediate(RUNNING, RUNNING, Syntax::Ios);
        assert_eq!(
            result,
            Remediation {
                commands: vec![],
                warnings: vec![],
                supported: true
            }
        );
    }

    #[test]
    fn parses_blocks_and_ignores_display_noise() {
        let tree = ConfigTree::parse(RUNNING, Syntax::Ios);
        assert_eq!(tree.roots[0].text, "version 15.2");
        let port = tree.find(&["interface GigabitEthernet0/2"]).unwrap();
        assert_eq!(port.children.len(), 4);
        assert!(tree.find(&["interface Vlan10", "no shutdown"]).is_some());
        assert!(
            tree.roots
                .iter()
                .all(|node| node.text != "end" && !node.text.starts_with('!'))
        );
        assert_eq!(
            ConfigTree::parse(&tree.render(), Syntax::Ios),
            tree,
            "render/parse round-trips"
        );
    }

    #[test]
    fn adds_a_vlan_and_extends_the_trunk_without_restating_it() {
        let intended = RUNNING
            .replace(
                "vlan 20\n name Guests\n",
                "vlan 20\n name Guests\n!\nvlan 30\n name Voice\n",
            )
            .replace("allowed vlan 10,20", "allowed vlan 10,20,30");
        assert_eq!(
            fix(&intended),
            vec![
                "vlan 30",
                "name Voice",
                "exit",
                "interface GigabitEthernet0/1",
                "switchport trunk allowed vlan add 30",
                "exit",
            ]
        );
    }

    #[test]
    fn trunk_vlan_removal_uses_remove_and_handles_ranges_and_add_lines() {
        let running = "interface Gi1\n switchport trunk allowed vlan 10,20,30-35\n switchport trunk allowed vlan add 100,200\n";
        let intended = "interface Gi1\n switchport trunk allowed vlan 10,30-33,100,300\n";
        assert_eq!(
            remediate(running, intended, Syntax::Ios).commands,
            vec![
                "interface Gi1",
                "switchport trunk allowed vlan add 300",
                "switchport trunk allowed vlan remove 20,34-35,200",
                "exit",
            ]
        );
        // From "all VLANs" to a list there is nothing to patch: state the list.
        let open = "interface Gi1\n switchport mode trunk\n";
        let restricted =
            "interface Gi1\n switchport mode trunk\n switchport trunk allowed vlan 10,20\n";
        assert_eq!(
            remediate(open, restricted, Syntax::Ios).commands,
            vec![
                "interface Gi1",
                "switchport trunk allowed vlan 10,20",
                "exit"
            ]
        );
        assert_eq!(
            remediate(restricted, open, Syntax::Ios).commands,
            vec!["interface Gi1", "no switchport trunk allowed vlan", "exit"]
        );
    }

    #[test]
    fn replace_style_commands_are_not_negated_first() {
        let intended = RUNNING
            .replace(
                "ip address 10.0.10.2 255.255.255.0",
                "ip address 10.0.10.3 255.255.255.0",
            )
            .replace("description printer", "description scanner")
            .replace("switchport access vlan 10", "switchport access vlan 20")
            .replace("hostname SW1", "hostname SW1-NEW")
            .replace("enable secret 5 $1$old$hash", "enable secret 5 $1$new$hash");
        assert_eq!(
            fix(&intended),
            vec![
                "hostname SW1-NEW",
                "enable secret 5 $1$new$hash",
                "interface GigabitEthernet0/2",
                "description scanner",
                "switchport access vlan 20",
                "exit",
                "interface Vlan10",
                "ip address 10.0.10.3 255.255.255.0",
                "exit",
            ]
        );
    }

    #[test]
    fn removals_use_the_right_negation_for_each_line() {
        let intended = RUNNING
            .replace("username olduser privilege 1 secret 5 $1$c$d\n", "")
            .replace(" description printer\n", "")
            .replace(" spanning-tree portfast\n", "")
            .replace(" no shutdown\n", "")
            .replace(
                "interface Loopback9\n ip address 9.9.9.9 255.255.255.255\n",
                "",
            )
            .replace("ip route 0.0.0.0 0.0.0.0 10.0.10.1\n", "")
            .replace("vlan 20\n name Guests\n", "");
        assert_eq!(
            fix(&intended),
            vec![
                "interface GigabitEthernet0/2",
                "no description",
                "no spanning-tree portfast",
                "exit",
                "interface Vlan10",
                "shutdown",
                "exit",
                "no username olduser",
                "no vlan 20",
                "no interface Loopback9",
                "no ip route 0.0.0.0 0.0.0.0 10.0.10.1",
            ]
        );
    }

    #[test]
    fn physical_interfaces_are_defaulted_and_unremovable_lines_warn() {
        let intended = RUNNING
            .replace("interface GigabitEthernet0/2\n description printer\n switchport access vlan 10\n switchport mode access\n spanning-tree portfast\n", "")
            .replace("line vty 0 4\n exec-timeout 5 0\n transport input telnet ssh\n", "")
            .replace("hostname SW1\n", "");
        let result = remediate(RUNNING, &intended, Syntax::Ios);
        assert_eq!(
            result.commands,
            vec!["default interface GigabitEthernet0/2"]
        );
        assert_eq!(result.warnings.len(), 2);
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.contains("hostname SW1"))
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.contains("line vty 0 4"))
        );
    }

    #[test]
    fn replacements_are_made_before_the_old_entry_is_removed() {
        let running = "interface Vlan10\n ip address dhcp\n ip access-group OLD in\n ip access-group EGRESS out\nip route 0.0.0.0 0.0.0.0 10.0.0.1\nlogging host 10.0.0.5\n";
        let intended = "interface Vlan10\n ip address 10.0.10.2 255.255.255.0\n ip access-group NEW in\n ip access-group EGRESS out\nip route 0.0.0.0 0.0.0.0 10.0.0.254\nlogging host 10.0.0.6\n";
        assert_eq!(
            remediate(running, intended, Syntax::Ios).commands,
            vec![
                "interface Vlan10",
                "ip address 10.0.10.2 255.255.255.0",
                "ip access-group NEW in",
                "exit",
                "ip route 0.0.0.0 0.0.0.0 10.0.0.254",
                "logging host 10.0.0.6",
                "no ip route 0.0.0.0 0.0.0.0 10.0.0.1",
                "no logging host 10.0.0.5",
            ]
        );
    }

    #[test]
    fn nested_sections_enter_and_exit_correctly() {
        let running = "router bgp 65001\n neighbor 10.0.0.2 remote-as 65002\n address-family ipv4\n  network 10.1.0.0 mask 255.255.0.0\n";
        let intended = "router bgp 65001\n neighbor 10.0.0.2 remote-as 65002\n neighbor 10.0.0.3 remote-as 65003\n address-family ipv4\n  network 10.1.0.0 mask 255.255.0.0\n  network 10.2.0.0 mask 255.255.0.0\n  neighbor 10.0.0.3 activate\nntp server 10.0.0.9\n";
        assert_eq!(
            remediate(running, intended, Syntax::Ios).commands,
            vec![
                "router bgp 65001",
                "neighbor 10.0.0.3 remote-as 65003",
                "address-family ipv4",
                "network 10.2.0.0 mask 255.255.0.0",
                "neighbor 10.0.0.3 activate",
                "exit",
                "exit",
                "ntp server 10.0.0.9",
            ]
        );
    }

    #[test]
    fn access_lists_append_in_place_or_are_rebuilt() {
        let running = "ip access-list extended WEB\n permit tcp any any eq 80\n deny ip any any\n";
        let appended = "ip access-list extended WEB\n permit tcp any any eq 80\n deny ip any any\n permit icmp any any\n";
        assert_eq!(
            remediate(running, appended, Syntax::Ios).commands,
            vec!["ip access-list extended WEB", "permit icmp any any", "exit"]
        );

        let reordered = "ip access-list extended WEB\n permit tcp any any eq 443\n permit tcp any any eq 80\n deny ip any any\n";
        let result = remediate(running, reordered, Syntax::Ios);
        assert_eq!(
            result.commands,
            vec![
                "no ip access-list extended WEB",
                "ip access-list extended WEB",
                "permit tcp any any eq 443",
                "permit tcp any any eq 80",
                "deny ip any any",
                "exit",
            ]
        );
        assert!(result.warnings[0].contains("order-sensitive"));

        let numbered_old =
            "ip access-list extended WEB\n 10 permit tcp any any eq 80\n 20 deny ip any any\n";
        let numbered_new = "ip access-list extended WEB\n 10 permit tcp any any eq 80\n 15 permit tcp any any eq 443\n";
        assert_eq!(
            remediate(numbered_old, numbered_new, Syntax::Ios).commands,
            vec![
                "ip access-list extended WEB",
                "15 permit tcp any any eq 443",
                "no 20",
                "exit"
            ]
        );
    }

    #[test]
    fn banners_are_single_values() {
        let running = "hostname R1\nbanner motd ^C\nAuthorised users only\nSecond line\n^C\nntp server 1.1.1.1\n";
        let tree = ConfigTree::parse(running, Syntax::Ios);
        assert_eq!(tree.roots.len(), 3);
        assert_eq!(
            tree.roots[1].text,
            "banner motd ^C\nAuthorised users only\nSecond line\n^C"
        );

        let changed = running.replace("Authorised users only", "Keep out");
        assert_eq!(
            remediate(running, &changed, Syntax::Ios).commands,
            vec!["banner motd ^C\nKeep out\nSecond line\n^C"]
        );
        let removed = "hostname R1\nntp server 1.1.1.1\n";
        assert_eq!(
            remediate(running, removed, Syntax::Ios).commands,
            vec!["no banner motd"]
        );
        let inline = ConfigTree::parse("banner login #Welcome#\nhostname X\n", Syntax::Ios);
        assert_eq!(inline.roots.len(), 2);
    }

    #[test]
    fn rollback_is_remediation_in_reverse() {
        let intended = RUNNING
            .replace(
                "vlan 20\n name Guests\n",
                "vlan 20\n name Guests\n!\nvlan 30\n name Voice\n",
            )
            .replace("allowed vlan 10,20", "allowed vlan 10,20,30")
            .replace("description printer", "description scanner");
        assert_eq!(
            remediate(&intended, RUNNING, Syntax::Ios).commands,
            vec![
                "interface GigabitEthernet0/1",
                "switchport trunk allowed vlan remove 30",
                "exit",
                "interface GigabitEthernet0/2",
                "description printer",
                "exit",
                "no vlan 30",
            ]
        );
    }

    #[test]
    fn vrp_uses_undo_and_quit() {
        let running = "#\nsysname SW-A\n#\nvlan batch 10 20\n#\ninterface GigabitEthernet0/0/1\n description old\n port link-type access\n port default vlan 10\n#\ninterface LoopBack0\n ip address 1.1.1.1 255.255.255.255\n#\nreturn\n";
        let intended = "#\nsysname SW-A\n#\nvlan batch 10 20\n#\ninterface GigabitEthernet0/0/1\n description new\n port link-type access\n port default vlan 20\n stp edged-port enable\n#\nreturn\n";
        assert_eq!(
            remediate(running, intended, Syntax::Vrp).commands,
            vec![
                "interface GigabitEthernet0/0/1",
                "description new",
                "port default vlan 20",
                "stp edged-port enable",
                "quit",
                "undo interface LoopBack0",
            ]
        );
    }

    #[test]
    fn set_style_configs_delete_then_set() {
        let running = "set system host-name old\nset system name-server 8.8.8.8\nset vlans staff vlan-id 10\n";
        let intended = "set system host-name new\nset system name-server 8.8.8.8\nset vlans staff vlan-id 10\nset vlans voice vlan-id 30\n";
        assert_eq!(
            remediate(running, intended, Syntax::Set).commands,
            vec![
                "delete system host-name old",
                "set system host-name new",
                "set vlans voice vlan-id 30"
            ]
        );
    }

    #[test]
    fn routeros_adds_and_finds_entries_to_remove() {
        let running = "/interface vlan add interface=bridge1 name=vlan20 vlan-id=20\n/ip address add address=10.0.20.1/24 interface=vlan20 comment=\"guest net\"\n/ip dns set servers=1.1.1.1\n";
        let intended = "/interface vlan add interface=bridge1 name=vlan20 vlan-id=20\n/interface vlan add interface=bridge1 name=vlan30 vlan-id=30\n/ip dns set servers=9.9.9.9\n";
        let result = remediate(running, intended, Syntax::Routeros);
        assert_eq!(
            result.commands,
            vec![
                "/interface vlan add interface=bridge1 name=vlan30 vlan-id=30",
                "/ip dns set servers=9.9.9.9",
                "/ip address remove [find address=10.0.20.1/24 interface=vlan20]",
            ]
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.contains("cannot be undone"))
        );
    }

    #[test]
    fn unknown_grammar_is_reported_not_guessed() {
        let result = remediate("a", "b", Syntax::Flat);
        assert!(!result.supported && result.commands.is_empty() && !result.warnings.is_empty());
    }

    /// Every line of a configuration with the section it sits in, sorted:
    /// "the same configuration", whatever order the device prints it in.
    fn canonical(text: &str, syntax: Syntax) -> Vec<String> {
        fn walk(nodes: &[Node], prefix: &str, out: &mut Vec<String>) {
            for node in nodes {
                let path = if prefix.is_empty() {
                    node.text.clone()
                } else {
                    format!("{prefix} / {}", node.text)
                };
                out.push(path.clone());
                walk(&node.children, &path, out);
            }
        }
        let mut lines = Vec::new();
        walk(&ConfigTree::parse(text, syntax).roots, "", &mut lines);
        lines.sort();
        lines
    }

    fn apply_to(running: &str, commands: &[&str]) -> String {
        let mut tree = ConfigTree::parse(running, Syntax::Ios);
        let commands: Vec<String> = commands.iter().map(|line| line.to_string()).collect();
        let report = tree.apply(&commands, Syntax::Ios);
        assert!(
            report.supported && report.unmodelled.is_empty(),
            "{report:?}"
        );
        tree.render()
    }

    #[test]
    fn applying_a_remediation_reaches_the_intended_configuration() {
        let variants = [
            // add a VLAN and extend a trunk
            RUNNING
                .replace(
                    "vlan 20\n name Guests\n",
                    "vlan 20\n name Guests\n!\nvlan 30\n name Voice\n",
                )
                .replace("allowed vlan 10,20", "allowed vlan 10,20,30"),
            // change values held in a "slot"
            RUNNING
                .replace("description printer", "description scanner")
                .replace("switchport access vlan 10", "switchport access vlan 20")
                .replace(
                    "ip address 10.0.10.2 255.255.255.0",
                    "ip address 10.0.10.3 255.255.255.0",
                ),
            // remove things of several shapes
            RUNNING
                .replace("username olduser privilege 1 secret 5 $1$c$d\n", "")
                .replace(" spanning-tree portfast\n", "")
                .replace("ip route 0.0.0.0 0.0.0.0 10.0.10.1\n", "")
                .replace("vlan 20\n name Guests\n", "")
                .replace(
                    "interface Loopback9\n ip address 9.9.9.9 255.255.255.255\n",
                    "",
                ),
            // nested sections
            format!("{RUNNING}router ospf 1\n network 10.0.10.0 0.0.0.255 area 0\n"),
        ];
        for intended in variants {
            let commands = remediate(RUNNING, &intended, Syntax::Ios).commands;
            let predicted = apply_to(
                RUNNING,
                &commands.iter().map(String::as_str).collect::<Vec<_>>(),
            );
            assert_eq!(
                canonical(&predicted, Syntax::Ios),
                canonical(&intended, Syntax::Ios),
                "commands: {commands:?}"
            );
            // And the remediation in the other direction undoes it exactly.
            let back = remediate(&predicted, RUNNING, Syntax::Ios).commands;
            let restored = apply_to(
                &predicted,
                &back.iter().map(String::as_str).collect::<Vec<_>>(),
            );
            assert_eq!(
                canonical(&restored, Syntax::Ios),
                canonical(RUNNING, Syntax::Ios),
                "rollback commands: {back:?}"
            );
        }
    }

    #[test]
    fn sections_are_entered_and_left_the_way_a_device_does() {
        let running =
            "hostname SW1\ninterface Gi0/1\n description old\n shutdown\nvlan 10\n name Staff\n";
        let after = apply_to(
            running,
            &[
                "interface Gi0/1",
                "description new",
                "no shutdown",
                "exit",
                "ntp server 10.0.0.9",
                "vlan 10",
                "name Users",
                "end",
                "hostname SW2",
            ],
        );
        assert_eq!(
            after,
            "hostname SW2\ninterface Gi0/1\n description new\nvlan 10\n name Users\nntp server 10.0.0.9\n"
        );
        // A section header typed inside a section switches to it, as on the device.
        let switched = apply_to(
            running,
            &["interface Gi0/1", "vlan 10", "name Users", "exit"],
        );
        assert!(switched.contains("vlan 10\n name Users\n"), "{switched}");
    }

    #[test]
    fn removals_sub_commands_and_whole_sections() {
        let running = "hostname SW1\ninterface Gi0/2\n description printer\n switchport access vlan 10\nvlan 20\n name Guests\nip route 0.0.0.0 0.0.0.0 10.0.0.1\n";
        assert_eq!(
            apply_to(
                running,
                &[
                    "interface Gi0/2",
                    "no description",
                    "exit",
                    "no vlan 20",
                    "no ip route 0.0.0.0 0.0.0.0 10.0.0.1",
                    "no username nobody",
                ]
            ),
            "hostname SW1\ninterface Gi0/2\n switchport access vlan 10\n"
        );
        // `default interface` empties the port and says so.
        let mut tree = ConfigTree::parse(running, Syntax::Ios);
        let report = tree.apply(&["default interface Gi0/2".to_string()], Syntax::Ios);
        assert_eq!(
            tree.render(),
            "hostname SW1\ninterface Gi0/2\nvlan 20\n name Guests\nip route 0.0.0.0 0.0.0.0 10.0.0.1\n"
        );
        assert!(report.warnings[0].contains("resets the port"), "{report:?}");
    }

    #[test]
    fn the_allowed_vlan_list_of_a_trunk_is_patched_as_a_set() {
        let trunk = "interface Gi0/1\n switchport mode trunk\n switchport trunk allowed vlan 10,20\n description uplink\n";
        let add = apply_to(
            trunk,
            &["interface Gi0/1", "switchport trunk allowed vlan add 30,40"],
        );
        assert!(
            add.contains(" switchport trunk allowed vlan 10,20,30-40\n")
                || add.contains(" switchport trunk allowed vlan 10,20,30,40\n"),
            "{add}"
        );
        // the list keeps its place among the other lines
        assert!(add.ends_with(" description uplink\n"), "{add}");
        assert!(
            apply_to(
                trunk,
                &["interface Gi0/1", "switchport trunk allowed vlan remove 20"]
            )
            .contains(" switchport trunk allowed vlan 10\n")
        );
        assert!(
            apply_to(
                trunk,
                &["interface Gi0/1", "switchport trunk allowed vlan 99"]
            )
            .contains(" switchport trunk allowed vlan 99\n"),
            "a bare list replaces — which is exactly why the planner flags it"
        );
        assert!(
            apply_to(
                trunk,
                &["interface Gi0/1", "no switchport trunk allowed vlan"]
            )
            .contains("interface Gi0/1\n switchport mode trunk\n description uplink\n")
        );
        // On a trunk with no list every VLAN is allowed: `add` changes nothing,
        // `remove` has to state the list the device is left with.
        let open = "interface Gi0/1\n switchport mode trunk\n";
        assert_eq!(
            apply_to(
                open,
                &["interface Gi0/1", "switchport trunk allowed vlan add 30"]
            ),
            open
        );
        let removed = apply_to(
            open,
            &[
                "interface Gi0/1",
                "switchport trunk allowed vlan remove 4000-4094",
            ],
        );
        assert!(
            removed.contains(" switchport trunk allowed vlan 1-3999\n"),
            "{removed}"
        );
    }

    #[test]
    fn what_cannot_be_predicted_is_reported_not_guessed() {
        let mut tree = ConfigTree::parse("interface Gi0/1\n description x\n", Syntax::Ios);
        let report = tree.apply(
            &[
                "interface range Gi0/1 - 4".to_string(),
                "shutdown".to_string(),
            ],
            Syntax::Ios,
        );
        assert_eq!(report.unmodelled, vec!["interface range Gi0/1 - 4"]);

        let mut flat = ConfigTree::parse("/ip address add address=10.0.0.1/24", Syntax::Routeros);
        let report = flat.apply(
            &["/ip dns set servers=9.9.9.9".to_string()],
            Syntax::Routeros,
        );
        assert!(!report.supported && !report.warnings.is_empty());
        assert_eq!(flat.roots.len(), 1, "nothing is guessed at");
    }

    #[test]
    fn sections_and_global_commands_are_recognised() {
        for line in [
            "interface Vlan10",
            "router ospf 1",
            "line vty 0 4",
            "vlan 30",
            "ip access-list extended WEB",
            "ip dhcp pool LAN",
        ] {
            assert!(opens_section(line, Syntax::Ios), "{line}");
        }
        for line in [
            "vlan internal allocation policy ascending",
            "description vlan 30",
            "switchport trunk allowed vlan 10",
            "spanning-tree portfast",
        ] {
            assert!(!opens_section(line, Syntax::Ios), "{line}");
        }
        for line in [
            "hostname SW2",
            "username bob privilege 15 secret x",
            "no ip route 0.0.0.0 0.0.0.0 10.0.0.1",
            "ip default-gateway 10.0.0.1",
            "enable secret 5 x",
            "spanning-tree mode rapid-pvst",
            "interface Vlan10",
        ] {
            assert!(is_global(line, Syntax::Ios), "{line}");
        }
        // Sub-commands that merely look global must stay with their section.
        for line in [
            "ip address 10.0.0.1 255.255.255.0",
            "spanning-tree portfast",
            "service-policy input SHAPE",
            "logging event link-status",
            "switchport mode trunk",
        ] {
            assert!(!is_global(line, Syntax::Ios), "{line}");
        }
    }

    #[test]
    fn vrp_lines_are_applied_with_undo_and_quit() {
        let running = "sysname SW-A\ninterface GigabitEthernet0/0/1\n description old\n port default vlan 10\n";
        let mut tree = ConfigTree::parse(running, Syntax::Vrp);
        let commands = [
            "interface GigabitEthernet0/0/1",
            "description new",
            "undo port default vlan",
            "quit",
            "sysname SW-B",
        ]
        .map(String::from)
        .to_vec();
        assert!(tree.apply(&commands, Syntax::Vrp).supported);
        assert_eq!(
            tree.render(),
            "sysname SW-B\ninterface GigabitEthernet0/0/1\n description new\n"
        );
    }

    #[test]
    fn vlan_range_helpers() {
        let set = parse_vlan_ranges("10, 20,30-32").unwrap();
        assert_eq!(
            set.iter().copied().collect::<Vec<_>>(),
            vec![10, 20, 30, 31, 32]
        );
        assert_eq!(format_vlan_ranges(&[1, 2, 3, 7, 9, 10]), "1-3,7,9-10");
        assert!(
            parse_vlan_ranges("10-5").is_none()
                && parse_vlan_ranges("5000").is_none()
                && parse_vlan_ranges("x").is_none()
        );
    }

    #[test]
    fn text_diff_counts_and_trims_context() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let new = "a\nb\nC\nd\ne\nf\ng\nh\ni\n";
        let full = text_diff(old, new, None);
        assert_eq!((full.added, full.removed), (2, 1));
        assert_eq!(full.lines.len(), 10);
        let trimmed = text_diff(old, new, Some(1));
        let shown: Vec<String> = trimmed
            .lines
            .iter()
            .map(|line| format!("{}{}", line.tag, line.text))
            .collect();
        assert_eq!(shown, vec![" b", "-c", "+C", " d", "@…", " h", "+i"]);
    }

    // ------------------------------------------------------- modular QoS

    fn applied(running: &str, commands: &[&str]) -> Vec<String> {
        canonical(&apply_to(running, commands), Syntax::Ios)
    }

    fn strings(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    /// The commands work on a device in the order given: nothing in use is
    /// removed, nothing is named before it exists, no interface gets a
    /// second policy in one direction — and they reach `intended`.
    fn reaches(running: &str, intended: &str) -> Vec<String> {
        let commands = remediate(running, intended, Syntax::Ios).commands;
        assert_eq!(
            qos_refusal(running, &commands),
            None,
            "the device would refuse a line of {commands:#?}"
        );
        let refs: Vec<&str> = commands.iter().map(String::as_str).collect();
        assert_eq!(
            applied(running, &refs),
            canonical(intended, Syntax::Ios),
            "commands: {commands:#?}"
        );
        commands
    }

    const WAN: &str = "\
class-map match-any WAN-VOICE
 match dscp ef
class-map match-any WAN-VIDEO
 match dscp af41 af42 af43
policy-map WAN
 class WAN-VOICE
  priority percent 10
 class WAN-VIDEO
  bandwidth percent 20
policy-map WAN-SHAPE
 class class-default
  shape average 50000000
  service-policy WAN
interface GigabitEthernet0/1
 description wan
 service-policy output WAN-SHAPE
";

    #[test]
    fn a_value_typed_again_into_a_class_replaces_the_one_it_had() {
        // h2-qos-templates-and-sim-0: the prediction kept the old line beside
        // the new one, so a device that replaced it — as IOS does — differed.
        let running = "policy-map P\n class V\n  priority percent 10\n  police 8000\n class class-default\n  shape average 50000000\n  set dscp ef\n  service-policy CHILD\n";
        assert_eq!(
            applied(
                running,
                &[
                    "policy-map P",
                    "class V",
                    "priority percent 15",
                    "police cir 16000",
                    "exit",
                    "class class-default",
                    "shape average 40000000",
                    "set dscp 26",
                    "service-policy OTHER",
                ]
            ),
            canonical(
                "policy-map P\n class V\n  priority percent 15\n  police cir 16000\n class class-default\n  shape average 40000000\n  set dscp af31\n  service-policy OTHER\n",
                Syntax::Ios
            )
        );
        // `priority` with and without a rate is one setting; `priority-queue` is another command.
        assert_eq!(
            applied("policy-map P\n class V\n  priority percent 10\n", &["policy-map P", "class V", "priority"]),
            canonical("policy-map P\n class V\n  priority\n", Syntax::Ios)
        );
        assert_eq!(
            applied("interface Gi0/1\n priority-queue out\n", &["interface Gi0/1", "priority 1"]).len(),
            3
        );
    }

    #[test]
    fn the_way_back_takes_a_policy_off_and_removes_what_uses_before_what_is_used() {
        // h2-qos-templates-and-sim-1: removals followed the running order —
        // class maps first, a child policy before its parent — and the device
        // refused every one of them while they were in use.
        let before = "interface GigabitEthernet0/1\n description wan\n";
        let back = reaches(WAN, before);
        assert_eq!(
            back,
            strings(&[
                "interface GigabitEthernet0/1",
                "no service-policy output WAN-SHAPE",
                "exit",
                "no policy-map WAN-SHAPE",
                "no policy-map WAN",
                "no class-map match-any WAN-VOICE",
                "no class-map match-any WAN-VIDEO",
            ])
        );
        // And the other way: definitions before what names them, a child
        // policy before its parent, whatever order the text lists them in.
        let scrambled = "interface GigabitEthernet0/1\n description wan\n service-policy output WAN-SHAPE\npolicy-map WAN-SHAPE\n class class-default\n  shape average 50000000\n  service-policy WAN\npolicy-map WAN\n class WAN-VOICE\n  priority percent 10\n class WAN-VIDEO\n  bandwidth percent 20\nclass-map match-any WAN-VIDEO\n match dscp af41 af42 af43\nclass-map match-any WAN-VOICE\n match dscp ef\n";
        reaches(before, scrambled);
    }

    #[test]
    fn taking_a_marking_out_of_a_statement_keeps_the_others_in_it() {
        // h2-qos-templates-and-sim-7: `no match dscp af42` takes the value
        // out of the statement that lists it, as IOS does.
        let running = "class-map match-any VIDEO\n match dscp af41 af42 af43\n match ip dscp af42\n";
        assert_eq!(
            applied(running, &["class-map match-any VIDEO", "no match dscp af42"]),
            canonical(
                "class-map match-any VIDEO\n match dscp af41 af43\n match ip dscp af42\n",
                Syntax::Ios
            )
        );
        assert_eq!(
            applied(running, &["class-map match-any VIDEO", "no match dscp 34 36 38"]),
            canonical("class-map match-any VIDEO\n match ip dscp af42\n", Syntax::Ios),
            "a statement left with nothing goes"
        );
    }

    #[test]
    fn markings_are_predicted_the_way_the_device_prints_them() {
        assert_eq!(
            applied(
                "",
                &[
                    "class-map match-any V",
                    "match dscp ef 44",
                    "match dscp EF",
                    "policy-map P",
                    "class V",
                    "set dscp 46",
                ]
            ),
            canonical(
                "class-map match-any V\n match dscp 44 ef\n match dscp ef\npolicy-map P\n class V\n  set dscp ef\n",
                Syntax::Ios
            )
        );
        // A configuration somebody wrote reads like one the device printed.
        assert_eq!(
            canonical("class-map C\n match dscp 46 10\n match dscp af11 ef\n", Syntax::Ios),
            canonical("class-map match-all C\n match dscp af11 ef\n", Syntax::Ios)
        );
        // What the device would refuse is left as it was typed.
        assert_eq!(
            applied("", &["class-map match-any V", "match dscp 64"]),
            canonical("class-map match-any V\n match dscp 64\n", Syntax::Ios)
        );
    }

    #[test]
    fn a_class_named_from_inside_another_one_moves_to_it() {
        // Without `exit` between classes, as people type them.
        assert_eq!(
            applied(
                "",
                &[
                    "policy-map P",
                    "class A",
                    "priority",
                    "class B",
                    "bandwidth 100",
                    "class class-default",
                    "set dscp 0",
                ]
            ),
            canonical(
                "policy-map P\n class A\n  priority\n class B\n  bandwidth 100\n class class-default\n  set dscp default\n",
                Syntax::Ios
            )
        );
        // `class-default` stays the last class.
        let mut tree = ConfigTree::parse("policy-map P\n class A\n class class-default\n  set dscp default\n", Syntax::Ios);
        tree.apply(&strings(&["policy-map P", "class C"]), Syntax::Ios);
        let classes: Vec<&str> = tree.roots[0].children.iter().map(|node| node.text.as_str()).collect();
        assert_eq!(classes, vec!["class A", "class C", "class class-default"]);
        // A class map is one per name: typed again with another type it changes.
        assert_eq!(
            applied("class-map match-any V\n match dscp ef\n", &["class-map match-all V", "match dscp cs5"]),
            canonical("class-map match-all V\n match dscp ef\n match dscp cs5\n", Syntax::Ios)
        );
        assert_eq!(
            applied("class-map match-any V\n match dscp ef\n", &["class-map V", "match dscp cs5"]),
            canonical("class-map match-any V\n match dscp ef\n match dscp cs5\n", Syntax::Ios)
        );
    }

    #[test]
    fn an_attached_policy_whose_classes_keep_their_order_is_edited_where_it_is() {
        let intended = WAN
            .replace("priority percent 10", "priority percent 15")
            .replace("shape average 50000000", "shape average 40000000");
        assert_eq!(
            reaches(WAN, &intended),
            strings(&[
                "policy-map WAN",
                "class WAN-VOICE",
                "priority percent 15",
                "exit",
                "exit",
                "policy-map WAN-SHAPE",
                "class class-default",
                "shape average 40000000",
                "exit",
                "exit",
            ]),
            "nothing is taken off the interface"
        );
        // Actions that exclude each other: what goes before what comes.
        let swapped = WAN.replace("priority percent 10", "bandwidth percent 10");
        let commands = reaches(WAN, &swapped);
        let at = |line: &str| commands.iter().position(|sent| sent == line).unwrap();
        assert!(at("no priority") < at("bandwidth percent 10"), "{commands:?}");
        // A class added at the end, one taken away.
        let grown = WAN.replace(
            "policy-map WAN\n",
            "class-map match-any WAN-BULK\n match dscp cs1\npolicy-map WAN\n",
        )
        .replace(" class WAN-VIDEO\n  bandwidth percent 20\n", " class WAN-VIDEO\n  bandwidth percent 20\n class WAN-BULK\n  bandwidth percent 5\n");
        let commands = reaches(WAN, &grown);
        assert!(!commands.iter().any(|line| line.starts_with("no ")), "{commands:?}");
        let shrunk = WAN
            .replace("class-map match-any WAN-VIDEO\n match dscp af41 af42 af43\n", "")
            .replace(" class WAN-VIDEO\n  bandwidth percent 20\n", "");
        let commands = reaches(WAN, &shrunk);
        let at = |line: &str| commands.iter().position(|sent| sent == line).unwrap();
        assert!(at("no class WAN-VIDEO") < at("no class-map match-any WAN-VIDEO"), "{commands:?}");
        assert!(!commands.iter().any(|line| line.starts_with("no policy-map")), "{commands:?}");
    }

    #[test]
    fn a_policy_whose_classes_change_order_is_taken_off_typed_again_and_put_back() {
        let reordered = WAN.replace(
            " class WAN-VOICE\n  priority percent 10\n class WAN-VIDEO\n  bandwidth percent 20\n",
            " class WAN-VIDEO\n  bandwidth percent 20\n class WAN-VOICE\n  priority percent 10\n",
        );
        let result = remediate(WAN, &reordered, Syntax::Ios);
        let commands = reaches(WAN, &reordered);
        let at = |line: &str| commands.iter().position(|sent| sent == line).unwrap();
        // WAN is the child of WAN-SHAPE, which stays: it is taken out of the
        // parent's class, not off the interface.
        assert!(at("no service-policy WAN") < at("no policy-map WAN"), "{commands:?}");
        assert!(at("no policy-map WAN") < at("policy-map WAN"), "{commands:?}");
        assert!(at("policy-map WAN") < at("service-policy WAN"), "{commands:?}");
        assert!(!commands.contains(&"no service-policy output WAN-SHAPE".to_string()), "{commands:?}");
        assert!(
            result.warnings.iter().any(|warning| warning.contains("`policy-map WAN` is removed and typed again")),
            "{:?}",
            result.warnings
        );
    }

    #[test]
    fn a_class_map_that_changes_takes_the_policies_that_use_it_with_it() {
        // Taking one value out of a statement takes it out of every statement
        // that lists it, so a changed class map is typed again, and the
        // device only lets it go once no policy names it.
        let moved = WAN.replace(" match dscp ef\n", " match dscp cs5\n");
        let commands = reaches(WAN, &moved);
        let at = |line: &str| commands.iter().position(|sent| sent == line).unwrap();
        assert!(at("no policy-map WAN") < at("no class-map match-any WAN-VOICE"), "{commands:?}");
        assert!(at("no class-map match-any WAN-VOICE") < at("class-map match-any WAN-VOICE"), "{commands:?}");
        // An added statement is just added.
        let more = WAN.replace(" match dscp ef\n", " match dscp ef\n match dscp cs5\n");
        assert_eq!(
            reaches(WAN, &more),
            strings(&["class-map match-any WAN-VOICE", "match dscp cs5", "exit"])
        );
    }

    #[test]
    fn a_second_policy_in_one_direction_goes_on_only_after_the_first_is_off() {
        let running = "policy-map A\n class class-default\n  shape average 8000000\npolicy-map B\n class class-default\n  shape average 9000000\ninterface Gi0/1\n service-policy output A\n";
        let intended = running.replace("service-policy output A", "service-policy output B");
        let result = remediate(running, &intended, Syntax::Ios);
        let commands = reaches(running, &intended);
        let at = |line: &str| commands.iter().position(|sent| sent == line).unwrap();
        assert!(at("no service-policy output A") < at("service-policy output B"), "{commands:?}");
        assert!(
            result.warnings.iter().any(|warning| warning.contains("no output policy applies there")),
            "{:?}",
            result.warnings
        );
    }

    #[test]
    fn a_line_the_device_refuses_for_a_qos_reason_is_named_with_the_reason() {
        let refusal = |commands: &[&str]| qos_refusal(WAN, &strings(commands));
        let refused = refusal(&["no policy-map WAN-SHAPE"]).expect("in use on the interface");
        assert_eq!(refused.index, 0);
        assert!(refused.message.contains("interface GigabitEthernet0/1"), "{}", refused.message);
        let refused = refusal(&["no policy-map WAN"]).expect("in use inside WAN-SHAPE");
        assert!(refused.message.contains("policy-map WAN-SHAPE"), "{}", refused.message);
        let refused = refusal(&["no class-map match-any WAN-VOICE"]).expect("in use");
        assert!(refused.message.contains("policy-map WAN"), "{}", refused.message);
        let refused = refusal(&[
            "policy-map OTHER",
            "class class-default",
            "shape average 8000000",
            "interface GigabitEthernet0/1",
            "service-policy output OTHER",
        ])
        .expect("a second output policy");
        assert_eq!(refused.index, 4);
        assert!(refused.message.contains("no service-policy output WAN-SHAPE"), "{}", refused.message);
        assert!(refusal(&["interface GigabitEthernet0/1", "service-policy input NOPE"]).is_some());
        assert!(refusal(&["policy-map X", "class NOPE"]).is_some());
        assert!(refusal(&["policy-map X", "class class-default", "service-policy NOPE"]).is_some());
        // In an order the device takes, the same lines are fine.
        assert_eq!(
            refusal(&[
                "policy-map OTHER",
                "class class-default",
                "shape average 8000000",
                "interface GigabitEthernet0/1",
                "no service-policy output WAN-SHAPE",
                "service-policy output OTHER",
                "exit",
                "no policy-map WAN-SHAPE",
                "no policy-map WAN",
                "no class-map match-any WAN-VOICE",
            ]),
            None
        );
    }

    #[test]
    fn a_child_policy_applies_where_its_parent_is_attached() {
        let tree = ConfigTree::parse(WAN, Syntax::Ios);
        let attached = Attached {
            section: "interface GigabitEthernet0/1".into(),
            output: true,
            policy: "WAN-SHAPE".into(),
        };
        assert_eq!(tree.qos_attached(), vec![attached.clone()]);
        assert_eq!(tree.qos_applies("WAN"), vec![attached.clone()]);
        assert_eq!(tree.qos_applies("WAN-SHAPE"), vec![attached]);
        assert_eq!(
            tree.qos_classes("WAN-SHAPE"),
            Some(vec![("class-default".to_string(), Some("WAN".to_string()))])
        );
        assert_eq!(tree.qos_classes("NOPE"), None);
    }
}
