//! The redactor every piece of evidence passes before it is stored.
//!
//! Evidence is kept for months and exported into documents that leave the
//! building, so what it holds is decided once, on the way in, and there is
//! no way round it: no parameter of any evidence method stores text as it
//! came. Three layers, the same three a model's input passes:
//!
//! 1. `nocturne_ai::Redactor`'s built-in rules, plus the `config.secrets`
//!    patterns of every device type that is loaded — one capture can hold
//!    the output of several platforms, and a secret only has to look like a
//!    secret once;
//! 2. the values Nocturne itself holds: the passwords, enable secrets, key
//!    passphrases and SNMP secrets of every stored credential, wherever they
//!    appear and whatever surrounds them;
//! 3. every value the rules recognised in this text, wherever else it occurs
//!    in the same text: `snmp-server community X RO` is unmistakable, the
//!    same `X` in a log line three screens later is not.
//!
//! A redactor lives for one item. Its placeholder table is never stored, so
//! what was replaced cannot be recovered from the item.
//!
//! One thing the engine does not do for itself is done here: **a placeholder
//! is kept whole**. The engine applies its rules one after another to the
//! same text, so a rule meets the placeholders the rules before it made —
//! and a device type's pattern for `secret: value` finds exactly that inside
//! `<secret:1>`, takes `1>` for a credential and leaves `<secret:<secret:2>`
//! behind. Nothing leaks, but the item no longer says which values were the
//! same, and it is not what anybody would call evidence. So placeholders are
//! set aside while rules run ([`shield`]), and a pattern that can match
//! inside one runs in a pass of its own, before any other rule has made any
//! ([`EvidenceRedactor::rules`]).
//!
//! And one thing is done differently from a chat: **a device type's pattern
//! is applied to one line at a time.** The patterns are written for a line
//! of configuration, and several of them say so with a `$` that only means
//! "end of line" when the text is one line. Given a whole transcript, a
//! pattern for `password: value` reads the device's `Password:` prompt, steps
//! over the line break and takes the first word of the next line — the
//! prompt and the command typed at it — for the password. That hides no
//! secret and turns `SW1#show clock` into a placeholder wherever it occurs.
//! Only the engine's own rules see the text whole, for the one thing that
//! really spans lines: a private key.
//!
//! The assistant builds the same thing for a chat
//! (`nocturne-assistant/src/redact.rs`), and the terminal feature holds the
//! transcript handling ([`EvidenceRedactor::text`]) in its own crate.
//! Features do not link to each other, so the construction is written again
//! here from the engine's parts; its proper home is the core, next to the
//! profiles and the vault it reads.

use nocturne_ai::Redactor;
use nocturne_ai::redact::RedactionTable;
use nocturne_app::{App, credentials, profiles};
use regex::Regex;
use serde_json::Value;
use zeroize::Zeroizing;

/// Shortest value from the vault that is hidden wherever it appears. Shorter
/// ones are ordinary words far more often than they are secrets — a lab
/// password of `pw` would shred every line containing "pw" — and the rules
/// still catch them where they stand as a credential.
const MIN_KNOWN_LEN: usize = 6;

/// What a hidden line of key material reads as. The engine's rules take it
/// for something already masked and leave it alone.
const HIDDEN: &str = "<hidden>";

/// How far into a line a prompt can end, and how many candidates are tried.
const PROMPT_WITHIN: usize = 96;
const PROMPT_CANDIDATES: usize = 4;

/// How many lines that are not key material may stand between two that are
/// before a cut-off key is taken to have ended: a pager's `--More--`, a
/// line the device logged in the middle of it.
const STRAY_LINES: usize = 3;

/// The highest number a placeholder already in the text may carry and still
/// be counted on from. A device prints what it likes, `<secret:4294967295>`
/// included, and numbering from there would run the engine's counter off
/// its end. No redactor of ours gets anywhere near this.
const HIGHEST_PLAUSIBLE: u32 = u32::MAX / 2;

/// The shortest attribute value of a drawing the rules are tried on. Below
/// it are the coordinates, sizes and colours a drawing is mostly made of.
const SHORTEST_SEARCHED: usize = 6;

pub struct EvidenceRedactor {
    /// The engine's own rules and nothing else, for what spans lines.
    plain: Redactor,
    /// Device-type patterns that can match inside a placeholder, each with
    /// a redactor of its own. Usually none or one.
    apart: Vec<(Regex, Redactor)>,
    /// Every other device-type pattern and the engine's own rules. Its table
    /// is the item's table: the others are brought into step with it before
    /// they are used and it with them afterwards.
    redactor: Redactor,
    /// Values held by the vault. They are secrets for as long as this lives,
    /// which is one call.
    known: Vec<Zeroizing<String>>,
}

impl EvidenceRedactor {
    /// A redactor with the loaded device types' patterns and the vault's
    /// values.
    pub fn open(app: &App) -> EvidenceRedactor {
        let mut patterns: Vec<String> = Vec::new();
        for platform in profiles::registry(app).platforms() {
            for pattern in &platform.profile.config.secrets {
                if !patterns.contains(pattern) {
                    patterns.push(pattern.clone());
                }
            }
        }
        EvidenceRedactor::with(&patterns, vault_values(app))
    }

    fn with(patterns: &[String], known: Vec<Zeroizing<String>>) -> EvidenceRedactor {
        // A pattern that does not compile is left out, and said: one device
        // type's mistake must not switch redaction off for all of them.
        let (usable, broken): (Vec<&String>, Vec<&String>) = patterns
            .iter()
            .partition(|pattern| Regex::new(pattern).is_ok());
        if !broken.is_empty() {
            tracing::warn!(
                count = broken.len(),
                "a device type's secret pattern does not compile; evidence is redacted without it"
            );
        }
        let (apart, together): (Vec<&String>, Vec<&String>) = usable
            .into_iter()
            .partition(|pattern| matches_inside_a_placeholder(pattern));
        let build = |patterns: &[&String]| {
            // Every pattern here compiled a moment ago; the engine's own
            // rules are what is left if it disagrees.
            Redactor::with_rules(patterns).unwrap_or_default()
        };
        EvidenceRedactor {
            plain: Redactor::new(),
            apart: apart
                .iter()
                .filter_map(|pattern| Some((Regex::new(pattern).ok()?, build(&[*pattern]))))
                .collect(),
            redactor: build(&together),
            known,
        }
    }

