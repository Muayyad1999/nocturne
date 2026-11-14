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
    // These are not anchored to the start of a line on purpose: a device
    // quotes a configuration command in the middle of a log entry
    // (`%PARSER-5-CFGLOG_LOGGEDCMD: … logged command:enable secret 5 …`,
    // or a FortiGate `msg="… set psksecret ENC …"`), and an anchored rule
    // walks straight past the secret. The keywords are distinctive enough
    // to be safe anywhere; the ones that are only a word and a value stay
    // anchored below, because those would fire on prose.
    r"(?im)\benable (?:secret|password)(?: level \d+)?(?: (?:\d|sha512|sha256|md5))? (?P<secret>\S+)",
    r"(?im)\busername \S+ .*?(?:secret|password)(?: (?:\d|sha512|sha256|md5))? (?P<secret>\S+)",
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
    // The algorithm may stand between the key id and the key itself
    // (`ntp authentication-key 1 md5 <key> 7`). Without it in the pattern the
    // algorithm is masked and the key is left in the clear.
    r"(?im)\bauthentication-key(?: \d+)?(?: (?:md5|sha1?|sha256|sha512|hmac-sha1|hmac-sha256|aes|cmac))? (?P<secret>\S+)",
    r"(?im)\b(?:standby|vrrp|glbp) \d+ authentication(?: text| md5 key-string)?(?: \d)? (?P<secret>\S+)",
    r"(?im)\bppp (?:chap|pap) (?:password|sent-username \S+ password)(?: \d)? (?P<secret>\S+)",
    r"(?im)\bwpa-psk (?:ascii|hex)(?: \d)? (?P<secret>\S+)",
    // `set-key ascii0 <key>` (Aruba, HPE); the generic password rule misses
    // it because there is no encoding digit after a keyword it knows.
    r"(?im)\bset-key (?:ascii|hex)\d*(?: \d)? (?P<secret>\S+)",
    // `ip ftp password <password>` (Cisco) — likewise no encoding digit.
    r"(?im)\bip (?:ftp|scp|tftp) password(?: \d)? (?P<secret>\S+)",
    r"(?im)\bcommunity(?: name| string)?\s*[:=]\s*(?P<secret>\S+)",
    // --- Junos -----------------------------------------------------------------
    r#"(?im)\b(?:encrypted-password|secret|authentication-key|simple-password|md5-key|key|passphrase)\s+"(?P<secret>[^"]+)""#,
    r#"(?im)\bpre-shared-key (?:ascii-text|hexadecimal) "(?P<secret>[^"]+)""#,
    // The access level may stand between the keyword and the community
    // (`set snmp community read-only Secret`, CatOS). Without it in the
    // pattern, `read-only` is masked and the community is left in the clear.
    r"(?im)\bsnmp community (?:(?:read-only|read-write|ro|rw) )?(?P<secret>\S+)",
    r"(?im)^\s*community (?P<secret>[^\s{;]+)\s*\{",
    // --- RouterOS, and key=value styles in general -----------------------------
    r#"(?im)\b(?:password|secret|passphrase|pre-shared-key|preshared-key|psk|private-key|wpa2?-pre-shared-key|authentication-password|encryption-password|auth-key|api-key|token)=(?P<secret>"[^"]*"|[^\s"]+)"#,
    r#"(?im)/snmp community\b.*?\bname=(?P<secret>"[^"]*"|\S+)"#,
    r#"(?im)\b(?:add|set)\b[^\n]*?\bcommunity=(?P<secret>"[^"]*"|\S+)"#,
    // --- FortiOS ---------------------------------------------------------------
    r"(?im)\bset (?:password|passwd|psksecret|secret|key|passphrase|auth-pwd|priv-pwd|ppk-secret|group-password|keystring) (?:ENC )?(?P<secret>\S+)",
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
        if value.is_empty() || already_masked(value) || is_a_marker(value) {
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
/// A format or key number that a pattern caught by mistake, not a secret.
///
/// Rules come from every installed device type at once, because one chat can
/// touch several platforms and a secret only has to look like a secret once.
/// The cost is that a careless pattern in *any* profile acts on *every*
/// vendor's configuration — and one did. Junos's `authentication-key (\S+)`
/// matched the IOS line `ntp authentication-key 1 md5 N0cturneNtpKey` and
/// captured the key *number*, `1`.
///
/// That is not a harmless extra entry. The table is numbered in the order
/// entries are made, so a spurious one shifts every placeholder after it —
/// and `<secret:1>` in a line the model wrote then restores a different
/// secret from the one it meant. In the test that found this, that put a real
/// NTP key into an interface description, where no pattern matches it on the
/// way back, and the model read it.
///
/// So a short run of digits is never taken for a secret. Nothing is lost:
/// a number of five digits or fewer carries almost nothing, and these markers
/// (`secret 5`, `password 7`, `authentication-key 1`) are exactly what such a
/// capture is in practice.
fn is_a_marker(value: &str) -> bool {
    value.len() < 6 && value.chars().all(|c| c.is_ascii_digit())
}

fn already_masked(value: &str) -> bool {
    value.contains("<secret:")
        || value.chars().all(|c| c == '*' || c == 'x' || c == 'X')
        || matches!(value, "<removed>" | "<hidden>" | "<redacted>" | "********")
}

#[cfg(test)]
mod tests {
    /// A careless pattern in one device type must not shift the placeholders
    /// of another.
    ///
    /// The rules of every installed device type are applied to every
    /// configuration, so a pattern that captures a key *number* instead of a
    /// key puts a junk entry in the table. The table is numbered in the order
    /// entries are made, so that renumbers every placeholder after it — and
    /// `<secret:1>`, in a line the model writes, then restores a different
    /// secret from the one it meant. That is how a real NTP key reached a
    /// model: it was restored into an interface description, where no pattern
    /// matches it on the way back.
    #[test]
    fn a_key_number_is_not_mistaken_for_a_key_whatever_the_rule_says() {
        let config = "ntp authentication-key 1 md5 N0cturneNtpKey\n";
        // A rule exactly like the one that caused it: Junos's unquoted
        // `authentication-key`, applied — as rules are — to an IOS line.
        let careless =
            Redactor::with_rules(&["authentication-key (?P<secret>\\S+)".to_string()]).unwrap();
        let hidden = careless.redact(config);
        assert!(
            !hidden.contains("N0cturneNtpKey"),
            "the key itself is still masked by the built-in rules:\n{hidden}"
        );
        assert!(
            hidden.contains("authentication-key 1 "),
            "but the key *number* is left alone, so it takes no placeholder \
             and shifts nothing:\n{hidden}"
        );
        // And the first placeholder is the key, as it would be without the
        // careless rule at all.
        assert_eq!(
            Redactor::new().redact(config),
            hidden,
            "a careless rule should make no difference to the result"
        );
    }

    #[test]
    fn a_number_long_enough_to_be_a_secret_is_still_masked() {
        let redactor = Redactor::with_rules(&["key (?P<secret>\\S+)".to_string()]).unwrap();
        let hidden = redactor.redact("key 8675309421\n");
        assert!(!hidden.contains("8675309421"), "{hidden}");
    }

    /// Keys that sit behind their algorithm. A pattern that stops at the
    /// algorithm masks the word `md5` and leaves the key in the configuration.
    #[test]
    fn the_key_is_masked_and_not_the_algorithm_that_precedes_it() {
        let redactor = Redactor::new();
        let config = "\
ntp authentication-key 1 md5 N0cturne!Ntp 7
ntp authentication-key 2 sha1 S3condKey
interface Tunnel0
 ip ospf authentication-key 7 110A1016141D
 ip ospf message-digest-key 1 md5 D1gestKey
";
        let hidden = redactor.redact(config);
        for secret in ["N0cturne!Ntp", "S3condKey", "110A1016141D", "D1gestKey"] {
            assert!(
                !hidden.contains(secret),
                "`{secret}` survived redaction:\n{hidden}"
            );
        }
        // The algorithm is not a secret and must still be readable.
        assert!(
            hidden.contains("md5") && hidden.contains("sha1"),
            "{hidden}"
        );
        assert_eq!(
            redactor.restore(&hidden).text,
            config,
            "restoring must give the original back"
        );
    }

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

/// Redacting twice must be the same as redacting once.
///
/// Nothing in the application is supposed to redact text that has already
/// been redacted, but a feature that composes a result out of several reads
/// can easily end up doing it, and the failure is silent: a rule whose
/// pattern happens to straddle a `<secret:N>` placeholder would mask part of
/// the placeholder, or — if a skipped match left the write cursor behind —
/// drop the text after it. The assistant crate reported both symptoms
/// against an earlier version of this file, so they are pinned here.
#[cfg(test)]
mod idempotence {
    use super::*;

    #[test]
    fn a_secret_quoted_inside_a_log_line_does_not_escape() {
        // Every one of these is a real shape: a device telling you what
        // somebody typed. The rules used to require the command to start the
        // line, so all of them went past in the clear — and a log line is
        // exactly the kind of text that reaches a model.
        let lines = [
            (
                "%PARSER-5-CFGLOG_LOGGEDCMD: User:admin logged command:enable secret 5 $1$Qs4w$Kp9",
                "$1$Qs4w$Kp9",
            ),
            (
                "Nov 13 22:04:01 SW1 %SYS-5-CONFIG_I: logged command:username ops privilege 15 password 0 Hunter2xy",
                "Hunter2xy",
            ),
            (
                "date=2026-11-13 devname=\"FG100\" msg=\"cfg attr set: set psksecret ENC 7Ab9Qz\"",
                "7Ab9Qz",
            ),
            (
                "admin typed: ip ftp password FtpSecret9 on CORE1",
                "FtpSecret9",
            ),
            (
                "/snmp community set name=PrivateString address=0.0.0.0/0",
                "PrivateString",
            ),
            (
                "12:00:01 cli: set community=WriteMe for the manager",
                "WriteMe",
            ),
        ];
        for (line, secret) in lines {
            let redactor = Redactor::new();
            let out = redactor.redact(line);
            assert!(!out.contains(secret), "`{secret}` survived in: {out}");
            assert!(out.contains("<secret:"), "nothing was masked in: {out}");
        }
    }

    #[test]
    fn ordinary_prose_about_passwords_is_left_alone() {
        // The price of not anchoring is false positives, so the rules that
        // are only a keyword and a value stay anchored. These must survive.
        let redactor = Redactor::new();
        for line in [
            "the password is wrong, check the credential",
            "ask the operator for the key before you start",
            "a community of practice meets on Tuesdays",
        ] {
            assert_eq!(redactor.redact(line), line, "prose was masked");
        }
    }

    #[test]
    fn placeholders_from_somewhere_else_pass_through_whole() {
        let redactor = Redactor::new();
        let text = "snmp-server community <secret:3> RO\n key <secret:4>\n";
        assert_eq!(redactor.redact(text), text);
    }

    #[test]
    fn a_second_pass_over_a_whole_configuration_is_a_no_op() {
        // One line per rule this crate carries, in the form a device emits.
        let configuration = "\
hostname CORE-SW1
enable secret 5 $1$mERr$Q8Fh2jKpL9vXcB3nD7wQe1
username admin privilege 15 password 7 070C285F4D06
username backup secret 9 $9$abc123DEF456ghi
snmp-server community PublicRead RO
snmp-server community PrivateWrite RW
snmp-server host 10.0.0.5 version 3 priv AuthPass123
ntp authentication-key 1 md5 NtpSharedKey 7
crypto isakmp key MyPreSharedKey address 203.0.113.9
tacacs-server key TacacsSecret99
radius-server key 7 110A1016141D
ip ftp password FtpPassword1
line vty 0 4
 password VtyLinePass
key chain EIGRP-KEYS
 key 1
  key-string RoutingKey42
interface Tunnel0
 ipsec profile PROF
 pre-shared-key TunnelPSK2026
wlan guest 2 guest-ssid
 security wpa psk set-key ascii0 WifiPassword9
username api password 0 PlainTextPass
set snmp community read-only WriteString
/interface wireguard peers add private-key=\"aGVsbG8gd29ybGQgdGhpcyBpcyBhIGtleQ==\"
/user add name=ops password=\"RouterOsPass\"
";
        let redactor = Redactor::new();
        let once = redactor.redact(configuration);
        let twice = redactor.redact(&once);
        assert_eq!(twice, once, "a second pass changed the text");

        // Every line survives, in order — nothing was dropped on the way.
        assert_eq!(
            once.lines().count(),
            configuration.lines().count(),
            "lines went missing:\n{once}"
        );
        assert!(once.contains("hostname CORE-SW1"));
        assert!(once.contains("line vty 0 4"));
        assert!(once.ends_with('\n'));

        // And it did its job: none of the secrets is still there.
        for secret in [
            "Q8Fh2jKpL9vXcB3nD7wQe1",
            "070C285F4D06",
            "PublicRead",
            "PrivateWrite",
            "AuthPass123",
            "NtpSharedKey",
            "MyPreSharedKey",
            "TacacsSecret99",
            "110A1016141D",
            "FtpPassword1",
            "VtyLinePass",
            "RoutingKey42",
            "TunnelPSK2026",
            "WifiPassword9",
            "PlainTextPass",
            "WriteString",
            "RouterOsPass",
        ] {
            assert!(
                !once.contains(secret),
                "`{secret}` was left in the clear:\n{once}"
            );
        }

        // Every placeholder is whole: `<secret:` followed by digits and `>`,
        // never nested and never with a rule's leftovers inside it.
        assert!(
            !twice.contains("<secret:<"),
            "a placeholder was nested: {twice}"
        );
        for (index, _) in twice.match_indices("<secret:") {
            let rest = &twice[index + "<secret:".len()..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            assert!(
                !digits.is_empty() && rest[digits.len()..].starts_with('>'),
                "a placeholder is malformed at {index}: {:?}",
                &twice[index..(index + 24).min(twice.len())]
            );
        }
    }
}
