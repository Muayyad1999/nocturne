//! Text helpers shared by the CLI renderers.

/// Truncate to at most `max` characters (never splits a character).
pub(crate) fn clip(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Case-insensitive "`full` starts with `abbrev`" (ASCII), the CLI abbreviation rule.
pub(crate) fn is_abbrev(abbrev: &str, full: &str) -> bool {
    !abbrev.is_empty()
        && abbrev.len() <= full.len()
        && full.as_bytes()[..abbrev.len()].eq_ignore_ascii_case(abbrev.as_bytes())
}

/// Length of the longest common case-insensitive prefix.
pub(crate) fn common_prefix_len(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x.eq_ignore_ascii_case(y)).count()
}

/// Compress a sorted list of numbers into `1,3-5,9` form.
pub(crate) fn ranges(sorted: &[u16]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        let mut end = start;
        while i + 1 < sorted.len() && sorted[i + 1] == end.saturating_add(1) {
            i += 1;
            end = sorted[i];
        }
        if !out.is_empty() {
            out.push(',');
        }
        out.push_str(&start.to_string());
        if end != start {
            out.push('-');
            out.push_str(&end.to_string());
        }
        i += 1;
    }
    out
}

/// Parse `1,3-5,9` into a sorted, de-duplicated list within `lo..=hi`.
pub(crate) fn parse_ranges(s: &str, lo: u16, hi: u16) -> Option<Vec<u16>> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (a.trim().parse::<u16>().ok()?, b.trim().parse::<u16>().ok()?),
            None => {
                let v = part.parse::<u16>().ok()?;
                (v, v)
            }
        };
        if a > b || a < lo || b > hi {
            return None;
        }
        out.extend(a..=b);
    }
    out.sort_unstable();
    out.dedup();
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(clip("héllo", 2), "hé");
        assert_eq!(clip("ab", 5), "ab");
        assert!(is_abbrev("sh", "show") && is_abbrev("SHOW", "show"));
        assert!(!is_abbrev("", "show") && !is_abbrev("shows", "show") && !is_abbrev("sx", "show"));
        assert_eq!(common_prefix_len("interfaze", "interface"), 7);
        assert_eq!(ranges(&[1, 3, 4, 5, 9]), "1,3-5,9");
        assert_eq!(ranges(&[]), "");
        assert_eq!(parse_ranges("10,20,30-32", 1, 4094), Some(vec![10, 20, 30, 31, 32]));
        assert_eq!(parse_ranges("5-3", 1, 4094), None);
        assert_eq!(parse_ranges("0", 1, 4094), None);
        assert_eq!(parse_ranges("a", 1, 4094), None);
    }
}