    /// With the built-in rules and these patterns, for tests of the text
    /// handling.
    #[cfg(test)]
    fn bare(known: &[&str]) -> EvidenceRedactor {
        EvidenceRedactor::with(
            &[],
            known
                .iter()
                .map(|value| Zeroizing::new(value.to_string()))
                .collect(),
        )
    }

    /// Run `other` over `text` with the placeholders in it set aside, as
    /// part of this item: one value gets one placeholder whichever redactor
    /// found it.
    fn pass(&self, other: &Redactor, text: &str) -> String {
        let (shielded, held) = shield(text);
        other.import(self.redactor.export());
        let redacted = other.redact(&shielded);
        self.redactor.import(other.export());
        unshield(redacted, &held)
    }

    /// The engine's own rules over the whole text: what spans lines.
    fn across_lines(&self, text: &str) -> String {
        self.pass(&self.plain, text)
    }

    /// Every rule, over one line, keeping the placeholders in it whole.
    ///
    /// The patterns that can match inside a placeholder go first, one pass
    /// each, so that none of them ever meets a placeholder: those already in
    /// the line are set aside for the pass, and no other rule has made any
    /// yet.
    fn rules(&self, line: &str) -> String {
        let mut current = line.to_string();
        for (pattern, apart) in &self.apart {
            // Most lines have nothing for it, and then the pass is skipped:
            // the engine's own rules run with the last one in any case.
            if pattern.is_match(&shield(&current).0) {
                current = self.pass(apart, &current);
            }
        }
        let (shielded, held) = shield(&current);
        unshield(self.redactor.redact(&shielded), &held)
    }

    /// [`EvidenceRedactor::rules`] for each line of `text`.
    fn line_by_line(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for (index, line) in text.split('\n').enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(&self.rules(line));
        }
        out
    }

    /// Device output, a session log, or text a person selected.
    ///
    /// The engine's rules are written for configuration, where `password …`
    /// or `key …` opens a line. In a transcript the same words follow a
    /// prompt (`SW1(config-line)#password …`), and in a session log a
    /// timestamp too. What the capture methods store has already been
    /// through the terminal feature, which sets both aside; what a client
    /// hands to `evidence.add_text` has been through nothing. So it is done
    /// here as well, and without knowing what a prompt looks like: whatever
    /// follows a character a prompt can end in is offered to the rules as if
    /// it began a line. A wrong guess costs a pass that finds nothing, or
    /// hides a word that was no secret — the safe way to be wrong.
    pub fn text(&self, text: &str) -> String {
        // A placeholder that is already in the text was made by another
        // redactor. Numbering from above it keeps one placeholder from
        // standing for two different values in the same item.
        self.start_above(highest_placeholder(text));

        let whole = self.spanning(text);
        let mut out = String::with_capacity(whole.len());
        for (index, line) in whole.split('\n').enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(&self.after_prompts(&self.rules(line)));
        }
        self.values(&out)
    }

    /// What spans lines: a whole key, which the engine's rules replace from
    /// its first marker to its last, and one that an edge cut off, which
    /// they would not see.
    fn spanning(&self, text: &str) -> String {
        let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        hide_cut_off_keys(&mut lines);
        self.across_lines(&lines.join("\n"))
    }

    /// A title, a note, a name: the rules and the known values, nothing
    /// about transcripts.
    ///
    /// A title is one line. A string out of a chart, out of recorded
    /// parameters or out of a drawing is whatever its source put there, so
    /// one that has several lines is searched for what spans them too.
    pub fn line(&self, text: &str) -> String {
        if text.contains('\n') {
            return self.values(&self.line_by_line(&self.spanning(text)));
        }
        self.values(&self.line_by_line(text))
    }

    /// Every string inside a JSON value. Keys are left alone: they are this
    /// feature's own field names.
    pub fn json(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.line(text)),
            Value::Array(items) => Value::Array(items.iter().map(|item| self.json(item)).collect()),
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, item)| (key.clone(), self.json(item)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// A drawing.
    ///
    /// It is taken apart as a parser would take it: text between tags, the
    /// values of attributes, comments and CDATA sections. Each is read as
    /// the text it stands for (`&quot;` is a quote to a rule that takes a
    /// quoted value whole), searched, and written back escaped — so a
    /// placeholder is text like any other and the drawing stays one. The
    /// names of elements and attributes are never touched: a word a rule
    /// took for a secret is not chased into `stroke-width`.
    ///
    /// It is gone through twice, because a value the rules recognise in the
    /// last label has to be hidden in the first one as well.
    pub fn svg(&self, svg: &str) -> String {
        let searched = self.drawing(svg, true);
        self.drawing(&searched, false)
    }

    /// One pass over a drawing: with the rules, or with only the values
    /// known by now.
    fn drawing(&self, svg: &str, rules: bool) -> String {
        let hide = |plain: &str| {
            if rules {
                self.line(plain)
            } else {
                self.values(plain)
            }
        };
        let mut out = String::with_capacity(svg.len());
        for piece in pieces(svg) {
            match piece {
                // The line breaks and indentation between tags.
                Piece::Text(text) if text.trim().is_empty() => out.push_str(text),
                Piece::Text(text) => {
                    let plain = xml_unescaped(text);
                    let hidden = hide(&plain);
                    // A piece nothing was hidden in stays byte for byte
                    // what the source wrote.
                    if hidden == plain {
                        out.push_str(text);
                    } else {
                        out.push_str(&xml_escaped(&hidden, false));
                    }
                }
                Piece::Tag(tag) => {
                    let mut rest = tag;
                    while let Some(start) = rest.find(['"', '\'']) {
                        let quote = &rest[start..=start];
                        let Some(length) = rest[start + 1..].find(quote) else {
                            break;
                        };
                        let value = &rest[start + 1..start + 1 + length];
                        out.push_str(&rest[..=start]);
                        let plain = xml_unescaped(value);
                        // Coordinates and colours are most of a drawing and
                        // hold nothing; the known values are still looked
                        // for in them.
                        let hidden = if plain.len() < SHORTEST_SEARCHED {
                            self.values(&plain)
                        } else {
                            hide(&plain)
                        };
                        if hidden == plain {
                            out.push_str(value);
                        } else {
                            out.push_str(&xml_escaped(&hidden, true));
                        }
                        out.push_str(quote);
                        rest = &rest[start + length + 2..];
                    }
                    out.push_str(rest);
                }
                // Not shown as text and not escaped: `<` may stand in a
                // comment or a CDATA section as it is, and what closes one
                // is not in a placeholder.
                Piece::Held { open, inner, close } => {
                    out.push_str(open);
                    out.push_str(&hide(inner));
                    out.push_str(close);
                }
            }
        }
        out
    }

    fn start_above(&self, highest: u32) {
        if highest == 0 {
            return;
        }
        // The value is one no text can contain, so it is never matched; the
        // entry only moves the counter.
        self.redactor.import(RedactionTable {
            entries: vec![(highest, "\u{0}placeholders of an earlier pass".to_string())],
        });
    }

    fn after_prompts(&self, line: &str) -> String {
        let mut line = line.to_string();
        // Last first: a change made further right leaves the earlier
        // positions where they were.
        for at in restarts(&line).into_iter().rev() {
            let redacted = self.rules(&line[at..]);
            if redacted != line[at..] {
                line.truncate(at);
                line.push_str(&redacted);
            }
        }
        line
    }

    fn values(&self, text: &str) -> String {
        let mut out = text.to_string();
        for value in &self.known {
            out = self.redactor.redact_value(&out, value);
        }
        for (_, value) in self.redactor.export().entries {
            if distinctive(&value) {
                out = self.redactor.redact_value(&out, &value);
            }
        }
        out
    }
}

