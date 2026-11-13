//! Terminal output as plain text, and hiding the secrets in it.
//!
//! The live byte stream of a session is its user's own view of the device and
//! is never altered. Text *derived* from it — a snapshot handed to "explain
//! this", a session log read back — hides device secrets unless the caller
//! asks for it verbatim.

use std::sync::LazyLock;

use nocturne_ai::Redactor;
use nocturne_drivers::term::Screen;
use regex::Regex;

/// What a hidden line of key material reads as. The redactor recognises it
/// as already masked.
const HIDDEN: &str = "<hidden>";

/// The timestamp a session log puts in front of every line.
static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\S* ?").expect("timestamp pattern")
});
/// A command prompt in front of what was typed: `SW1#`, `R1(config-if)#`, `user@host:~$`.
static PROMPT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[\w.\-:/@~\[\]]{1,64}(?:\([\w\-/. ]+\))?\s?[>#$%]\s?").expect("prompt pattern")
});
static KEY_BEGIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----").expect("key pattern")
});
static KEY_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-----END [A-Z0-9 ]*PRIVATE KEY-----").expect("key pattern"));
/// What can stand between the two markers: base64, PEM headers, blank lines.
static KEY_BODY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:[A-Za-z0-9+/=]+|[\w-]+: .*)?\s*$").expect("key body pattern")
});

/// Raw terminal output as the lines a person would have read.
pub(crate) fn plain_lines(raw: &[u8]) -> Vec<String> {
    let mut screen = Screen::new();
    screen.feed(raw);
    screen.finish()
}

/// How many bytes at the end of `bytes` start a UTF-8 character that the
/// next chunk will complete.
pub(crate) fn incomplete_utf8_suffix(bytes: &[u8]) -> usize {
    for back in 1..=bytes.len().min(3) {
        let byte = bytes[bytes.len() - back];
        let length = match byte {
            0x80..=0xBF => continue,
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => 1,
        };
        return if length > back { back } else { 0 };
    }
    0
}

pub(crate) struct Redaction {
    pub text: String,
    /// Something was hidden.
    pub redacted: bool,
}

/// Hide the secrets in a transcript.
///
/// The redactor's rules are written for configuration text, where a secret
/// sits on a line of its own kind (`enable secret …`). In a transcript the
/// same words follow a prompt, and in a session log a timestamp too, so those
/// are set aside before redacting and put back afterwards.
pub(crate) fn redact_transcript(text: &str, extra_rules: &[String]) -> Redaction {
    let redactor = Redactor::with_rules(extra_rules).unwrap_or_else(|err| {
        tracing::debug!(%err, "a profile's secret pattern does not compile; using the built-in rules");
        Redactor::new()
    });

    let mut prefixes = Vec::new();
    let mut bodies = Vec::new();
    for line in text.split('\n') {
        let stamp = TIMESTAMP.find(line).map_or(0, |found| found.end());
        let prompt = PROMPT.find(&line[stamp..]).map_or(0, |found| found.end());
        let (prefix, body) = line.split_at(stamp + prompt);
        prefixes.push(prefix);
        bodies.push(body.to_string());
    }
    hide_private_keys(&mut bodies);

    let redacted = redactor.redact(&bodies.join("\n"));
    let redacted_lines: Vec<&str> = redacted.split('\n').collect();
    let result = if redacted_lines.len() == prefixes.len() {
        let kept: usize = prefixes.iter().map(|prefix| prefix.len()).sum();
        let mut result = String::with_capacity(kept + redacted.len());
        for (index, (prefix, body)) in prefixes.iter().zip(redacted_lines).enumerate() {
            if index > 0 {
                result.push('\n');
            }
            result.push_str(prefix);
            result.push_str(body);
        }
        result
    } else {
        // A profile rule that swallows line breaks: the lines no longer pair
        // up with their prefixes, so redact the text as it stands instead.
        redactor.redact(text)
    };
    Redaction {
        redacted: result != text,
        text: result,
    }
}

