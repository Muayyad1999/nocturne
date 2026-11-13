//! Keeping device secrets out of the model's context.
//!
//! Device output and configuration pass through a [`Redactor`] before they are
//! shown to a model. Every secret is replaced by a stable placeholder such as
//! `<secret:3>`: the same value always maps to the same placeholder, so the
//! model can still reason about equality ("both switches use the same RADIUS
//! key") without ever seeing the value. When the model writes configuration
//! that contains a placeholder, [`Redactor::restore`] puts the real value back
//! just before the lines are sent to the device — so a model can move or
//! re-apply a line containing a secret it was never shown.
//!
//! Over-redaction is the safe failure mode: a non-secret word hidden behind a
//! placeholder costs nothing, a leaked credential cannot be taken back.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Values so well known that telling the model "this is a default" leaks
/// nothing and lets it flag the weakness.
const NOTORIOUS: &[&str] = &[
    "public",
    "private",
    "cisco",
    "admin",
    "password",
    "pass",
    "1234",
    "12345",
    "123456",
    "default",
    "secret",
    "test",
    "community",
    "changeme",
    "letmein",
    "root",
    "guest",
    "manager",
    "monitor",
    "read",
    "write",
    "ubnt",
    "mikrotik",
    "class",
    "enable",
    "router",
    "switch",
];

/// Built-in patterns. Each captures the secret itself in a group named
/// `secret`; everything else on the line stays visible.
const BUILTIN_RULES: &[&str] = &[
    // --- generic, multi-line -------------------------------------------------
    r"(?P<secret>-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----)",
    // --- IOS / IOS-XE / NX-OS / EOS style -------------------------------------
    r"(?im)^\s*enable (?:secret|password)(?: level \d+)?(?: (?:\d|sha512|sha256|md5))? (?P<secret>\S+)",
    r"(?im)^\s*username \S+ .*?(?:secret|password)(?: (?:\d|sha512|sha256|md5))? (?P<secret>\S+)",
    r"(?im)\b(?:password|secret) (?:[0-9]|sha512|sha256|md5|encrypted) (?P<secret>\S+)",
    r#"(?im)^\s*(?:password|secret) (?P<secret>[^\s";]+)\s*$"#,
    r#"(?im)\bsnmp-server community (?P<secret>"[^"]+"|\S+)"#,
    r"(?im)\bsnmp-server host \S+(?: vrf \S+)?(?: (?:traps|informs))?(?: version (?:1|2c|3 (?:auth|noauth|priv)))? (?P<secret>\S+)",
    r"(?im)\bauth (?:md5|sha\S*) (?P<secret>\S+)",
    r"(?im)\bpriv (?:(?:des|3des|aes)(?:[ -]?\d+)? )?(?P<secret>\S+)",
    r"(?im)\b(?:tacacs|radius)(?:-server| server)\b.*?\bkey(?: \d)? (?P<secret>\S+)",
    r"(?im)^\s*key (?:\d )?(?P<secret>\S{4,})\s*$",
    r"(?im)\bkey-string(?: \d)? (?P<secret>\S+)",
    r"(?im)\bpre-shared-key(?: (?:local|remote))?(?: \d)? (?P<secret>\S+)",
    r"(?im)\bcrypto isakmp key(?: \d)? (?P<secret>\S+)",
    r"(?im)\bneighbor \S+ password(?: \d)? (?P<secret>\S+)",
    r"(?im)\bmessage-digest-key \d+ md5(?: \d)? (?P<secret>\S+)",
    r"(?im)\bauthentication-key(?: \d)? (?P<secret>\S+)",
    r"(?im)\b(?:standby|vrrp|glbp) \d+ authentication(?: text| md5 key-string)?(?: \d)? (?P<secret>\S+)",
    r"(?im)\bntp authentication-key \d+ md5 (?P<secret>\S+)",
    r"(?im)\bppp (?:chap|pap) (?:password|sent-username \S+ password)(?: \d)? (?P<secret>\S+)",
    r"(?im)\bwpa-psk (?:ascii|hex)(?: \d)? (?P<secret>\S+)",
    r"(?im)\bcommunity(?: name| string)?\s*[:=]\s*(?P<secret>\S+)",
    // --- Junos -----------------------------------------------------------------
    r#"(?im)\b(?:encrypted-password|secret|authentication-key|simple-password|md5-key|key|passphrase)\s+"(?P<secret>[^"]+)""#,
    r#"(?im)\bpre-shared-key (?:ascii-text|hexadecimal) "(?P<secret>[^"]+)""#,
    r"(?im)\bsnmp community (?P<secret>\S+)",
    r"(?im)^\s*community (?P<secret>[^\s{;]+)\s*\{",
    // --- RouterOS, and key=value styles in general -----------------------------
    r#"(?im)\b(?:password|secret|passphrase|pre-shared-key|preshared-key|psk|private-key|wpa2?-pre-shared-key|authentication-password|encryption-password|auth-key|api-key|token)=(?P<secret>"[^"]*"|[^\s"]+)"#,
    r#"(?im)^/snmp community\b.*?\bname=(?P<secret>"[^"]*"|\S+)"#,
    r#"(?im)^(?:add|set)\b.*?\bcommunity=(?P<secret>"[^"]*"|\S+)"#,
    // --- FortiOS ---------------------------------------------------------------
    r"(?im)^\s*set (?:password|passwd|psksecret|secret|key|passphrase|auth-pwd|priv-pwd|ppk-secret|group-password|keystring) (?:ENC )?(?P<secret>\S+)",
    // --- Huawei VRP / H3C Comware ----------------------------------------------
    r"(?im)\b(?:password|key|authentication-mode \S+) (?:cipher|simple|irreversible-cipher|hash) (?P<secret>\S+)",
    r"(?im)\bsnmp-agent community (?:read|write) (?:cipher |simple )?(?P<secret>\S+)",
    r"(?P<secret>%\^%#.+?%\^%#)",
    r"(?P<secret>%@%@.+?%@%@)",
    r"(?P<secret>\$c\$\d\$\S+)",
    // --- PAN-OS and other XML ----------------------------------------------------
    r"(?i)<(?:phash|password|key|private-key|secret|auth-password|priv-password|bind-password|psk)>(?P<secret>[^<]+)</",
    // --- hashes, tokens and cloud keys wherever they appear -----------------------
    r#"(?P<secret>\$(?:1|5|6|8|9|y|2[aby])\$[^\s";<]{8,})"#,
    r"\b(?P<secret>AKIA[0-9A-Z]{16})\b",
    r"\b(?P<secret>eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})",
    r"(?im)\bauthorization:\s*(?:bearer|basic)\s+(?P<secret>\S+)",
    r#"(?im)\bx?-?api[_-]?key["']?\s*[:=]\s*["']?(?P<secret>[^\s"',]+)"#,
];