/// The secrets of every stored credential. A locked vault has nothing to
/// teach: the rules then stand alone, as they do for a model's input.
fn vault_values(app: &App) -> Vec<Zeroizing<String>> {
    let mut known: Vec<Zeroizing<String>> = Vec::new();
    let mut remember = |value: &str| {
        let value = value.trim();
        if value.len() >= MIN_KNOWN_LEN && !known.iter().any(|existing| **existing == *value) {
            known.push(Zeroizing::new(value.to_string()));
        }
    };
    for credential in credentials::list(app).unwrap_or_default() {
        let Ok(resolved) = credentials::resolve(app, &credential.id) else {
            continue;
        };
        for secret in [
            &resolved.password,
            &resolved.enable_secret,
            &resolved.passphrase,
            &resolved.snmp_community,
            &resolved.snmp_auth_password,
            &resolved.snmp_priv_password,
        ] {
            if let Some(value) = secret.as_ref() {
                remember(value);
            }
        }
        // A whole key is caught by the rules; a few lines of one, quoted in
        // an error message or cut by a selection, are not.
        if let Some(key) = resolved.private_key.as_ref() {
            for line in key.lines().filter(|line| line.trim().len() >= 20) {
                remember(line);
            }
        }
    }
    // Longest first, so that a value containing another is replaced whole.
    known.sort_by_key(|value| std::cmp::Reverse(value.len()));
    known
}

/// Whether a value the rules picked up is distinctive enough to hide out of
/// its context. The rules are generous on purpose and also collect the odd
/// ordinary word; replacing those everywhere would shred the text.
fn distinctive(value: &str) -> bool {
    if value.chars().count() < 8 {
        return false;
    }
    let kinds = [
        value.chars().any(|ch| ch.is_ascii_lowercase()),
        value.chars().any(|ch| ch.is_ascii_uppercase()),
        value.chars().any(|ch| ch.is_ascii_digit()),
        value.chars().any(|ch| !ch.is_ascii_alphanumeric()),
    ];
    kinds.into_iter().filter(|present| *present).count() >= 2
}

/// Whether a device type's pattern would find a secret inside a placeholder
/// (`<secret:7>`), which is how one gets cut in two.
fn matches_inside_a_placeholder(pattern: &str) -> bool {
    let Ok(rule) = Regex::new(pattern) else {
        return false;
    };
    [
        "<secret:7>",
        "<secret:7:weak>",
        "key <secret:7> 7",
        "\"<secret:7>\"",
    ]
    .iter()
    .any(|probe| {
        rule.captures_iter(probe).any(|found| {
            let taken = found
                .name("secret")
                .or_else(|| found.get(0))
                .map_or("", |taken| taken.as_str());
            // Taking the whole placeholder is harmless: the engine sees
            // that it is one and leaves it. Taking a piece of it is not.
            !taken.is_empty() && !taken.contains("<secret:")
        })
    })
}

/// The two ends of what a placeholder is replaced by while rules run. Made
/// only of characters the engine takes for an already masked value, so a
/// rule that captures one leaves it alone.
const SHIELD_OPEN: &str = "*X*x*";
const SHIELD_CLOSE: &str = "*x*X*";
const SHIELD_DIGITS: usize = 16;

/// Set the placeholders in `text` aside. Returns the text with each replaced
/// by a stand-in no rule can read anything into, and what each stands for.
fn shield(text: &str) -> (String, Vec<(String, String)>) {
    const OPEN: &str = "<secret:";
    // Text that already holds a stand-in could not be put back together
    // unambiguously. It is left as it is.
    if !text.contains(OPEN) || text.contains(SHIELD_OPEN) {
        return (text.to_string(), Vec::new());
    }
    let mut held: Vec<(String, String)> = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(found) = rest.find(OPEN) {
        out.push_str(&rest[..found]);
        let candidate = &rest[found..];
        let digits = candidate[OPEN.len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .count();
        let after = &candidate[OPEN.len() + digits..];
        let length = match (
            digits,
            after.strip_prefix(":weak>"),
            after.strip_prefix('>'),
        ) {
            (0, _, _) => None,
            (_, Some(_), _) => Some(OPEN.len() + digits + ":weak>".len()),
            (_, None, Some(_)) => Some(OPEN.len() + digits + 1),
            _ => None,
        };
        // Something that only begins like a placeholder is ordinary text.
        let Some(length) = length.filter(|_| held.len() < (1 << SHIELD_DIGITS)) else {
            out.push_str(OPEN);
            rest = &candidate[OPEN.len()..];
            continue;
        };
        let token = &candidate[..length];
        let index = match held.iter().position(|(_, known)| known == token) {
            Some(index) => index,
            None => {
                let number: String = (0..SHIELD_DIGITS)
                    .rev()
                    .map(|bit| if held.len() >> bit & 1 == 1 { 'X' } else { 'x' })
                    .collect();
                held.push((
                    format!("{SHIELD_OPEN}{number}{SHIELD_CLOSE}"),
                    token.to_string(),
                ));
                held.len() - 1
            }
        };
        out.push_str(&held[index].0);
        rest = &candidate[length..];
    }
    out.push_str(rest);
    (out, held)
}

/// Put the placeholders back.
fn unshield(mut text: String, held: &[(String, String)]) -> String {
    for (stand_in, token) in held {
        if text.contains(stand_in.as_str()) {
            text = text.replace(stand_in.as_str(), token);
        }
    }
    text
}

/// Where a typed command may begin inside a line: after a session log's
/// timestamp, and after each of the first few characters a prompt can end in.
fn restarts(line: &str) -> Vec<usize> {
    let mut at: Vec<usize> = Vec::new();
    if let Some((stamp, _)) = line.split_once(' ')
        && chrono::DateTime::parse_from_rfc3339(stamp).is_ok()
    {
        at.push(stamp.len() + 1);
    }
    let from = at.first().copied().unwrap_or(0);
    at.extend(
        line.char_indices()
            .skip_while(|(index, _)| *index < from)
            .take_while(|(index, _)| *index < from + PROMPT_WITHIN)
            .filter(|(_, ch)| matches!(ch, '#' | '>' | '$' | '%'))
            .map(|(index, ch)| index + ch.len_utf8())
            .take(PROMPT_CANDIDATES),
    );
    at.retain(|index| *index < line.len());
    at
}

fn begins_key(line: &str) -> bool {
    line.contains("-----BEGIN ") && line.contains("PRIVATE KEY-----")
}

fn ends_key(line: &str) -> bool {
    line.contains("-----END ") && line.contains("PRIVATE KEY-----")
}

/// A line of a session log without the timestamp it was written under.
fn unstamped(line: &str) -> &str {
    match line.split_once(' ') {
        Some((stamp, rest)) if chrono::DateTime::parse_from_rfc3339(stamp).is_ok() => rest,
        _ => line,
    }
}

/// What can stand between the two markers of a key: base64, a PEM header
/// (`Proc-Type: 4,ENCRYPTED`), or nothing. In a session log every line of
/// it has a timestamp in front, which is not part of the question.
fn key_material(line: &str) -> bool {
    let line = unstamped(line).trim();
    line.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '='))
        || line.split_once(": ").is_some_and(|(name, _)| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        })
}

