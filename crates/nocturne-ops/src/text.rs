//! Escaping and small text helpers shared by the exporters and the search.
//!
//! Device names, interface descriptions, alert titles and device output are
//! written by whoever configured (or attacked) the network. Wherever such text
//! is placed into a document it goes through one of these functions first.

/// `text` for an HTML document: nothing in it can open a tag, close an
/// attribute or start an entity.
pub fn html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Whitespace and control characters collapsed so that `text` is one line.
pub fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split(|ch: char| ch.is_whitespace() || ch.is_control()) {
        if word.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// `text` for a Markdown document the text's author does not control: it
/// stays on one line and cannot become emphasis, code, a link, an image, a
/// table cell boundary, a list, a heading or raw HTML.
pub fn md(text: &str) -> String {
    let line = one_line(text);
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len() + 8);
    for (index, &ch) in chars.iter().enumerate() {
        let before = index.checked_sub(1).map(|at| chars[at]);
        let after = chars.get(index + 1).copied();
        let escape = match ch {
            '\\' | '|' | '`' | '*' | '[' | ']' | '<' | '>' | '~' | '&' => true,
            // Inside a word (`core_sw_1`) an underscore is never emphasis.
            '_' => {
                !(before.is_some_and(char::is_alphanumeric)
                    && after.is_some_and(char::is_alphanumeric))
            }
            // At the start of a line these would open a heading or a list.
            '#' => index == 0,
            '-' | '+' => index == 0 && after.is_none_or(|next| next == ' '),
            '.' | ')' => {
                index > 0
                    && chars[..index].iter().all(char::is_ascii_digit)
                    && after.is_none_or(|next| next == ' ')
            }
            _ => false,
        };
        if escape {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// `text` with every control character except the line break and the tab
/// shown as a picture of itself (`␛` for escape, `␈` for backspace).
///
/// Device output can carry a backspace or a carriage return that makes a
/// line read as something else on a terminal. In a document that is
/// evidence, what was printed has to be what is seen.
pub fn visible(text: &str) -> String {
    // A line that ends in CR LF ends once.
    text.replace("\r\n", "\n")
        .chars()
        .map(|ch| match ch {
            '\n' | '\t' => ch,
            '\u{0}'..='\u{1f}' => char::from_u32(0x2400 + u32::from(ch)).unwrap_or('\u{fffd}'),
            '\u{7f}' => '\u{2421}',
            ch if ch.is_control() => '\u{fffd}',
            // Not control characters to Unicode, and they do the same job:
            // an override that makes a line read right to left shows a
            // reader something other than what was printed.
            '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{61c}' => '\u{fffd}',
            ch => ch,
        })
        .collect()
}

/// `text` as a Markdown code block that nothing inside it can close.
///
/// A fence is closed by a line of at least as many backticks as opened it,
/// so the fence is made one longer than the longest run of backticks in the
/// text. Inside a fence Markdown reads nothing: no emphasis, no link, no
/// heading, no HTML.
pub fn fenced(text: &str) -> String {
    let text = visible(text);
    let mut longest = 0;
    let mut run = 0;
    for ch in text.chars() {
        run = if ch == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}text\n{}\n{fence}\n", text.trim_end_matches('\n'))
}

/// One line of device output as a spreadsheet cell: its spacing kept, its
/// control characters visible, and quoted out when it would be taken for a
/// formula. Stricter than a table cell's rule (a leading `-` counts too, and
/// so does a formula behind leading blanks), because here the whole line is
/// the device's.
pub fn cell_line(line: &str) -> String {
    let line = visible(line);
    match line.trim_start().chars().next() {
        Some('=' | '+' | '-' | '@') => format!("'{line}"),
        _ => line,
    }
}

/// A size as people say it: `812 bytes`, `12 KiB`, `1.4 MiB`.
pub fn size(bytes: usize) -> String {
    const KIB: usize = 1024;
    if bytes < KIB {
        format!("{bytes} bytes")
    } else if bytes < KIB * KIB {
        format!("{} KiB", bytes.div_ceil(KIB))
    } else {
        format!("{:.1} MiB", bytes as f64 / (KIB * KIB) as f64)
    }
}

/// The most a free-text field of an incident, a note or a handover holds.
pub const MAX_PROSE_BYTES: usize = 64 * 1024;

/// Refuse prose that is too long to be prose.
///
/// These fields are stored whole inside their record, rewritten on every
/// save and copied into its search text, and nothing in them is redacted.
/// The only thing that makes one this long is pasted device output, and
/// that has a place of its own now.
pub fn prose(what: &str, text: &str) -> nocturne_app::AppResult<()> {
    if text.len() <= MAX_PROSE_BYTES {
        return Ok(());
    }
    Err(nocturne_app::AppError::invalid(format!(
        "{what} can be at most {}; this is {}. Device output belongs in evidence, where it is \
         redacted and keeps its source: capture it from the session with `evidence.capture_terminal`, \
         or keep text you hold with `evidence.add_text`, then attach the item with `evidence.attach`.",
        size(MAX_PROSE_BYTES),
        size(text.len())
    ))
    .with_details(serde_json::json!({
        "limit": "prose_bytes",
        "max": MAX_PROSE_BYTES,
        "actual": text.len(),
        "instead": ["evidence.capture_terminal", "evidence.add_text"],
    })))
}

/// A file-name-safe rendering of a title: lower-case ASCII letters and
/// digits, words joined by dashes, at most `max` characters.
pub fn slug(text: &str, max: usize) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if out.len() >= max {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// A piece of `text` of at most `max` characters around the first place one
/// of the (lower-cased) `words` occurs — the beginning when none does.
pub fn excerpt(text: &str, words: &[String], max: usize) -> String {
    let chars: Vec<char> = one_line(text).chars().collect();
    // One lower-case character per character keeps positions aligned with `chars`.
    let lowered: Vec<char> = chars
        .iter()
        .map(|ch| ch.to_lowercase().next().unwrap_or(*ch))
        .collect();
    let found = words
        .iter()
        .filter_map(|word| {
            let needle: Vec<char> = word.chars().collect();
            if needle.is_empty() || needle.len() > lowered.len() {
                return None;
            }
            lowered
                .windows(needle.len())
                .position(|window| window == needle.as_slice())
        })
        .min();
    let lead = max / 4;
    let start = match found {
        Some(at) if at > lead => at - lead,
        _ => 0,
    };
    let end = (start + max).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn html_escapes_everything_that_could_be_markup() {
        assert_eq!(
            html(r#"<script>alert("x")</script> & 'q'"#),
            "&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; &#39;q&#39;"
        );
        assert_eq!(html("plain ünïcode"), "plain ünïcode");
    }

    #[test]
    fn markdown_text_cannot_become_markup() {
        assert_eq!(md("uplink | core"), r"uplink \| core");
        assert_eq!(
            md("<script>alert(1)</script>"),
            r"\<script\>alert(1)\</script\>"
        );
        assert_eq!(md("[click](javascript:x)"), r"\[click\](javascript:x)");
        assert_eq!(md("![img](x)"), r"!\[img\](x)");
        assert_eq!(md("*bold* and `code`"), r"\*bold\* and \`code\`");
        assert_eq!(md("a\\b"), r"a\\b");
        assert_eq!(md("&lt;b&gt;"), r"\&lt;b\&gt;");
        // Names with underscores stay readable; a leading or trailing one is escaped.
        assert_eq!(md("core_sw_1"), "core_sw_1");
        assert_eq!(md("_lab_"), r"\_lab\_");
        // Several lines become one.
        assert_eq!(md("  line one\r\n\tline two  "), "line one line two");
        // Nothing at the start of a line can open a block.
        assert_eq!(md("# not a heading"), r"\# not a heading");
        assert_eq!(md("- not a list"), r"\- not a list");
        assert_eq!(md("12. not a list"), r"12\. not a list");
        assert_eq!(md("-core-1"), "-core-1");
        assert_eq!(md("10.0.0.1"), "10.0.0.1");
        assert_eq!(md("C#"), "C#");
    }

    #[test]
    fn control_characters_are_shown_rather_than_obeyed() {
        // A backspace that would wipe "rm -rf" off a terminal, an escape
        // sequence, a bare carriage return that would overprint the line.
        assert_eq!(
            visible("ok\u{8}\u{8}rm -rf\r\nnext\u{1b}[2Jline\rover\ttab\u{7f}\u{0}\u{85}"),
            "ok␈␈rm -rf\nnext␛[2Jline␍over\ttab␡␀\u{fffd}"
        );
        // An override that would show `no shutdown` where `shutdown` was
        // printed is not a control character, and is not obeyed either.
        assert_eq!(
            visible("int Gi0/1 \u{202e}nwodtuhs on\u{202c} \u{2066}x\u{2069}\u{200f}"),
            "int Gi0/1 \u{fffd}nwodtuhs on\u{fffd} \u{fffd}x\u{fffd}\u{fffd}"
        );
    }

    #[test]
    fn a_code_fence_is_longer_than_any_fence_in_the_text() {
        assert_eq!(
            fenced("show clock\n12:00\n"),
            "```text\nshow clock\n12:00\n```\n"
        );
        let hostile = "ok\n```\n# pwned\n````` five\n";
        let block = fenced(hostile);
        let lines: Vec<&str> = block.lines().collect();
        assert_eq!(
            lines.first(),
            Some(&"``````text"),
            "six, one more than five"
        );
        assert_eq!(lines.last(), Some(&"``````"));
        // Everything between the two fences is the text, untouched.
        assert_eq!(lines[1..lines.len() - 1].join("\n"), hostile.trim_end());
    }

    #[test]
    fn a_line_of_output_cannot_be_a_formula() {
        assert_eq!(
            cell_line("=HYPERLINK(\"http://x\")"),
            "'=HYPERLINK(\"http://x\")"
        );
        assert_eq!(cell_line("   +cmd|' /C calc'!A0"), "'   +cmd|' /C calc'!A0");
        assert_eq!(cell_line("-----BEGIN"), "'-----BEGIN");
        assert_eq!(cell_line("@SUM(1)"), "'@SUM(1)");
        // Ordinary output keeps its columns.
        assert_eq!(cell_line("Gi0/1   up    up"), "Gi0/1   up    up");
    }

    #[test]
    fn prose_past_the_limit_is_refused_with_where_to_put_it() {
        assert!(prose("a note", &"x".repeat(MAX_PROSE_BYTES)).is_ok());
        let refused = prose("a note", &"x".repeat(MAX_PROSE_BYTES + 1)).unwrap_err();
        assert_eq!(refused.code, "invalid");
        assert!(refused.message.contains("64 KiB"), "{}", refused.message);
        assert!(refused.message.contains("evidence.add_text"));
        assert_eq!(size(812), "812 bytes");
        assert_eq!(size(1025), "2 KiB");
        assert_eq!(size(3 * 1024 * 1024 / 2), "1.5 MiB");
    }

    #[test]
    fn slugs_are_safe_file_names() {
        assert_eq!(
            slug("Core switch DOWN in HQ!", 40),
            "core-switch-down-in-hq"
        );
        assert_eq!(slug("../../etc/passwd", 40), "etc-passwd");
        assert_eq!(slug("a very long title indeed", 10), "a-very-lon");
        assert_eq!(slug("日本語", 40), "");
    }

    #[test]
    fn excerpts_centre_on_the_first_match() {
        let text = "To fail over the WAN link, shut the primary uplink on EDGE-RTR1 and \
                    watch BGP converge on the backup.\nThen tell the NOC.";
        let words = vec!["bgp".to_string()];
        // "BGP" is the 75th character; the piece starts a quarter of its length before it.
        assert_eq!(
            excerpt(text, &words, 40),
            "…and watch BGP converge on the backup. Th…"
        );
        assert_eq!(
            excerpt(text, &["nothing".into()], 20),
            "To fail over the WAN…"
        );
        assert_eq!(excerpt("short", &words, 40), "short");
        assert_eq!(
            excerpt("ÜBER den Wolken", &["über".into()], 40),
            "ÜBER den Wolken"
        );
    }
}
