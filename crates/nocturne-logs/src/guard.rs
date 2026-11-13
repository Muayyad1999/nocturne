//! Keeping secrets out of what is stored.
//!
//! Device output reaches this crate twice: as a syslog line a device sent,
//! and as the `show logging` buffer a pull read back. Both can contain a
//! credential — a RouterOS line echoes the command that was typed, an IOS
//! `%PARSER-5-CFGLOG_LOGGEDCMD` line logs the configuration command itself,
//! and anything with a key in it can end up in a log message. A log store is
//! exactly the kind of place a secret is never noticed again, so everything
//! kept goes through the same redactor the assistant's context does and is
//! stored with `<secret:n>` in place of the value.
//!
//! The redactor is forty-odd regular expressions, which is too much to run
//! over every line of a network sending twenty thousand a minute. A line
//! that cannot contain a secret is one that contains none of the words the
//! rules key on, so a cheap case-insensitive substring scan decides whether
//! the regular expressions run at all. The list below must stay a superset
//! of the words those rules need: a word missing here is a secret stored in
//! the clear.

use std::borrow::Cow;

use nocturne_ai::redact::Redactor;

/// Substrings that make a line worth running the redactor over. Every
/// built-in rule of `nocturne_ai::redact` needs one of these to match.
const TRIGGERS: &[&str] = &[
    "pass",       // password, passwd, passphrase, pre-shared, …
    "secret",     //
    "key",        // key, key-string, api-key, md5-key, set-key, psk keywords
    "communit",   // community, community name=
    "auth",       // authentication-key, auth md5, authorization:
    "token",      //
    "psk",        //
    "priv",       // priv aes, private-key
    "credential", //
    "-----begin", // a key block
    "$",          // hashes: $1$, $6$, $c$
    "%^%#",       // Huawei VRP ciphertext
    "%@%@",       // H3C ciphertext
    "eyj",        // a JWT
    "akia",       // an AWS access key id
    "phash",      // PAN-OS <phash>
    "bearer",     //
    "wpa",        //
    "enc ",       // FortiOS `set password ENC …`
];

/// The redactor this feature stores lines with.
///
/// It is the shared one, with nothing added. This crate used to carry
/// unanchored copies of two configuration-command rules, because the shared
/// ones were anchored to the start of a line and a log entry *quotes* a
/// command in the middle of itself — IOS's `%PARSER-5-CFGLOG_LOGGEDCMD`, a
/// FortiGate `msg="… set psksecret ENC …"`. That belonged upstream and is
/// now there, with those log shapes as its test cases, so every caller
/// benefits rather than only this one.
pub fn redactor() -> Redactor {
    Redactor::new()
}

/// Whether `haystack` contains `needle` (already lower case), ignoring case.
fn contains_fold(haystack: &str, needle: &str) -> bool {
    let (haystack, needle) = (haystack.as_bytes(), needle.as_bytes());
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
}

/// Whether this text is worth redacting at all.
pub fn may_contain_secret(text: &str) -> bool {
    TRIGGERS.iter().any(|trigger| contains_fold(text, trigger))
}

/// The text as it may be stored: secrets replaced by stable placeholders.
pub fn clean<'a>(redactor: &Redactor, text: &'a str) -> Cow<'a, str> {
    if !may_contain_secret(text) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(redactor.redact(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines in the shapes devices really write them, each with something
    /// that must not end up in the database.
    const SECRETS: &[(&str, &str)] = &[
        (
            "%PARSER-5-CFGLOG_LOGGEDCMD: User:admin  logged command:snmp-server community Str0ngR0 RO",
            "Str0ngR0",
        ),
        (
            "%PARSER-5-CFGLOG_LOGGEDCMD: User:admin  logged command:username bob secret 5 $1$mERr$9cTjUIEqNGurQiFU.ZeCi1",
            "$1$mERr$9cTjUIEqNGurQiFU.ZeCi1",
        ),
        (
            "system,info /interface wireless security-profiles set wpa2-pre-shared-key=Sup3rSecret numbers=0",
            "Sup3rSecret",
        ),
        (
            "%SYS-5-CONFIG_I: tacacs-server host 10.0.0.9 key 7 070C285F4D06",
            "070C285F4D06",
        ),
        // A configuration command quoted inside a log line, which the
        // upstream rules only recognise at the start of a line.
        (
            "date=2026-11-13 time=09:13:01 devname=FG100 type=event subtype=system logdesc=\"Configuration changed\" cfgpath=\"vpn.ipsec.phase1-interface\" msg=\"Edit vpn ipsec phase1-interface: set psksecret ENC vB7k1nQ2zz9\"",
            "vB7k1nQ2zz9",
        ),
        (
            "%HTTPS-4-AUTH_TOKEN: token eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhZG1pbiJ9.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk rejected",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhZG1pbiJ9.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        ),
    ];

    #[test]
    fn a_line_carrying_a_credential_is_stored_with_a_placeholder() {
        let redactor = redactor();
        for (line, secret) in SECRETS {
            assert!(
                may_contain_secret(line),
                "the prefilter must let this line reach the redactor: {line}"
            );
            let kept = clean(&redactor, line);
            assert!(
                !kept.contains(secret),
                "the secret is still in what would be stored: {kept}"
            );
            assert!(kept.contains("<secret:"), "{kept}");
        }
    }

    #[test]
    fn an_ordinary_line_is_stored_word_for_word_without_the_regexes_running() {
        let redactor = redactor();
        let line = "%LINK-3-UPDOWN: Interface GigabitEthernet1/0/3, changed state to down";
        assert!(!may_contain_secret(line));
        assert!(matches!(clean(&redactor, line), Cow::Borrowed(kept) if kept == line));
    }

    #[test]
    fn the_same_secret_gets_the_same_placeholder_twice() {
        let redactor = redactor();
        let first = clean(&redactor, "snmp-server community Str0ngR0 RO").into_owned();
        let second = clean(&redactor, "snmp-server community Str0ngR0 RW").into_owned();
        let token = |text: &str| {
            text.split_whitespace()
                .find(|word| word.starts_with("<secret:"))
                .map(str::to_string)
        };
        assert_eq!(token(&first), token(&second));
        assert!(token(&first).is_some());
    }

    #[test]
    fn case_folding_finds_the_trigger_wherever_it_sits() {
        assert!(contains_fold("Set PassWord xyz", "pass"));
        assert!(!contains_fold("interface up", "pass"));
        assert!(!contains_fold("ab", "abc"));
    }
}