/// Hide key material whose block is cut off at either end.
///
/// A selection or the last N lines of a session is a window on a longer
/// stream. The rules hide a whole key, from its first marker to its last; a
/// marker without its counterpart would leave whatever part of the key is in
/// the window standing in the clear.
///
/// A key that is cut off runs to the edge of the window, so what is hidden
/// is every line of key material from the marker towards that edge, past
/// the odd line that is something else ([`STRAY_LINES`]): a pager writes
/// `--More--` into the middle of a key as readily as anywhere.
fn hide_cut_off_keys(lines: &mut [String]) {
    let hide = |line: &mut String| {
        if !line.trim().is_empty() {
            *line = HIDDEN.to_string();
        }
    };
    let mut index = 0;
    while index < lines.len() {
        match (begins_key(&lines[index]), ends_key(&lines[index])) {
            (true, false) => {
                // Its last marker, when the window still holds it. That is
                // the stretch the engine's rule takes, whatever is in it.
                let closes = lines[index + 1..]
                    .iter()
                    .take_while(|line| !begins_key(line))
                    .position(|line| ends_key(line));
                if let Some(offset) = closes {
                    // A whole key: the rules replace it, markers and all.
                    index += offset + 2;
                    continue;
                }
                index += 1;
                let mut strays = 0;
                while index < lines.len() && strays < STRAY_LINES && !begins_key(&lines[index]) {
                    if key_material(&lines[index]) {
                        hide(&mut lines[index]);
                        strays = 0;
                    } else {
                        strays += 1;
                    }
                    index += 1;
                }
            }
            (false, true) => {
                hide(&mut lines[index]);
                let mut above = index;
                let mut strays = 0;
                while above > 0 && strays < STRAY_LINES {
                    let line = &lines[above - 1];
                    if line == HIDDEN || begins_key(line) || ends_key(line) {
                        break;
                    }
                    above -= 1;
                    if key_material(&lines[above]) {
                        hide(&mut lines[above]);
                        strays = 0;
                    } else {
                        strays += 1;
                    }
                }
                index += 1;
            }
            _ => index += 1,
        }
    }
}

