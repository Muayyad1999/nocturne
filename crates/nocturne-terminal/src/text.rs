//! Terminal output as plain text, and hiding the secrets in it.
//!
//! The live byte stream of a session is its user's own view of the device and
//! is never altered. Text *derived* from it — a snapshot handed to "explain
//! this", a session log read back — hides device secrets unless the caller
//! asks for it verbatim.
//!
//! What is handed out is always a window: the last lines of a scrollback, a
//! page of a log file. Every secret but one sits on a line of its own, and a
//! window made of whole lines cannot cut it. The one that does not is a
//! private key, which runs over dozens of lines, so that a window can hold
//! lines of it and neither of the two markers that say what they are. That is
//! why the lines around a window are looked at too ([`redact_window`]): never
//! shown, only read to learn whether the window lies inside a key.

use std::ops::Range;
use std::sync::LazyLock;

use nocturne_ai::Redactor;
use nocturne_drivers::term::Screen;
use regex::Regex;

/// What a hidden line of key material reads as. The redactor recognises it
/// as already masked.
const HIDDEN: &str = "<hidden>";

/// How many lines in a row that are not key material a key is followed
/// across when one of its markers is missing: a pager writes `--More--` into
/// the middle of a key as readily as anywhere, and a device logs a line when
/// it likes. One more than this and the key is taken to have stopped there —
/// interrupted, or never finished.
const STRAY_LINES: usize = 3;

/// What both markers of a key end in. Looked for first, because nearly every
/// line of a long scrollback is neither.
const MARKER_TAIL: &str = "PRIVATE KEY-----";

/// The timestamp a session log puts in front of every line.
static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\S* ?").expect("timestamp pattern")
});
/// A command prompt in front of what was typed: `SW1#`, `R1(config-if)#`, `user@host:~$`.
static PROMPT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[\w.\-:/@~\[\]]{1,64}(?:\([\w\-/. ]+\))?\s?[>#$%]\s?").expect("prompt pattern")
});
static KEY_BEGIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----").expect("key pattern"));
static KEY_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-----END [A-Z0-9 ]*PRIVATE KEY-----").expect("key pattern"));
/// The body of a key: a line of base64, or a blank one.
static KEY_BASE64: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[A-Za-z0-9+/=]*\s*$").expect("key body pattern"));
/// A PEM header, as an encrypted key has between its first marker and its
/// body: `Proc-Type: 4,ENCRYPTED`, `DEK-Info: AES-128-CBC,…`.
static KEY_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[\w-]+: .*$").expect("key header pattern"));

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

/// Hide the secrets in a transcript that is all there is: nothing is known
/// of what stood before or after it.
#[cfg(test)]
pub(crate) fn redact_transcript(text: &str, extra_rules: &[String]) -> Redaction {
    let lines: Vec<&str> = text.split('\n').collect();
    redact_window(&lines, 0..lines.len(), extra_rules)
}

