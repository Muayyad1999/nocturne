//! Hiding secrets in configuration text before it leaves the backend.
//!
//! One [`Redaction`] is used for everything that goes into one response, so a
//! placeholder means the same secret wherever it appears in that response —
//! on both sides of a diff, or on two devices in one search.
//!
//! Redacted text always has exactly as many lines as the text it was made
//! from. Line numbers in search results, diffs and compliance evidence
//! therefore mean the same line whether the configuration is shown redacted
//! or not.

use nocturne_ai::Redactor;
use nocturne_app::{App, profiles};
use nocturne_drivers::hier;

pub struct Redaction {
    redactor: Redactor,
}

impl Redaction {
    /// The built-in rules plus the `config.secrets` patterns of the given device types.
    pub fn for_profiles<'a>(app: &App, profile_ids: impl IntoIterator<Item = &'a str>) -> Redaction {
        let registry = profiles::registry(app);
        let mut patterns: Vec<String> = Vec::new();
        for platform in profile_ids.into_iter().filter_map(|id| registry.get(id)) {
            for pattern in &platform.profile.config.secrets {
                if !patterns.contains(pattern) {
                    patterns.push(pattern.clone());
                }
            }
        }
        // Profiles are only loaded when their patterns compile. Should one not,
        // drop that one pattern rather than every profile rule with it.
        patterns.retain(|pattern| regex::Regex::new(pattern).is_ok());
        Redaction {
            redactor: Redactor::with_rules(&patterns).unwrap_or_default(),
        }
    }

    /// Replace every secret in `text` by its placeholder, line for line.
    pub fn text(&self, text: &str) -> String {
        let redacted = self.redactor.redact(text);
        if redacted.lines().count() == text.lines().count() {
            redacted
        } else {
            realign(text, &redacted)
        }
    }
}

/// Whether the built-in rules would hide part of `text` — used to tell a
/// person that what they typed can never match redacted text.
pub fn has_secret(text: &str) -> bool {
    Redactor::new().redact(text) != text
}

/// Whether `text` carries placeholders left by an earlier redaction.
pub fn has_placeholders(text: &str) -> bool {
    Redactor::has_placeholders(text)
}

/// A secret that spans several lines (a private key) becomes one placeholder.
/// Pad the lines it used to occupy with empty ones so that every line after it
/// keeps its number.
fn realign(original: &str, redacted: &str) -> String {
    fn close(out: &mut Vec<String>, replaced: usize, mut replacement: Vec<String>) {
        if replacement.len() > replaced {
            // A placeholder never adds a line break, so this does not happen;
            // if it ever did, fold the surplus into one line to keep the count.
            let surplus = replacement.split_off(replaced);
            match replacement.last_mut().or(out.last_mut()) {
                Some(last) => {
                    for extra in surplus {
                        last.push(' ');
                        last.push_str(&extra);
                    }
                }
                None => out.push(surplus.join(" ")),
            }
        }
        let padding = replaced.saturating_sub(replacement.len());
        out.extend(replacement);
        out.extend(std::iter::repeat_n(String::new(), padding));
    }

    let mut out: Vec<String> = Vec::new();
    // The stretch being rewritten: how many original lines, and what stands in for them.
    let mut replaced = 0;
    let mut replacement: Vec<String> = Vec::new();
    for line in hier::text_diff(original, redacted, None).lines {
        match line.tag.as_str() {
            "-" => replaced += 1,
            "+" => replacement.push(line.text),
            _ => {
                close(&mut out, replaced, std::mem::take(&mut replacement));
                replaced = 0;
                out.push(line.text);
            }
        }
    }
    close(&mut out, replaced, replacement);
    let mut text = out.join("\n");
    if original.ends_with('\n') {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn redact(text: &str) -> String {
        Redaction {
            redactor: Redactor::new(),
        }
        .text(text)
    }

    #[test]
    fn single_line_secrets_stay_on_their_line() {
        let config = "hostname R1\nenable secret 5 $1$abcd$efghijklmnopqrstuvwxyz\nntp server 192.0.2.1\n";
        assert_eq!(
            redact(config),
            "hostname R1\nenable secret 5 <secret:1>\nntp server 192.0.2.1\n"
        );
    }

    #[test]
    fn a_multi_line_secret_keeps_the_line_numbers_after_it() {
        let config = "\
config vpn certificate local
    edit \"site\"
        set private-key \"-----BEGIN ENCRYPTED PRIVATE KEY-----
MIIFDjBABgkqhkiG9w0BBQ0wMzAbBgkqhkiG9w0BBQwwDgQI
c2VjcmV0IGtleSBtYXRlcmlhbCBnb2VzIGhlcmU=
-----END ENCRYPTED PRIVATE KEY-----\"
        set comments \"after the key\"
    next
end
";
        let redacted = redact(config);
        assert!(!redacted.contains("MIIFDjBABgkqhkiG9w0BBQ0w"), "{redacted}");
        assert!(!redacted.contains("c2VjcmV0IGtleSBtYXRlcmlhbCBnb2VzIGhlcmU"), "{redacted}");
        let before: Vec<&str> = config.lines().collect();
        let after: Vec<&str> = redacted.lines().collect();
        assert_eq!(after.len(), before.len());
        assert_eq!(after[2], "        set private-key \"<secret:1>\"");
        assert_eq!(&after[3..6], ["", "", ""]);
        // Everything after the key is where it was.
        assert_eq!(after[6..], before[6..]);
        assert!(redacted.ends_with("end\n"));
    }

    #[test]
    fn secret_and_placeholder_detection() {
        assert!(has_secret("snmp-server community S3cr3t RO"));
        assert!(!has_secret("snmp-server location Building 2"));
        assert!(has_placeholders("enable secret 5 <secret:3>"));
        assert!(!has_placeholders("enable secret 5 $1$abc"));
    }
}
