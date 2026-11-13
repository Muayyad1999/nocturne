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
            '_' => !(before.is_some_and(char::is_alphanumeric)
                && after.is_some_and(char::is_alphanumeric)),
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
        assert_eq!(md("<script>alert(1)</script>"), r"\<script\>alert(1)\</script\>");
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
    fn slugs_are_safe_file_names() {
        assert_eq!(slug("Core switch DOWN in HQ!", 40), "core-switch-down-in-hq");
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
        assert_eq!(excerpt(text, &["nothing".into()], 20), "To fail over the WAN…");
        assert_eq!(excerpt("short", &words, 40), "short");
        assert_eq!(excerpt("ÜBER den Wolken", &["über".into()], 40), "ÜBER den Wolken");
    }
}