/// Hide the secrets in `lines[window]`, a window on a longer transcript.
///
/// Only the window is returned and only the window is searched by the
/// redactor. The other lines are context: they decide whether a line of the
/// window is part of a private key whose markers the window does not hold.
///
/// The redactor's rules are written for configuration text, where a secret
/// sits on a line of its own kind (`enable secret …`). In a transcript the
/// same words follow a prompt, and in a session log a timestamp too, so those
/// are set aside before redacting and put back afterwards. What was taken
/// for a prompt is then redacted with its line once more, in case it was
/// not one.
pub(crate) fn redact_window(
    lines: &[&str],
    window: Range<usize>,
    extra_rules: &[String],
) -> Redaction {
    let redactor = Redactor::with_rules(extra_rules).unwrap_or_else(|err| {
        tracing::debug!(%err, "a profile's secret pattern does not compile; using the built-in rules");
        Redactor::new()
    });
    let window = window.start.min(lines.len())..window.end.min(lines.len());
    let key = key_lines(lines);

    let mut stamps = Vec::with_capacity(window.len());
    let mut prompts = Vec::with_capacity(window.len());
    let mut bodies = Vec::with_capacity(window.len());
    for index in window.clone() {
        let line = lines[index];
        let stamp = stamp_len(line);
        stamps.push(&line[..stamp]);
        if key[index] {
            // The whole line under its timestamp: a prompt has no place
            // inside a key, and what looks like one there is part of it.
            prompts.push("");
            bodies.push(if line[stamp..].trim().is_empty() {
                line[stamp..].to_string()
            } else {
                HIDDEN.to_string()
            });
            continue;
        }
        let prompt = PROMPT.find(&line[stamp..]).map_or(0, |found| found.end());
        prompts.push(&line[stamp..stamp + prompt]);
        bodies.push(line[stamp + prompt..].to_string());
    }

    let original = lines[window].join("\n");
    // Redacted text, line for line again — unless a profile rule swallowed a
    // line break, and the lines no longer pair up with what was set aside.
    let paired = |text: String| -> Option<Vec<String>> {
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        (lines.len() == stamps.len()).then_some(lines)
    };
    let prompted = |bodies: &[String]| -> String {
        join_prefixed(&prompts, bodies.iter().map(String::as_str))
    };
    let stamped = |text: &str| join_prefixed(&stamps, text.split('\n'));

    // Twice. First what follows each prompt, on its own: that is where the
    // rules written for a line of configuration find it. Then the line with
    // its prompt, because not everything that looks like a prompt is one: a
    // line of `/etc/shadow` starts `root:$`, and the `$` taken for the end
    // of a prompt is the start of the hash. The redactor leaves alone what
    // the first pass has masked.
    let result = match paired(redactor.redact(&bodies.join("\n"))) {
        Some(after_prompts) => {
            let whole = prompted(&after_prompts);
            match paired(redactor.redact(&whole)) {
                Some(whole) => stamped(&whole.join("\n")),
                None => redactor.redact(&stamped(&whole)),
            }
        }
        // Redact the text as it stands instead — with the key material
        // already out of it, which no rule would find.
        None => redactor.redact(&stamped(&prompted(&bodies))),
    };
    Redaction {
        redacted: result != original,
        text: result,
    }
}

fn join_prefixed<'a>(prefixes: &[&str], bodies: impl Iterator<Item = &'a str>) -> String {
    let mut result = String::new();
    for (index, (prefix, body)) in prefixes.iter().zip(bodies).enumerate() {
        if index > 0 {
            result.push('\n');
        }
        result.push_str(prefix);
        result.push_str(body);
    }
    result
}

/// How much of `line` is the timestamp a session log wrote in front of it.
fn stamp_len(line: &str) -> usize {
    TIMESTAMP.find(line).map_or(0, |found| found.end())
}

/// Whether a line holds the first marker of a key, and whether its last.
fn markers(line: &str) -> (bool, bool) {
    if !line.contains(MARKER_TAIL) {
        return (false, false);
    }
    (KEY_BEGIN.is_match(line), KEY_END.is_match(line))
}

/// What a line between the two markers of a key is, if it can stand there
/// at all. In a session log each such line has a timestamp in front, which
/// is not part of the question.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Material {
    /// Base64, or a blank line.
    Body,
    /// `Name: value`, which a key has only above its body.
    Header,
}

fn key_material(line: &str) -> Option<Material> {
    let line = &line[stamp_len(line)..];
    if KEY_BASE64.is_match(line) {
        Some(Material::Body)
    } else if KEY_HEADER.is_match(line) {
        Some(Material::Header)
    } else {
        None
    }
}