static BUILTIN: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    BUILTIN_RULES
        .iter()
        .map(|pattern| Regex::new(pattern).expect("built-in redaction rule must compile"))
        .collect()
});

static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<secret:(\d+)(?::weak)?>").expect("token pattern must compile"));

#[derive(Default)]
struct Table {
    by_value: HashMap<String, u32>,
    by_id: HashMap<u32, String>,
    next: u32,
}

impl Table {
    fn token_for(&mut self, value: &str) -> String {
        let id = match self.by_value.get(value) {
            Some(id) => *id,
            None => {
                self.next += 1;
                self.by_value.insert(value.to_string(), self.next);
                self.by_id.insert(self.next, value.to_string());
                self.next
            }
        };
        if NOTORIOUS.contains(&value.to_ascii_lowercase().as_str()) {
            format!("<secret:{id}:weak>")
        } else {
            format!("<secret:{id}>")
        }
    }
}

/// The placeholder table, for carrying a conversation across restarts.
/// It contains the secrets themselves: store it only in the encrypted vault.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct RedactionTable {
    pub entries: Vec<(u32, String)>,
}

/// Result of putting secrets back into model-written text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    pub text: String,
    /// Placeholders that did not resolve (from another session, or invented by the model).
    pub unresolved: Vec<String>,
}

pub struct Redactor {
    extra: Vec<Regex>,
    table: Mutex<Table>,
}

impl Default for Redactor {
    fn default() -> Self {
        Redactor::new()
    }
}

impl Redactor {
    pub fn new() -> Redactor {
        Redactor {
            extra: Vec::new(),
            table: Mutex::new(Table::default()),
        }
    }

    /// Add platform-specific rules (from device profiles). A rule should name
    /// its secret group `secret`; without one the whole match is hidden.
    pub fn with_rules<S: AsRef<str>>(patterns: &[S]) -> Result<Redactor, regex::Error> {
        let extra = patterns
            .iter()
            .map(|pattern| Regex::new(pattern.as_ref()))
            .collect::<Result<_, _>>()?;
        Ok(Redactor {
            extra,
            table: Mutex::new(Table::default()),
        })
    }

    /// Replace every secret in `text` with its placeholder.
    pub fn redact(&self, text: &str) -> String {
        let mut table = self.lock();
        let mut current = text.to_string();
        for rule in self.extra.iter().chain(BUILTIN.iter()) {
            if rule.is_match(&current) {
                current = apply(rule, &current, &mut table);
            }
        }
        current
    }

