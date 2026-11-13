//! Deciding what a command does before it is sent.
//!
//! Every command that does not come straight from a human typing in a
//! terminal — the AI's, a playbook's, a scheduled job's — is classified first.
//! The rule is default-deny: a command is read-only only if it matches the
//! platform's read-only list and does nothing sneaky afterwards (writing to a
//! file through a pipe, chaining a second command). Everything else is a
//! change, and a short list of commands is destructive enough to be called
//! out separately.

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandClass {
    /// Only displays state.
    ReadOnly,
    /// Changes device state or configuration.
    Change,
    /// Disruptive or destructive: reload, erase, format, factory reset…
    Dangerous,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Classification {
    pub class: CommandClass,
    /// Why, in words a user can read.
    pub reason: String,
}

/// A platform's command policy, compiled from its profile.
#[derive(Clone, Debug)]
pub struct Policy {
    read_only: Vec<Regex>,
    dangerous: Vec<Regex>,
}

/// Output modifiers that write somewhere instead of just filtering.
const WRITING_PIPES: &[&str] = &["redirect", "tee", "append", "save", "file", "copy", "email"];

impl Policy {
    /// Patterns are matched case-insensitively against the trimmed command.
    pub fn new<S: AsRef<str>>(read_only: &[S], dangerous: &[S]) -> Result<Policy, regex::Error> {
        let compile = |patterns: &[S]| -> Result<Vec<Regex>, regex::Error> {
            patterns
                .iter()
                .map(|pattern| Regex::new(&format!("(?i){}", pattern.as_ref())))
                .collect()
        };
        Ok(Policy {
            read_only: compile(read_only)?,
            dangerous: compile(dangerous)?,
        })
    }

    /// A conservative policy for platforms without a profile-specific one.
    pub fn generic() -> Policy {
        Policy::new(GENERIC_READ_ONLY, GENERIC_DANGEROUS)
            .expect("generic policy patterns are valid")
    }

    /// The Cisco-style policy, tolerant of the abbreviations those CLIs accept.
    pub fn ios() -> Policy {
        Policy::new(IOS_READ_ONLY, IOS_DANGEROUS).expect("ios policy patterns are valid")
    }

    pub fn classify(&self, command: &str) -> Classification {
        let command = command.trim();
        if command.is_empty() {
            return Classification {
                class: CommandClass::ReadOnly,
                reason: "empty line".into(),
            };
        }
        // Several commands on one line are as risky as the riskiest of them.
        let mut worst = Classification {
            class: CommandClass::ReadOnly,
            reason: String::new(),
        };
        for part in split_commands(command) {
            let verdict = self.classify_one(part);
            if verdict.class > worst.class || worst.reason.is_empty() {
                worst = verdict;
            }
        }
        worst
    }

    pub fn is_read_only(&self, command: &str) -> bool {
        self.classify(command).class == CommandClass::ReadOnly
    }

    fn classify_one(&self, command: &str) -> Classification {
        let (head, pipes) = split_pipes(command);
        if self.dangerous.iter().any(|pattern| pattern.is_match(head)) {
            return Classification {
                class: CommandClass::Dangerous,
                reason: format!("`{head}` is disruptive or destructive"),
            };
        }
        if !self.read_only.iter().any(|pattern| pattern.is_match(head)) {
            return Classification {
                class: CommandClass::Change,
                reason: format!("`{head}` is not on this platform's read-only list"),
            };
        }
        for pipe in pipes {
            let verb = pipe
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if WRITING_PIPES
                .iter()
                .any(|writer| writer.starts_with(&verb) && verb.len() >= 3.min(writer.len()))
            {
                return Classification {
                    class: CommandClass::Change,
                    reason: format!("the output is written somewhere by `| {pipe}`"),
                };
            }
        }
        if head.contains('>') && !head.contains("->") {
            return Classification {
                class: CommandClass::Change,
                reason: "the output is redirected to a file".into(),
            };
        }
        Classification {
            class: CommandClass::ReadOnly,
            reason: "read-only command".into(),
        }
    }
}

/// Split on `;` and newlines outside quotes.
fn split_commands(command: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quote: Option<char> = None;
    for (index, ch) in command.char_indices() {
        match (quote, ch) {
            (Some(open), _) if ch == open => quote = None,
            (None, '"' | '\'') => quote = Some(ch),
            (None, ';' | '\n' | '\r') => {
                parts.push(&command[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&command[start..]);
    parts
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

/// `show run | include vlan | count` → (`show run`, [`include vlan`, `count`]).
fn split_pipes(command: &str) -> (&str, Vec<&str>) {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quote: Option<char> = None;
    for (index, ch) in command.char_indices() {
        match (quote, ch) {
            (Some(open), _) if ch == open => quote = None,
            (None, '"' | '\'') => quote = Some(ch),
            (None, '|') => {
                parts.push(command[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(command[start..].trim());
    let head = parts.remove(0);
    (head, parts)
}

const GENERIC_READ_ONLY: &[&str] = &[
    r"^(show|display|get|print|list|ping|traceroute|tracert|dir|more|cat|ls|pwd|whoami|uptime|uname|df|free)\b",
    r"^terminal (length|width|pager)\b",
    r"^(screen-length|set cli screen-length|set cli screen-width)\b",
];

const GENERIC_DANGEROUS: &[&str] = &[
    r"^(reload|reboot|restart|shutdown|halt|poweroff|format|erase|delete|rm|mkfs|dd)\b",
    r"^write erase\b",
    r"^request system (reboot|halt|power-off|zeroize)\b",
    r"^/?system (reboot|shutdown|reset-configuration)\b",
    r"^(factory-reset|reset saved-configuration|execute factoryreset)\b",
];

// IOS-style CLIs accept any unambiguous abbreviation, so the patterns do too.
const IOS_READ_ONLY: &[&str] = &[
    r"^sh(o(w)?)?\s+\S",
    r"^pi(n(g)?)?\s+\S",
    r"^tr(a(c(e(r(o(u(t(e)?)?)?)?)?)?)?)?\s+\S",
    r"^dir\b",
    r"^more\s+\S",
    r"^ter(m(i(n(a(l)?)?)?)?)?\s+(len(g(t(h)?)?)?|wid(t(h)?)?|no mon(i(t(o(r)?)?)?)?|mon(i(t(o(r)?)?)?)?)\b",
    r"^(en(a(b(l(e)?)?)?)?|disa(b(l(e)?)?)?|exit|end|logout)$",
    r"^verify\s+\S",
];

const IOS_DANGEROUS: &[&str] = &[
    r"^rel(o(a(d)?)?)?\b",
    r"^er(a(s(e)?)?)?\b",
    r"^wr(i(t(e)?)?)?\s+e(r(a(s(e)?)?)?)?\b",
    r"^for(m(a(t)?)?)?\b",
    r"^del(e(t(e)?)?)?\b",
    r"^sq(u(e(e(z(e)?)?)?)?)?\b",
    r"^(rmdir|mkdir)\b",
    r"^conf(i(g(u(r(e)?)?)?)?)?\s+rep(l(a(c(e)?)?)?)?\b",
    r"^boot\s+system\b",
    r"^(test|hw-module|power|redundancy force-switchover|install|request platform software)\b",
    r"^cl(e(a(r)?)?)?\s+(line|ip bgp|ip ospf|crypto|vtp)\b",
    // Copying *onto* the device (a file over the config, an image onto flash).
    // Saving (`copy running-config startup-config`) is an ordinary change.
    r"^copy\s+(tftp|ftp|scp|sftp|https?|rcp|usb\S*|flash|bootflash|disk\d|slot\d|nvram)\S*\s+\S",
    r"^archive\s+download-sw\b",
    r"^(tclsh|guestshell|bash|run bash|shell|start shell|app-hosting)\b",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn class(command: &str) -> CommandClass {
        Policy::ios().classify(command).class
    }

    #[test]
    fn show_commands_and_their_abbreviations_are_read_only() {
        for command in [
            "show running-config",
            "sh ip int br",
            "sho ver",
            "show interfaces status | include connected",
            "show run | section router ospf",
            "ping 10.0.0.1",
            "traceroute 8.8.8.8",
            "terminal length 0",
            "dir flash:",
            "  show vlan brief  ",
        ] {
            assert_eq!(class(command), CommandClass::ReadOnly, "{command}");
        }
    }

    #[test]
    fn anything_unknown_is_a_change() {
        for command in [
            "configure terminal",
            "conf t",
            "write memory",
            "wr",
            "copy running-config startup-config",
            "copy run start",
            "copy running-config tftp://10.0.0.5/backup.cfg",
            "clear counters",
            "interface Gi0/1",
            "shutdown",
            "vlan 30",
            "debug ip packet",
            "show",
        ] {
            assert_eq!(class(command), CommandClass::Change, "{command}");
        }
    }

    #[test]
    fn destructive_commands_are_flagged_even_abbreviated() {
        for command in [
            "reload",
            "rel in 5",
            "erase startup-config",
            "er st",
            "write erase",
            "wr e",
            "format flash:",
            "delete /force flash:config.text",
            "del vlan.dat",
            "configure replace flash:backup.cfg",
            "copy tftp://10.0.0.5/x.cfg startup-config",
            "copy flash:old.cfg running-config",
            "copy scp://user@10.0.0.5/ios.bin flash:",
            "tclsh",
            "clear line 2",
        ] {
            assert_eq!(class(command), CommandClass::Dangerous, "{command}");
        }
    }

    #[test]
    fn pipes_that_write_and_chained_commands_are_not_read_only() {
        assert_eq!(
            class("show running-config | redirect tftp://10.0.0.5/leak.cfg"),
            CommandClass::Change
        );
        assert_eq!(
            class("show tech-support | tee flash:tech.txt"),
            CommandClass::Change
        );
        assert_eq!(class("show tech | append flash:x"), CommandClass::Change);
        assert_eq!(class("show version ; reload"), CommandClass::Dangerous);
        assert_eq!(
            class("show version\nconfigure terminal"),
            CommandClass::Change
        );
        // A pipe or semicolon inside a quoted regex is just text.
        assert_eq!(
            class("show logging | include \"err|crit;\""),
            CommandClass::ReadOnly
        );
    }

    #[test]
    fn generic_policy_covers_other_vendors_basics() {
        let policy = Policy::generic();
        assert!(policy.is_read_only("display current-configuration"));
        assert!(policy.is_read_only("get system status"));
        assert!(policy.is_read_only("show configuration | display set"));
        assert_eq!(
            policy.classify("request system reboot").class,
            CommandClass::Dangerous
        );
        assert_eq!(
            policy.classify("/system reset-configuration").class,
            CommandClass::Dangerous
        );
        assert_eq!(
            policy.classify("commit confirmed 5").class,
            CommandClass::Change
        );
        assert_eq!(
            policy
                .classify("show configuration | save /var/tmp/x")
                .class,
            CommandClass::Change
        );
    }

    #[test]
    fn reasons_name_the_offending_part() {
        let verdict = Policy::ios().classify("show clock ; write erase");
        assert_eq!(verdict.class, CommandClass::Dangerous);
        assert!(verdict.reason.contains("write erase"));
    }
}