/// Which lines are private-key material: everything a key's first marker is
/// followed by, and everything its last marker is preceded by.
///
/// A block may be cut off at either end — the text is a window on a longer
/// stream, and a command can be interrupted — so a marker without its
/// counterpart claims the key material that adjoins it, across the odd line
/// that is something else ([`STRAY_LINES`]). The first marker itself is left
/// standing: it holds no secret and says what was there.
fn key_lines(lines: &[&str]) -> Vec<bool> {
    let mut key = vec![false; lines.len()];
    let mut index = 0;
    while index < lines.len() {
        match markers(lines[index]) {
            (true, false) => {
                index += 1;
                let mut strays = 0;
                // Headers come before the body. After it, `Name: value` is
                // the device talking again (`Hostname: SW1`), not the key.
                let mut in_body = false;
                while index < lines.len() {
                    let (begins, ends) = markers(lines[index]);
                    if ends {
                        key[index] = true;
                        index += 1;
                        break;
                    }
                    if begins {
                        break;
                    }
                    match key_material(lines[index]) {
                        Some(Material::Header) if in_body => strays += 1,
                        Some(material) => {
                            in_body |= material == Material::Body
                                && !lines[index][stamp_len(lines[index])..].trim().is_empty();
                            key[index] = true;
                            strays = 0;
                        }
                        None => strays += 1,
                    }
                    if strays > STRAY_LINES {
                        break;
                    }
                    index += 1;
                }
            }
            (false, true) => {
                key[index] = true;
                let mut above = index;
                let mut strays = 0;
                while above > 0 && !key[above - 1] && markers(lines[above - 1]) == (false, false) {
                    above -= 1;
                    if key_material(lines[above]).is_some() {
                        key[above] = true;
                        strays = 0;
                    } else {
                        strays += 1;
                        if strays > STRAY_LINES {
                            break;
                        }
                    }
                }
                index += 1;
            }
            // No marker, or a key on a single line, which the redactor handles.
            _ => index += 1,
        }
    }
    key
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
            assert!(
                !redaction.text.contains(secret),
                "{secret} leaked:\n{}",
                redaction.text
            );
        }
        // Everything that is not a secret is where it was.
        let lines: Vec<&str> = redaction.text.lines().collect();
        assert_eq!(lines.len(), 5);
        assert!(
            lines[0]
                .starts_with("2026-11-13T05:24:05.123+02:00 SW1(config)#enable secret <secret:")
        );
        assert_eq!(
            lines[2],
            "2026-11-13T05:24:07.000+02:00 SW1#show running-config | include snmp"
        );
        assert!(lines[3].ends_with(" RO"));
        assert_eq!(lines[4], "2026-11-13T05:24:07.200+02:00 SW1#");
    }

    #[test]
    fn a_secret_that_starts_where_a_prompt_would_end_is_hidden_all_the_same() {
        // A line of `/etc/shadow` begins as a prompt does — a name, a colon,
        // a `$` — and the hash begins with that very `$`. Synthetic: the
        // salts and hashes are letters typed for this test.
        let text = "\
admin@host:~$ sudo cat /etc/shadow
root:$6$saltSALTsalt$hashHASHhashHASHhashHASHhashHASHhashHASH:19000:0:99999:7:::
ops:$y$j9T$saltSALTsaltSALT$hashHASHhashHASHhashHASHhashHASH:19000:0:99999:7:::
admin@host:~$ ";
        let redaction = redact_transcript(text, &[]);
        assert!(redaction.redacted);
        for hash in ["saltSALTsalt", "hashHASHhash"] {
            assert!(
                !redaction.text.contains(hash),
                "{hash} is shown:\n{}",
                redaction.text
            );
        }
        let lines: Vec<&str> = redaction.text.split('\n').collect();
        assert_eq!(lines.len(), 4);
        // The prompts and the command are where they were.
        assert_eq!(lines[0], "admin@host:~$ sudo cat /etc/shadow");
        assert!(lines[1].starts_with("root:<secret:"), "{}", lines[1]);
        assert!(lines[2].starts_with("ops:<secret:"), "{}", lines[2]);
        assert_eq!(lines[3], "admin@host:~$ ");

        // The same under the timestamps of a session log, and with what
        // follows a real prompt still found by the rules written for a line
        // of configuration.
        let logged = "\
2026-11-13T05:24:05.123+02:00 root:$6$saltSALTsalt$hashHASHhashHASHhashHASHhashHASHhashHASH:19000:0:99999:7:::
2026-11-13T05:24:06.001+02:00 SW1(config-line)#password Sup3rS3cret";
        let redaction = redact_transcript(logged, &[]);
        assert!(!redaction.text.contains("hashHASH"), "{}", redaction.text);
        assert!(
            !redaction.text.contains("Sup3rS3cret"),
            "{}",
            redaction.text
        );
        assert!(
            redaction
                .text
                .contains("2026-11-13T05:24:06.001+02:00 SW1(config-line)#password <secret:"),
            "{}",
            redaction.text
        );
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

    /// Not a key: lines of the base64 alphabet, written by hand for these
    /// tests (synthetic).
    const KEY: [&str; 6] = [
        "MIIEpAIBAAKCAQEAtextTest01aaaaBBBBccccDDDDeeeeFFFFggggHHHHiiiiJJ",
        "Zm9yIHRlc3RzIG9ubHkgMDIgbm90IGEga2V5IGF0IGFsbCBqdXN0IGxldHRlcnMg",
        "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVowMTIzNDU2Nzg5YWJjZGVmZ2hpams0",
        "bm9jdHVybmUgdGVybWluYWwgcmVkYWN0aW9uIHVuaXQgdGVzdCBsaW5lIDA0IHBh",
        "c3ludGhldGljIGtleSBtYXRlcmlhbCBsaW5lIDA1IG5vdGhpbmcgcmVhbCBoZXJl",
        "bGFzdCBsaW5lPT0=",
    ];

    /// The window `lines[from..to]` of a transcript, redacted.
    fn window(lines: &[&str], from: usize, to: usize) -> String {
        redact_window(lines, from..to, &[]).text
    }

    #[test]
    fn a_window_inside_a_key_is_hidden_by_the_markers_outside_it() {
        let lines = [
            "SW1#show key",
            "-----BEGIN RSA PRIVATE KEY-----",
            KEY[0],
            KEY[1],
            KEY[2],
            KEY[3],
            KEY[4],
            KEY[5],
            "-----END RSA PRIVATE KEY-----",
            "SW1#show clock",
        ];
        // Every window there is, whether it holds a marker or not.
        for from in 0..lines.len() {
            for to in from + 1..=lines.len() {
                let text = window(&lines, from, to);
                for key in KEY {
                    assert!(!text.contains(key), "{from}..{to} shows the key:\n{text}");
                }
                // The window is what was asked for, line for line.
                assert_eq!(text.split('\n').count(), to - from, "{from}..{to}");
            }
        }
        assert_eq!(window(&lines, 3, 6), "<hidden>\n<hidden>\n<hidden>");
        // The lines around the key are the window's own when it holds them,
        // and are never added to it when it does not.
        assert_eq!(
            window(&lines, 0, 2),
            "SW1#show key\n-----BEGIN RSA PRIVATE KEY-----"
        );
        assert_eq!(window(&lines, 8, 10), "<hidden>\nSW1#show clock");
        // Only the first marker, or only the last, is enough.
        assert_eq!(window(&lines[..6], 4, 6), "<hidden>\n<hidden>");
        assert_eq!(window(&lines[3..], 0, 2), "<hidden>\n<hidden>");
        // With neither there is nothing to go by: text that is all there is
        // and only looks like a key is left alone.
        assert_eq!(window(&lines[3..6], 0, 3), lines[3..6].join("\n"));
    }

    #[test]
    fn a_window_of_a_session_log_is_judged_without_its_timestamps() {
        let stamped: Vec<String> = [
            "SW1#show key",
            "-----BEGIN RSA PRIVATE KEY-----",
            "Proc-Type: 4,ENCRYPTED",
            "",
            KEY[0],
            KEY[1],
            KEY[5],
            "-----END RSA PRIVATE KEY-----",
            "SW1#",
        ]
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let stamp = format!("2026-11-13T05:24:{index:02}.000+02:00");
            if line.is_empty() {
                stamp
            } else {
                format!("{stamp} {line}")
            }
        })
        .collect();
        let lines: Vec<&str> = stamped.iter().map(String::as_str).collect();
        for from in 0..lines.len() {
            for to in from + 1..=lines.len() {
                let text = window(&lines, from, to);
                for key in [KEY[0], KEY[1], KEY[5], "ENCRYPTED"] {
                    assert!(!text.contains(key), "{from}..{to} shows the key:\n{text}");
                }
            }
        }
        // Each hidden line keeps the time it was printed at; a blank one
        // stays blank.
        assert_eq!(
            window(&lines, 2, 5),
            "2026-11-13T05:24:02.000+02:00 <hidden>\n\
             2026-11-13T05:24:03.000+02:00\n\
             2026-11-13T05:24:04.000+02:00 <hidden>"
        );
    }

    #[test]
    fn a_cut_off_key_is_followed_across_a_line_or_two_that_is_something_else() {
        // A pager in the middle of it, and a line the device logged.
        let head = [
            "-----BEGIN OPENSSH PRIVATE KEY-----",
            KEY[0],
            " --More-- ",
            KEY[1],
            "%SYS-5-CONFIG_I: Configured from console by ops",
            "*Nov 13 05:24:07.123: %LINK-3-UPDOWN: Interface Gi0/1, changed state to up",
            KEY[2],
        ];
        assert_eq!(
            window(&head, 0, head.len()),
            format!(
                "{}\n<hidden>\n{}\n<hidden>\n{}\n{}\n<hidden>",
                head[0], head[2], head[4], head[5]
            )
        );
        let tail = [
            KEY[3],
            " --More-- ",
            KEY[4],
            KEY[5],
            "-----END OPENSSH PRIVATE KEY-----",
            "SW1#",
        ];
        assert_eq!(
            window(&tail, 0, tail.len()),
            "<hidden>\n --More-- \n<hidden>\n<hidden>\n<hidden>\nSW1#"
        );
    }

    #[test]
    fn a_key_that_was_interrupted_does_not_claim_what_came_after_it() {
        // Ctrl+C after two lines, and the session went on.
        let lines = [
            "$ cat id_rsa",
            "-----BEGIN RSA PRIVATE KEY-----",
            KEY[0],
            KEY[1],
            "^C",
            "$ show clock",
            "12:00:00.000 UTC Fri Nov 13 2026",
            "$ show users",
            "ops",
            "admin",
            "Hostname: SW1",
        ];
        let text = window(&lines, 0, lines.len());
        assert_eq!(
            text,
            "$ cat id_rsa\n-----BEGIN RSA PRIVATE KEY-----\n<hidden>\n<hidden>\n^C\n\
             $ show clock\n12:00:00.000 UTC Fri Nov 13 2026\n$ show users\nops\nadmin\nHostname: SW1"
        );
        // Closer than that to the cut, a bare word is taken for more of the
        // key: hiding a word too many is the safe way to be wrong.
        let lines = [
            "-----BEGIN RSA PRIVATE KEY-----",
            KEY[0],
            "^C",
            "ops",
            "Hostname: SW1",
        ];
        assert_eq!(
            window(&lines, 0, lines.len()),
            "-----BEGIN RSA PRIVATE KEY-----\n<hidden>\n^C\n<hidden>\nHostname: SW1"
        );
        // The same from below: a last marker claims what adjoins it and no more.
        let lines = [
            "admin",
            "$ show clock",
            "12:00:00.000 UTC Fri Nov 13 2026",
            "$ show users",
            "$ tail -3 id_rsa",
            KEY[4],
            KEY[5],
            "-----END RSA PRIVATE KEY-----",
        ];
        assert_eq!(
            window(&lines, 0, lines.len()),
            "admin\n$ show clock\n12:00:00.000 UTC Fri Nov 13 2026\n$ show users\n\
             $ tail -3 id_rsa\n<hidden>\n<hidden>\n<hidden>"
        );
        // Two keys one after the other are two keys.
        let lines = [
            "-----BEGIN RSA PRIVATE KEY-----",
            KEY[0],
            "-----BEGIN EC PRIVATE KEY-----",
            KEY[1],
            "-----END EC PRIVATE KEY-----",
            "done",
        ];
        assert_eq!(
            window(&lines, 0, lines.len()),
            "-----BEGIN RSA PRIVATE KEY-----\n<hidden>\n-----BEGIN EC PRIVATE KEY-----\n<hidden>\n<hidden>\ndone"
        );
    }

    #[test]
    fn a_profile_rule_that_swallows_line_breaks_does_not_bring_a_cut_off_key_back() {
        // The rule takes two lines for one secret, so the lines no longer
        // pair up with their prompts and the text is redacted as it stands.
        let rules = vec![r"(?s)banner motd (?P<secret>\^C.*?\^C)".to_string()];
        let text = format!(
            "SW1#show run\nbanner motd ^Ckeep\nout^C\n-----BEGIN RSA PRIVATE KEY-----\n{}\n{}",
            KEY[0], KEY[1]
        );
        let redaction = redact_transcript(&text, &rules);
        assert!(redaction.redacted);
        assert!(!redaction.text.contains("keep"), "{}", redaction.text);
        for key in &KEY[..2] {
            assert!(!redaction.text.contains(key), "{}", redaction.text);
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