/// Replace private-key material line by line. A block may be cut off at
/// either end — the text is a window on a longer stream — so a marker without
/// its counterpart hides whatever key material adjoins it.
fn hide_private_keys(lines: &mut [String]) {
    let hide = |line: &mut String| {
        if !line.trim().is_empty() {
            *line = HIDDEN.to_string();
        }
    };
    let mut index = 0;
    while index < lines.len() {
        let begins = KEY_BEGIN.is_match(&lines[index]);
        let ends = KEY_END.is_match(&lines[index]);
        index += 1;
        match (begins, ends) {
            (true, false) => {
                while index < lines.len() {
                    let end = KEY_END.is_match(&lines[index]);
                    if !end && !KEY_BODY.is_match(&lines[index]) {
                        break;
                    }
                    hide(&mut lines[index]);
                    index += 1;
                    if end {
                        break;
                    }
                }
            }
            (false, true) => {
                let marker = index - 1;
                hide(&mut lines[marker]);
                let mut above = marker;
                while above > 0 && lines[above - 1] != HIDDEN && KEY_BODY.is_match(&lines[above - 1])
                {
                    above -= 1;
                    hide(&mut lines[above]);
                }
            }
            // No marker, or a key on a single line, which the redactor handles.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn plain_lines_drop_escapes_and_keep_the_prompt_line() {
        let raw = b"SW1#show clock\r\n\x1b[1m12:00:00 UTC\x1b[0m\r\n --More-- \x08\x08\x08\x08\x08\x08\x08\x08\x08\x08          \x08\x08\x08\x08\x08\x08\x08\x08\x08\x08next\r\nSW1#";
        assert_eq!(
            plain_lines(raw),
            ["SW1#show clock", "12:00:00 UTC", "next", "SW1#"]
        );
    }

    #[test]
    fn incomplete_utf8_is_measured_at_the_end_only() {
        assert_eq!(incomplete_utf8_suffix(b""), 0);
        assert_eq!(incomplete_utf8_suffix(b"plain"), 0);
        let snowman = "☃".as_bytes(); // E2 98 83
        assert_eq!(incomplete_utf8_suffix(snowman), 0);
        assert_eq!(incomplete_utf8_suffix(&snowman[..2]), 2);
        assert_eq!(incomplete_utf8_suffix(&snowman[..1]), 1);
        let rocket = "🚀".as_bytes(); // F0 9F 9A 80
        assert_eq!(incomplete_utf8_suffix(&rocket[..3]), 3);
        assert_eq!(incomplete_utf8_suffix(rocket), 0);
        assert_eq!(incomplete_utf8_suffix(b"ab\xC3"), 1);
        // Stray continuation bytes are not waited for.
        assert_eq!(incomplete_utf8_suffix(b"\x80\x80\x80"), 0);
    }

    #[test]
    fn secrets_after_prompts_and_timestamps_are_hidden() {
        let text = "\
2026-11-13T05:24:05.123+02:00 SW1(config)#enable secret Sup3rS3cret
2026-11-13T05:24:06.001+02:00 SW1(config)#username ops privilege 15 secret 0 0psPass!
2026-11-13T05:24:07.000+02:00 SW1#show running-config | include snmp
2026-11-13T05:24:07.100+02:00 snmp-server community c0mmunity-RO RO
2026-11-13T05:24:07.200+02:00 SW1#";
        let redaction = redact_transcript(text, &[]);
        assert!(redaction.redacted);
        for secret in ["Sup3rS3cret", "0psPass!", "c0mmunity-RO"] {
            assert!(!redaction.text.contains(secret), "{secret} leaked:\n{}", redaction.text);
        }
        // Everything that is not a secret is where it was.
        let lines: Vec<&str> = redaction.text.lines().collect();
        assert_eq!(lines.len(), 5);
        assert!(lines[0].starts_with("2026-11-13T05:24:05.123+02:00 SW1(config)#enable secret <secret:"));
        assert_eq!(lines[2], "2026-11-13T05:24:07.000+02:00 SW1#show running-config | include snmp");
        assert!(lines[3].ends_with(" RO"));
        assert_eq!(lines[4], "2026-11-13T05:24:07.200+02:00 SW1#");
    }

    #[test]
    fn text_without_secrets_is_returned_verbatim() {
        let text = "SW1#show clock\n12:00:00.000 UTC Fri Nov 13 2026\nSW1#\n";
        let redaction = redact_transcript(text, &[]);
        assert!(!redaction.redacted);
        assert_eq!(redaction.text, text);
    }

    #[test]
    fn private_keys_are_hidden_even_when_cut_off() {
        let key = "\
-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACDm1Zc0c2VjcmV0c2VjcmV0c2VjcmV0c2VjcmV0c2VjcmV0cw==
-----END OPENSSH PRIVATE KEY-----";
        let lines: Vec<&str> = key.lines().collect();

        let whole = redact_transcript(&format!("$ cat id\n{key}\n$ "), &[]);
        assert_eq!(
            whole.text,
            "$ cat id\n-----BEGIN OPENSSH PRIVATE KEY-----\n<hidden>\n<hidden>\n<hidden>\n$ "
        );

        // The window ends inside the key …
        let head = redact_transcript(&format!("$ cat id\n{}\n{}", lines[0], lines[1]), &[]);
        assert_eq!(
            head.text,
            "$ cat id\n-----BEGIN OPENSSH PRIVATE KEY-----\n<hidden>"
        );
        // … or starts inside it.
        let tail = redact_transcript(&format!("{}\n{}\n$ exit", lines[2], lines[3]), &[]);
        assert_eq!(tail.text, "<hidden>\n<hidden>\n$ exit");
        for redaction in [whole, head, tail] {
            assert!(redaction.redacted);
            assert!(!redaction.text.contains("b3Blbn") && !redaction.text.contains("QyNTUx"));
        }
    }

    #[test]
    fn profile_rules_add_to_the_built_in_ones() {
        let rules = vec![r"(?m)^\s*wlan-key (?P<secret>\S+)".to_string()];
        let redaction = redact_transcript("AP1#show wlan\nwlan-key hunter2hunter2\nAP1#", &rules);
        assert!(!redaction.text.contains("hunter2hunter2"));
        // A rule that does not compile is ignored rather than turning redaction off.
        let broken = vec!["(unclosed".to_string()];
        let redaction = redact_transcript("enable secret Sup3rS3cret", &broken);
        assert!(!redaction.text.contains("Sup3rS3cret"));
    }
}
