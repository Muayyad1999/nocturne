//! Python string semantics the engine depends on.

/// `str.isspace()` for one character: Unicode white space plus the ASCII
/// separators FS, GS, RS and US.
pub(crate) fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// Whether `c` is a word character in the sense of Python's `\w`.
pub(crate) fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `str.rstrip()`.
pub(crate) fn rstrip(text: &str) -> &str {
    text.trim_end_matches(is_space)
}

/// `str.strip()`.
pub(crate) fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `str.splitlines()`: besides `\n`, `\r` and `\r\n`, Python treats vertical
/// tab, form feed, the separators FS, GS and RS, NEL and the Unicode line and
/// paragraph separators as line boundaries. A trailing boundary does not
/// produce a final empty line.
pub(crate) fn split_lines(text: &str) -> SplitLines<'_> {
    SplitLines { rest: text }
}

pub(crate) struct SplitLines<'a> {
    rest: &'a str,
}

impl<'a> Iterator for SplitLines<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.rest.is_empty() {
            return None;
        }
        let is_boundary = |c: char| {
            matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c'..='\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}')
        };
        let Some((end, boundary)) = self.rest.char_indices().find(|&(_, c)| is_boundary(c)) else {
            return Some(std::mem::take(&mut self.rest));
        };
        let line = &self.rest[..end];
        let after = &self.rest[end + boundary.len_utf8()..];
        self.rest = match (boundary, after.strip_prefix('\n')) {
            ('\r', Some(after_crlf)) => after_crlf,
            _ => after,
        };
        Some(line)
    }
}

/// Lines of a template as Python yields them when iterating over a file
/// opened in text mode: split at `\n`, `\r\n` or `\r`, without the terminator.
pub(crate) fn template_lines(source: &str) -> impl Iterator<Item = &str> {
    let mut rest = source;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = rest.find(['\n', '\r']).unwrap_or(rest.len());
        let line = &rest[..end];
        rest = &rest[end..];
        rest = rest
            .strip_prefix("\r\n")
            .or_else(|| rest.strip_prefix(['\n', '\r']))
            .unwrap_or(rest);
        Some(line)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<&str> {
        split_lines(text).collect()
    }

    #[test]
    fn split_lines_follows_python() {
        // Expected values are the output of `str.splitlines()` in CPython 3.13.
        assert_eq!(lines(""), Vec::<&str>::new());
        assert_eq!(lines("a"), ["a"]);
        assert_eq!(lines("a\n"), ["a"]);
        assert_eq!(lines("a\n\n"), ["a", ""]);
        assert_eq!(lines("\n"), [""]);
        assert_eq!(lines("a\nb"), ["a", "b"]);
        assert_eq!(lines("a\r\nb\r\n"), ["a", "b"]);
        assert_eq!(lines("a\rb"), ["a", "b"]);
        assert_eq!(lines("a\r\r\nb"), ["a", "", "b"]);
        assert_eq!(lines("a\n\rb"), ["a", "", "b"]);
        assert_eq!(lines("a\x0bb\x0cc"), ["a", "b", "c"]);
        assert_eq!(lines("a\x1cb\x1dc\x1ed\x1fe"), ["a", "b", "c", "d\x1fe"]);
        assert_eq!(lines("a\u{85}b\u{2028}c\u{2029}d"), ["a", "b", "c", "d"]);
        assert_eq!(lines("é\nü"), ["é", "ü"]);
    }

    #[test]
    fn template_lines_use_universal_newlines() {
        let collect = |source| template_lines(source).collect::<Vec<_>>();
        assert_eq!(collect(""), Vec::<&str>::new());
        assert_eq!(collect("a\nb"), ["a", "b"]);
        assert_eq!(collect("a\r\nb\r\n"), ["a", "b"]);
        assert_eq!(collect("a\rb\n\nc"), ["a", "b", "", "c"]);
        // Form feeds and the like are not line boundaries for file iteration.
        assert_eq!(collect("a\x0cb"), ["a\x0cb"]);
    }

    #[test]
    fn strip_uses_python_white_space() {
        assert_eq!(rstrip("a \t\x1c\u{a0}"), "a");
        assert_eq!(strip("\x1f a b \r"), "a b");
        assert!(is_word('é') && is_word('_') && is_word('7') && !is_word('-'));
    }
}