/// The highest number among the `<secret:N>` placeholders already in `text`
/// that a redactor could have made (see [`HIGHEST_PLAUSIBLE`]).
fn highest_placeholder(text: &str) -> u32 {
    const OPEN: &str = "<secret:";
    let mut highest = 0;
    let mut rest = text;
    while let Some(found) = rest.find(OPEN) {
        rest = &rest[found + OPEN.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(number) = digits.parse::<u32>()
            && number <= HIGHEST_PLAUSIBLE
        {
            highest = highest.max(number);
        }
    }
    highest
}

/// One piece of a drawing, as a parser would cut it.
#[derive(Debug, PartialEq)]
enum Piece<'a> {
    /// What stands between two tags.
    Text(&'a str),
    /// A tag, from `<` to `>`.
    Tag(&'a str),
    /// A comment, a CDATA section or a processing instruction: what opens
    /// it, what it holds, and what closes it.
    Held {
        open: &'a str,
        inner: &'a str,
        close: &'a str,
    },
}

/// Take a drawing apart. Put back together the pieces are the drawing,
/// byte for byte.
fn pieces(svg: &str) -> Vec<Piece<'_>> {
    const HELD: [(&str, &str); 3] = [("<!--", "-->"), ("<![CDATA[", "]]>"), ("<?", "?>")];
    let mut pieces = Vec::new();
    let mut rest = svg;
    while !rest.is_empty() {
        let Some(start) = rest.find('<') else {
            pieces.push(Piece::Text(rest));
            break;
        };
        if start > 0 {
            pieces.push(Piece::Text(&rest[..start]));
            rest = &rest[start..];
        }
        if let Some((open, close)) = HELD.into_iter().find(|(open, _)| rest.starts_with(open)) {
            let inner = &rest[open.len()..];
            match inner.find(close) {
                Some(end) => {
                    pieces.push(Piece::Held {
                        open,
                        inner: &inner[..end],
                        close,
                    });
                    rest = &inner[end + close.len()..];
                }
                // One that never closes: everything after it is inside it.
                None => {
                    pieces.push(Piece::Held {
                        open,
                        inner,
                        close: "",
                    });
                    rest = "";
                }
            }
            continue;
        }
        // A tag ends at the first `>` outside a quoted value: inside one,
        // `>` may stand as it is, and a secret can have one in it.
        let mut quote: Option<char> = None;
        let end = rest.char_indices().find_map(|(index, ch)| {
            match (quote, ch) {
                (None, '>') => return Some(index),
                (None, '"' | '\'') => quote = Some(ch),
                (Some(open), _) if open == ch => quote = None,
                _ => {}
            }
            None
        });
        match end {
            Some(end) => {
                pieces.push(Piece::Tag(&rest[..=end]));
                rest = &rest[end + 1..];
            }
            // A tag that never closes is not markup. It is searched as the
            // text it is, rules and all.
            None => {
                pieces.push(Piece::Text(rest));
                rest = "";
            }
        }
    }
    pieces
}

/// The text a piece of a drawing stands for: the five named entities and
/// the numbered ones. Anything else that starts with `&` is left as it is.
fn xml_unescaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        // The longest reference there is: `&#x10FFFF;`.
        let name = rest[1..]
            .char_indices()
            .take(9)
            .find(|(_, ch)| *ch == ';')
            .map(|(end, _)| &rest[1..=end]);
        let decoded = name.and_then(|name| match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => {
                let number = name.strip_prefix('#')?;
                let code = match number.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => number.parse().ok()?,
                };
                char::from_u32(code)
            }
        });
        match (name, decoded) {
            (Some(name), Some(ch)) => {
                out.push(ch);
                rest = &rest[name.len() + 2..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `text` as it is written into a drawing: nothing in it opens a tag or an
/// entity, and in an attribute's value nothing closes the value.
fn xml_escaped(text: &str, attribute: bool) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\'' if attribute => out.push_str("&apos;"),
            // A character that was written as a reference because it cannot
            // stand as itself goes back as one.
            ch if ch.is_control() && !matches!(ch, '\t' | '\n' | '\r') => {
                out.push_str(&format!("&#{};", u32::from(ch)));
            }
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn a_secret_typed_after_a_prompt_is_hidden_although_the_rule_wants_it_at_the_start_of_a_line() {
        let redactor = EvidenceRedactor::bare(&[]);
        // `password <value>` on a line of its own is the one rule that needs
        // the line to start there: in configuration it does, after a prompt
        // it does not.
        let text = "SW1(config)#line vty 0 4\n\
                    SW1(config-line)#password Pla1nVtyPass\n\
                    [admin@MikroTik] /ip address> password An0therPlainOne\n\
                    2026-11-13T05:24:06.001+02:00  password L0ggedPlainPass\n\
                    2026-11-13T05:24:07.001+02:00 SW1(config-line)#password Stamped1Prompted\n\
                    SW1#show clock";
        let redacted = redactor.text(text);
        for secret in [
            "Pla1nVtyPass",
            "An0therPlainOne",
            "L0ggedPlainPass",
            "Stamped1Prompted",
        ] {
            assert!(!redacted.contains(secret), "{secret} survived:\n{redacted}");
        }
        // The prompts, the timestamps and the lines without a secret are as
        // they were: what is kept is still the transcript.
        assert!(
            redacted.contains("SW1(config-line)#password <secret:"),
            "{redacted}"
        );
        assert!(redacted.contains("2026-11-13T05:24:06.001+02:00  password <secret:"));
        assert!(redacted.starts_with("SW1(config)#line vty 0 4\n"));
        assert!(redacted.ends_with("\nSW1#show clock"));
        assert_eq!(redacted.lines().count(), text.lines().count());
    }

    #[test]
    fn a_value_the_vault_holds_is_hidden_where_no_rule_would_look() {
        let redactor = EvidenceRedactor::bare(&["V4ultOnlyValue"]);
        let redacted = redactor.text("%SYS-5: login failed for V4ultOnlyValue from 10.0.0.9");
        assert!(!redacted.contains("V4ultOnlyValue"), "{redacted}");
        assert!(redacted.contains("login failed for <secret:"), "{redacted}");
        assert_eq!(
            redactor.line("show run of V4ultOnlyValue"),
            redactor.line("show run of V4ultOnlyValue"),
            "the same value keeps the same placeholder"
        );
    }

    #[test]
    fn a_secret_recognised_once_is_hidden_everywhere_else_in_the_same_text() {
        let redactor = EvidenceRedactor::bare(&[]);
        let redacted = redactor.text(
            "snmp-server community Str0ngCommunity RO\n\
             %SNMP-3-AUTHFAIL: Authentication failure, Community Index: Str0ngCommunity",
        );
        assert!(!redacted.contains("Str0ngCommunity"), "{redacted}");
        // An ordinary word a generous rule picked up is not chased through
        // the rest of the text.
        let redacted = redactor.text("snmp-server community campus RO\nthe campus core is SW1");
        assert!(redacted.contains("the campus core is SW1"), "{redacted}");
    }

    #[test]
    fn a_key_cut_off_by_the_edge_of_a_selection_is_hidden_from_either_end() {
        let redactor = EvidenceRedactor::bare(&[]);
        let head = "SW1#show key\n\
                    -----BEGIN RSA PRIVATE KEY-----\n\
                    Proc-Type: 4,ENCRYPTED\n\
                    MIIEowIBAAKCAQEAx1Yd0Zc3kq7v9JmM2bT4n8sQe5uWfR6hLpA3cXyZ0aBdEfGh\n\
                    IjKlMnOpQrStUvWxYz0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJ";
        let redacted = redactor.text(head);
        assert!(!redacted.contains("MIIEowIBAAKCAQEA"), "{redacted}");
        assert!(!redacted.contains("IjKlMnOpQrStUvWx"), "{redacted}");
        assert!(redacted.starts_with("SW1#show key\n"), "{redacted}");

        let tail = "IjKlMnOpQrStUvWxYz0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJ\n\
                    KLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKL\n\
                    -----END RSA PRIVATE KEY-----\n\
                    SW1#";
        let redacted = redactor.text(tail);
        assert!(!redacted.contains("IjKlMnOpQrStUvWx"), "{redacted}");
        assert!(!redacted.contains("KLMNOPQRSTUVWXYZ"), "{redacted}");
        assert!(redacted.ends_with("\nSW1#"), "{redacted}");

        // A whole key is the rules' business and becomes one placeholder.
        let whole = format!("{head}\n-----END RSA PRIVATE KEY-----\nSW1#");
        let redacted = redactor.text(&whole);
        assert!(!redacted.contains("MIIEowIBAAKCAQEA"), "{redacted}");
        assert!(!redacted.contains("PRIVATE KEY"), "{redacted}");
    }

    #[test]
    fn placeholders_of_an_earlier_pass_keep_their_numbers_to_themselves() {
        let redactor = EvidenceRedactor::bare(&[]);
        // `<secret:1>` and `<secret:2>` were put there by the terminal
        // feature, for two values this redactor never saw.
        let redacted = redactor.text(
            "enable secret 5 <secret:1>\n\
             snmp-server community <secret:2> RO\n\
             ntp authentication-key 1 md5 M1ssedByTheFirst 7",
        );
        assert!(!redacted.contains("M1ssedByTheFirst"), "{redacted}");
        assert!(
            redacted.contains("md5 <secret:3>"),
            "a new value gets a number no earlier placeholder has: {redacted}"
        );
        assert!(redacted.contains("enable secret 5 <secret:1>\n"));
        assert_eq!(highest_placeholder("a <secret:12:weak> b <secret:7>"), 12);
        assert_eq!(highest_placeholder("nothing here"), 0);
    }

    /// Two patterns of the shape device types really have: one for a line
    /// of configuration, and one for `secret: value` as a Linux box prints
    /// it. Written for this test; the second is the kind that reads a
    /// placeholder as a credential.
    fn with_a_pattern_that_reads_placeholders(known: &[&str]) -> EvidenceRedactor {
        EvidenceRedactor::with(
            &[
                r"\bwidget (?:secret|password) (?P<secret>\S+)".to_string(),
                r"(?i)\b(?:secret|token)\s*[=:]\s*(?P<secret>\S+)".to_string(),
                "(unclosed".to_string(),
            ],
            known
                .iter()
                .map(|value| Zeroizing::new(value.to_string()))
                .collect(),
        )
    }

    #[test]
    fn a_placeholder_is_never_cut_in_two_by_a_pattern_that_reads_it_as_a_secret() {
        assert!(matches_inside_a_placeholder(
            r"(?i)\b(?:secret|token)\s*[=:]\s*(?P<secret>\S+)"
        ));
        assert!(!matches_inside_a_placeholder(
            r"\bwidget (?:secret|password) (?P<secret>\S+)"
        ));
        // Taking a whole placeholder is not cutting one.
        assert!(!matches_inside_a_placeholder(r"key (?P<secret>\S+) \d"));

        let redactor = with_a_pattern_that_reads_placeholders(&["V4ultOnlyValue"]);
        let redacted = redactor.text(
            "SW1(config)#widget secret Fr0mThePattern\n\
             SW1(config)#enable secret 5 <secret:4>\n\
             token: Fr0mTheOtherPattern\n\
             SW1(config-line)#password Typ3dAfterAPrompt\n\
             login of V4ultOnlyValue failed, widget secret Fr0mThePattern again",
        );
        for secret in [
            "Fr0mThePattern",
            "Fr0mTheOtherPattern",
            "Typ3dAfterAPrompt",
            "V4ultOnlyValue",
        ] {
            assert!(!redacted.contains(secret), "{secret} survived:\n{redacted}");
        }
        // Every `<secret:` opens a well-formed placeholder, and the one that
        // came with the text is as it came.
        let lines: Vec<&str> = redacted.lines().collect();
        assert_eq!(lines[1], "SW1(config)#enable secret 5 <secret:4>");
        let (stripped, held) = shield(&redacted);
        assert!(
            !stripped.contains("<secret:"),
            "a broken placeholder: {redacted}"
        );
        assert_eq!(held.len(), 5, "five different values: {redacted}");
        // The same value has the same placeholder wherever it stands, and
        // no new placeholder took the number of the one that came with the
        // text.
        let widget = lines[0]
            .strip_prefix("SW1(config)#widget secret ")
            .expect("the line as it was, up to the secret");
        assert!(
            lines[4].ends_with(&format!("widget secret {widget} again")),
            "{redacted}"
        );
        let token = lines[2].strip_prefix("token: ").expect("the line");
        assert!(
            token != widget && token != "<secret:4>" && widget != "<secret:4>",
            "{redacted}"
        );
        // And the pattern that does not compile cost nothing but itself.
    }

    #[test]
    fn a_password_prompt_does_not_make_a_secret_of_the_next_line() {
        // `Password:` is the device asking, and nothing was echoed after
        // it. Read across the line break, the pattern for `secret: value`
        // would take `SW1#show` — and then hide it wherever it occurs.
        let redactor = with_a_pattern_that_reads_placeholders(&[]);
        let transcript = "SW1>enable\n\
                          Secret: \n\
                          SW1#show clock\n\
                          12:00:00.000 UTC Fri Nov 13 2026\n\
                          SW1#show version\n\
                          2026-11-13T05:24:03.000+02:00 Token:\n\
                          2026-11-13T05:24:04.000+02:00 SW1#configure terminal";
        assert_eq!(redactor.text(transcript), transcript);
        // On one line it is what the pattern is for.
        assert_eq!(
            redactor.text("Secret: \ntoken = N0tAcrossTheBreak"),
            "Secret: \ntoken = <secret:1>"
        );
    }

    #[test]
    fn a_name_joined_to_what_follows_with_a_colon_reads_as_a_credential() {
        // Why the titles this crate makes itself are written with a dash:
        // a session called `token` is unusual, a title that lost its second
        // word to a placeholder would be worse.
        let redactor = with_a_pattern_that_reads_placeholders(&[]);
        assert_eq!(
            redactor.line("token: last 11 lines"),
            "token: <secret:1> 11 lines"
        );
        assert_eq!(
            redactor.line("token — last 11 lines"),
            "token — last 11 lines"
        );
    }

    #[test]
    fn placeholders_are_set_aside_and_put_back_exactly() {
        let text = "a <secret:1> b <secret:22:weak> c <secret:1> <secret:x> <secret:9 <secret:>";
        let (shielded, held) = shield(text);
        assert_eq!(
            held.iter()
                .map(|(_, token)| token.as_str())
                .collect::<Vec<_>>(),
            ["<secret:1>", "<secret:22:weak>"],
            "only what really is a placeholder, each once"
        );
        assert!(
            shielded.contains("<secret:x> <secret:9 <secret:>"),
            "{shielded}"
        );
        assert!(!shielded.contains("<secret:1>") && !shielded.contains("<secret:22"));
        assert!(
            held.iter()
                .all(|(stand_in, _)| stand_in.chars().all(|ch| matches!(ch, '*' | 'x' | 'X'))),
            "the engine takes a stand-in for a value that is already masked"
        );
        assert_eq!(unshield(shielded, &held), text);
        assert_eq!(
            shield("nothing here"),
            ("nothing here".to_string(), Vec::new())
        );
    }

    #[test]
    fn a_drawing_stays_a_drawing_after_its_secrets_are_hidden() {
        let redactor = EvidenceRedactor::bare(&["Kn0wn&Value<1>"]);
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\">\
                   <title>snmp-server community Dr4wnCommunity RO</title>\
                   <text x=\"1\" data-note=\"Kn0wn&amp;Value&lt;1&gt;\">priv core-sw1</text>\
                   <text>Kn0wn&amp;Value&lt;1&gt; &amp; more</text>\
                   </svg>";
        let redacted = redactor.svg(svg);
        assert!(!redacted.contains("Dr4wnCommunity"), "{redacted}");
        assert!(!redacted.contains("Kn0wn"), "{redacted}");
        // Every `<` still opens a tag that was in the drawing, and every tag
        // is still closed: a placeholder is text, not markup.
        assert_eq!(
            redacted.matches('<').count(),
            svg.matches('<').count(),
            "{redacted}"
        );
        assert_eq!(redacted.matches('>').count(), svg.matches('>').count());
        assert!(redacted.contains("community &lt;secret:"), "{redacted}");
        assert!(redacted.contains("</title><text x=\"1\" data-note=\"&lt;secret:"));
        assert!(redacted.ends_with("</svg>"));
        // A piece nothing was hidden in is byte for byte what it was.
        let plain = "<svg><text>SW1 &gt; SW2</text></svg>";
        assert_eq!(redactor.svg(plain), plain);
    }

    #[test]
    fn json_strings_are_redacted_and_field_names_are_not() {
        let redactor = EvidenceRedactor::bare(&["V4ultOnlyValue"]);
        let value = serde_json::json!({
            "metric": "if.in_bps",
            "instance": "V4ultOnlyValue",
            "points": [[1, 2.5], [2, null]],
        });
        let redacted = redactor.json(&value);
        assert_eq!(redacted["metric"], "if.in_bps");
        assert_eq!(redacted["points"], value["points"]);
        assert!(
            redacted["instance"]
                .as_str()
                .unwrap()
                .starts_with("<secret:")
        );
    }

    /// Not a key: lines of the base64 alphabet, as long as a key's, written
    /// for these tests (synthetic).
    const KEY_LINES: [&str; 3] = [
        "MIIEowIBAAKCAQEAx1Yd0Zc3kq7v9JmM2bT4n8sQe5uWfR6hLpA3cXyZ0aBdEfGh",
        "IjKlMnOpQrStUvWxYz0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJ",
        "KLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKL",
    ];

    /// The lines as a session log holds them: each under its timestamp.
    fn stamped(lines: &[&str]) -> String {
        lines
            .iter()
            .enumerate()
            .map(|(index, line)| format!("2026-11-13T05:24:{index:02}.000+02:00 {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_placeholder_number_no_redactor_made_does_not_run_the_numbering_off_its_end() {
        let redactor = EvidenceRedactor::bare(&[]);
        // A device prints what it likes, the largest number a placeholder
        // can carry included. Counting on from it has nowhere to go.
        let redacted = redactor.text(
            "banner <secret:4294967295>\n\
             enable secret Sup3rS3cretEnable\n\
             snmp-server community C0mmunityStr1ng RO",
        );
        assert!(!redacted.contains("Sup3rS3cretEnable"), "{redacted}");
        assert!(!redacted.contains("C0mmunityStr1ng"), "{redacted}");
        assert!(
            redacted.starts_with("banner <secret:4294967295>\nenable secret <secret:1>\n"),
            "{redacted}"
        );
        assert_eq!(highest_placeholder("<secret:4294967295> <secret:7>"), 7);
        assert_eq!(highest_placeholder("<secret:99999999999999999999>"), 0);
    }

    #[test]
    fn a_key_cut_off_in_a_session_log_is_hidden_although_every_line_carries_a_timestamp() {
        let redactor = EvidenceRedactor::bare(&[]);
        let head = stamped(&[
            "SW1#show key",
            "-----BEGIN RSA PRIVATE KEY-----",
            KEY_LINES[0],
            KEY_LINES[1],
        ]);
        let redacted = redactor.text(&head);
        for line in &KEY_LINES[..2] {
            assert!(!redacted.contains(line), "{redacted}");
        }
        assert!(redacted.contains(" SW1#show key\n"), "{redacted}");

        let tail = stamped(&[
            KEY_LINES[1],
            KEY_LINES[2],
            "-----END RSA PRIVATE KEY-----",
            "SW1#show clock",
        ]);
        let redacted = redactor.text(&tail);
        for line in &KEY_LINES[1..] {
            assert!(!redacted.contains(line), "{redacted}");
        }
        assert!(redacted.ends_with(" SW1#show clock"), "{redacted}");

        // A whole one is still the rules' business, timestamps and all.
        let whole = stamped(&[
            "SW1#show key",
            "-----BEGIN RSA PRIVATE KEY-----",
            KEY_LINES[0],
            KEY_LINES[1],
            KEY_LINES[2],
            "-----END RSA PRIVATE KEY-----",
            "SW1#show clock",
        ]);
        let redacted = redactor.text(&whole);
        for line in KEY_LINES {
            assert!(!redacted.contains(line), "{redacted}");
        }
        assert!(!redacted.contains("PRIVATE KEY"), "{redacted}");
        assert!(redacted.contains(" SW1#show key\n") && redacted.ends_with(" SW1#show clock"));
    }

    #[test]
    fn a_cut_off_key_is_hidden_past_a_line_the_pager_put_in_the_middle_of_it() {
        let redactor = EvidenceRedactor::bare(&[]);
        let head = format!(
            "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n --More-- \n{}\n{}",
            KEY_LINES[0], KEY_LINES[1], KEY_LINES[2]
        );
        let redacted = redactor.text(&head);
        for line in KEY_LINES {
            assert!(!redacted.contains(line), "{redacted}");
        }
        let tail = format!(
            "{}\n --More-- \n{}\n-----END OPENSSH PRIVATE KEY-----\nSW1#",
            KEY_LINES[0], KEY_LINES[1]
        );
        let redacted = redactor.text(&tail);
        for line in &KEY_LINES[..2] {
            assert!(!redacted.contains(line), "{redacted}");
        }
        assert!(redacted.ends_with("\nSW1#"), "{redacted}");
        // What stands well clear of the key is left as it was.
        let after = format!(
            "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\nSW1#show clock\n12:00:00.000 UTC\nSW1#show version\nIOS",
            KEY_LINES[0]
        );
        let redacted = redactor.text(&after);
        assert!(!redacted.contains(KEY_LINES[0]), "{redacted}");
        assert!(
            redacted.ends_with("\nSW1#show clock\n12:00:00.000 UTC\nSW1#show version\nIOS"),
            "{redacted}"
        );
    }

    #[test]
    fn a_key_is_hidden_whole_in_a_string_of_a_chart_and_in_the_text_of_a_drawing() {
        let redactor = EvidenceRedactor::bare(&[]);
        let key = format!(
            "-----BEGIN EC PRIVATE KEY-----\n{}\n{}\n-----END EC PRIVATE KEY-----",
            KEY_LINES[0], KEY_LINES[1]
        );
        let redacted = redactor
            .json(&serde_json::json!({
                "instance": key,
                "note": format!("-----BEGIN EC PRIVATE KEY-----\n{}", KEY_LINES[2]),
            }))
            .to_string();
        for line in KEY_LINES {
            assert!(!redacted.contains(line), "{redacted}");
        }

        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><desc>{key}</desc><text>SW1</text></svg>"
        );
        let redacted = redactor.svg(&svg);
        for line in &KEY_LINES[..2] {
            assert!(!redacted.contains(line), "{redacted}");
        }
        assert!(
            redacted.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\"><desc>&lt;secret:")
                && redacted.ends_with("</desc><text>SW1</text></svg>"),
            "{redacted}"
        );
    }

    #[test]
    fn a_drawing_nothing_is_hidden_in_comes_back_byte_for_byte_however_broken_it_is() {
        let redactor = EvidenceRedactor::bare(&["V4ultOnlyValue"]);
        for drawing in [
            "",
            "plain text, no drawing at all",
            "<",
            ">",
            "<<>>",
            "< x",
            "<svg><!-- never closed",
            "<svg><![CDATA[ never closed",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x.dtd\">\n<svg/>",
            "<a b='x>y' c=\"it's\">t &amp; u &#x41; &bogus; &</a><![CDATA[ <b> ]]> <?pi ?> tail",
            "<text a=\"never closed>text</text>",
            "<svg><text>größe ‹SW1› 交换机 &lt;secret:1&gt;</text></svg>",
        ] {
            assert_eq!(redactor.svg(drawing), drawing);
            let again: String = pieces(drawing)
                .iter()
                .map(|piece| match piece {
                    Piece::Text(text) | Piece::Tag(text) => text.to_string(),
                    Piece::Held { open, inner, close } => format!("{open}{inner}{close}"),
                })
                .collect();
            assert_eq!(again, drawing, "the pieces are the drawing");
        }
        assert_eq!(
            pieces("<g a='>'>x</g><!--c-->"),
            [
                Piece::Tag("<g a='>'>"),
                Piece::Text("x"),
                Piece::Tag("</g>"),
                Piece::Held {
                    open: "<!--",
                    inner: "c",
                    close: "-->"
                },
            ]
        );
        assert_eq!(
            xml_unescaped(
                "&lt;a&gt; &amp;amp; &quot;q&quot; &apos;s&apos; &#65;&#x42; &nope; & &#xZZ;"
            ),
            "<a> &amp; \"q\" 's' AB &nope; & &#xZZ;"
        );
    }

    #[test]
    fn a_value_is_hidden_in_a_drawing_whatever_it_contains_and_wherever_it_stands() {
        // `>` may stand in an attribute's value as it is. A drawing taken
        // apart at every `>` would have half the value in one piece and
        // half in the next, and find it in neither.
        let redactor = EvidenceRedactor::bare(&["P4ss>W0rd\"x"]);
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\"><g data-note='P4ss>W0rd\"x'><text>a &gt; b</text></g></svg>";
        let redacted = redactor.svg(svg);
        assert!(!redacted.contains("P4ss"), "{redacted}");
        assert_eq!(
            redacted,
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><g data-note='&lt;secret:1&gt;'><text>a &gt; b</text></g></svg>"
        );

        // A value a rule recognises in the last label is hidden in the
        // first one too, and in an attribute.
        let redactor = EvidenceRedactor::bare(&[]);
        let redacted = redactor.svg(
            "<svg><g data-label=\"Dr4wnCommunity\"><text>uplink Dr4wnCommunity</text>\
             <text>snmp-server community Dr4wnCommunity RO</text></g></svg>",
        );
        assert!(!redacted.contains("Dr4wnCommunity"), "{redacted}");
        assert_eq!(
            redacted.matches("&lt;secret:1&gt;").count(),
            3,
            "{redacted}"
        );

        // In a drawing a quote is an entity. The rule that takes a quoted
        // value whole has to see the quote.
        let redacted = EvidenceRedactor::bare(&[])
            .svg("<svg><text>snmp-server community &quot;two words&quot; RO</text></svg>");
        assert!(!redacted.contains("words"), "{redacted}");
        assert!(redacted.ends_with(" RO</text></svg>"), "{redacted}");

        // A word a rule took for a secret is not chased into the names the
        // drawing is made of: it would stop being a drawing.
        let redacted = EvidenceRedactor::bare(&[]).svg(
            "<svg><text>password stroke-width</text><line stroke-width=\"2\" data-x=\"stroke-width\"/></svg>",
        );
        assert_eq!(
            redacted,
            "<svg><text>password &lt;secret:1&gt;</text><line stroke-width=\"2\" data-x=\"&lt;secret:1&gt;\"/></svg>"
        );

        // What was written as a reference because it cannot stand as itself
        // is still one after the piece around it was rewritten.
        let redacted = EvidenceRedactor::bare(&[])
            .svg("<svg><text>bell&#7; &amp; enable secret Sup3rS3cretEnable</text></svg>");
        assert_eq!(
            redacted,
            "<svg><text>bell&#7; &amp; enable secret &lt;secret:1&gt;</text></svg>"
        );

        // A comment and a CDATA section keep their ends.
        let redactor = EvidenceRedactor::bare(&["V4ultOnlyValue"]);
        let redacted = redactor.svg(
            "<svg><!-- V4ultOnlyValue > --><text><![CDATA[enable secret Sup3rS3cretEnable]]></text></svg>",
        );
        assert_eq!(
            redacted,
            "<svg><!-- <secret:1> > --><text><![CDATA[enable secret <secret:2>]]></text></svg>"
        );
    }
}