    /// Hide one known value (a vault credential, an API key) wherever it appears.
    pub fn redact_value(&self, text: &str, value: &str) -> String {
        if value.len() < 4 || !text.contains(value) {
            return text.to_string();
        }
        let token = self.lock().token_for(value);
        text.replace(value, &token)
    }

    /// Put real values back in place of placeholders.
    pub fn restore(&self, text: &str) -> Restored {
        let table = self.lock();
        let mut unresolved = Vec::new();
        let restored = TOKEN.replace_all(text, |caps: &regex::Captures<'_>| {
            let id = caps[1].parse::<u32>().ok();
            match id.and_then(|id| table.by_id.get(&id)) {
                Some(value) => value.clone(),
                None => {
                    unresolved.push(caps[0].to_string());
                    caps[0].to_string()
                }
            }
        });
        Restored {
            text: restored.into_owned(),
            unresolved,
        }
    }

    pub fn has_placeholders(text: &str) -> bool {
        TOKEN.is_match(text)
    }

    pub fn export(&self) -> RedactionTable {
        let table = self.lock();
        let mut entries: Vec<(u32, String)> = table
            .by_id
            .iter()
            .map(|(id, value)| (*id, value.clone()))
            .collect();
        entries.sort_by_key(|(id, _)| *id);
        RedactionTable { entries }
    }

    pub fn import(&self, saved: RedactionTable) {
        let mut table = self.lock();
        for (id, value) in saved.entries {
            table.next = table.next.max(id);
            table.by_value.insert(value.clone(), id);
            table.by_id.insert(id, value);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Table> {
        // A poisoned table is still a valid table; redaction must keep working.
        self.table
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn apply(rule: &Regex, text: &str, table: &mut Table) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for caps in rule.captures_iter(text) {
        let Some(found) = caps.name("secret").or_else(|| caps.get(0)) else {
            continue;
        };
        if found.start() < last {
            continue;
        }
        let raw = found.as_str();
        let (value, quoted) = match raw
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
        {
            Some(inner) if raw.len() >= 2 => (inner, true),
            _ => (raw, false),
        };
        if value.is_empty() || already_masked(value) {
            continue;
        }
        out.push_str(&text[last..found.start()]);
        let token = table.token_for(value);
        if quoted {
            out.push('"');
            out.push_str(&token);
            out.push('"');
        } else {
            out.push_str(&token);
        }
        last = found.end();
    }
    out.push_str(&text[last..]);
    out
}

/// Values the device (or an earlier pass) has already hidden.
fn already_masked(value: &str) -> bool {
    value.contains("<secret:")
        || value.chars().all(|c| c == '*' || c == 'x' || c == 'X')
        || matches!(value, "<removed>" | "<hidden>" | "<redacted>" | "********")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redact(text: &str) -> String {
        Redactor::new().redact(text)
    }

    #[test]
    fn hides_ios_secrets_but_keeps_structure() {
        let config = "\
hostname CORE-1
enable secret 9 $9$abcDEFghiJKL$mnopqrstuvwxyz0123456789ABCDEFGHIJ
username admin privilege 15 secret 5 $1$salt$hashhashhashhashhashha
username backup password 7 0822455D0A16
snmp-server community S3cr3tRO RO
snmp-server host 10.0.0.9 version 2c TrapComm
tacacs-server host 10.1.1.1 key 7 094F471A1A0A
line vty 0 4
 password 7 030752180500
 login
router bgp 65001
 neighbor 192.0.2.1 password BgpSecret!
interface Vlan10
 ip ospf message-digest-key 1 md5 OspfKey123
 standby 10 authentication md5 key-string HsrpKey
";
        let out = redact(config);
        for leaked in [
            "$9$abc",
            "$1$salt",
            "0822455D0A16",
            "S3cr3tRO",
            "TrapComm",
            "094F471A1A0A",
            "030752180500",
            "BgpSecret!",
            "OspfKey123",
            "HsrpKey",
        ] {
            assert!(!out.contains(leaked), "leaked {leaked} in:\n{out}");
        }
        assert!(out.contains("hostname CORE-1"));
        assert!(out.contains("snmp-server community <secret:"));
        assert!(out.contains(" RO\n"));
        assert!(out.contains("username admin privilege 15 secret 5 <secret:"));
        assert!(out.contains(" neighbor 192.0.2.1 password <secret:"));
        assert!(out.contains(" login\n"));
    }

    #[test]
    fn same_value_gets_same_placeholder_and_restores() {
        let redactor = Redactor::new();
        let a = redactor.redact("snmp-server community Shared123 RO");
        let b = redactor
            .redact("snmp-server community Shared123 RW\nsnmp-server community Other456 RO");
        let token = a.split_whitespace().nth(2).unwrap().to_string();
        assert!(b.starts_with(&format!("snmp-server community {token} RW")));
        assert!(!b.lines().nth(1).unwrap().contains(&token));

        let restored = redactor.restore(&format!(
            "snmp-server community {token} RO\nno snmp-server community <secret:99>"
        ));
        assert_eq!(
            restored.text,
            "snmp-server community Shared123 RO\nno snmp-server community <secret:99>"
        );
        assert_eq!(restored.unresolved, vec!["<secret:99>".to_string()]);
    }

    #[test]
    fn marks_notorious_defaults_as_weak() {
        let redactor = Redactor::new();
        let out = redactor.redact("snmp-server community public RO");
        assert_eq!(out, "snmp-server community <secret:1:weak> RO");
        assert_eq!(
            redactor.restore(&out).text,
            "snmp-server community public RO"
        );
    }

    #[test]
    fn hides_routeros_junos_fortios_huawei_and_xml_secrets() {
        let text = r#"
/ppp secret add name=user1 password="p@ss word" service=pppoe
/interface wireless security-profiles set default wpa2-pre-shared-key=WifiKey99
/snmp community add name=RosComm addresses=10.0.0.0/8
set system root-authentication encrypted-password "$6$abc$defghijklmnop"
set snmp community JunComm authorization read-only
    set psksecret ENC AbCdEfGh1234==
 local-user admin password irreversible-cipher $1c$xyz$0123456789
 snmp-agent community read cipher %^%#abcDEF123%^%#
<phash>$1$abcd$efghijklmnopqrstu</phash>
"#;
        let out = redact(text);
        for leaked in [
            "p@ss word",
            "WifiKey99",
            "RosComm",
            "$6$abc",
            "JunComm",
            "AbCdEfGh1234==",
            "$1c$xyz",
            "abcDEF123",
            "$1$abcd",
        ] {
            assert!(!out.contains(leaked), "leaked {leaked} in:\n{out}");
        }
        assert!(out.contains(r#"password="<secret:"#), "{out}");
        assert!(out.contains("service=pppoe"));
        assert!(out.contains("authorization read-only"));
    }

    #[test]
    fn hides_private_keys_and_tokens() {
        let text = "-----BEGIN RSA PRIVATE KEY-----\nMIIEow\nIBAAK\n-----END RSA PRIVATE KEY-----\nAuthorization: Bearer abc.def.ghi\nkey AKIAIOSFODNN7EXAMPLE";
        let out = redact(text);
        assert!(!out.contains("MIIEow"));
        assert!(!out.contains("abc.def.ghi"));
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn leaves_ordinary_config_alone() {
        let config = "interface GigabitEthernet0/1\n description uplink to core\n switchport mode trunk\n spanning-tree portfast\nservice password-encryption\nip route 0.0.0.0 0.0.0.0 10.0.0.1\n";
        assert_eq!(redact(config), config);
    }

    #[test]
    fn does_not_double_redact_or_touch_device_masks() {
        let redactor = Redactor::new();
        let once = redactor.redact("enable secret 5 $1$salt$hashhashhashhashhashha");
        assert_eq!(redactor.redact(&once), once);
        assert_eq!(
            redact("username admin password ********"),
            "username admin password ********"
        );
    }

    #[test]
    fn extra_rules_and_known_values() {
        let redactor = Redactor::with_rules(&[r"(?m)^magic-token (?P<secret>\S+)"]).unwrap();
        assert_eq!(
            redactor.redact("magic-token Zeta9"),
            "magic-token <secret:1>"
        );
        assert_eq!(
            redactor.redact_value("login with Hunter2Pass now", "Hunter2Pass"),
            "login with <secret:2> now"
        );
    }

    #[test]
    fn table_round_trips_through_export() {
        let first = Redactor::new();
        let hidden =
            first.redact("snmp-server community Alpha1 RO\nsnmp-server community Beta22 RW");
        let second = Redactor::new();
        second.import(first.export());
        assert_eq!(
            second.restore(&hidden).text,
            "snmp-server community Alpha1 RO\nsnmp-server community Beta22 RW"
        );
        // New secrets continue numbering after the imported ones.
        assert_eq!(
            second.redact("snmp-server community Gamma3 RO"),
            "snmp-server community <secret:3> RO"
        );
    }
}
