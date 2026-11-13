//! Enumeration of the strings a simple pattern spells out.

use super::ast::{GroupKind, Node};
use super::parser;

/// The strings `pattern` matches in full, if it is built from literals,
/// groups, alternation and `?` only and matches at most `limit` strings.
///
/// `(hp|aruba)_procurve` yields `aruba_procurve` and `hp_procurve`; a pattern
/// such as `.*vyos.*` yields `None`.
pub(crate) fn finite_language(pattern: &str, limit: usize) -> Option<Vec<String>> {
    let ast = parser::parse(pattern).ok()?;
    if !ast.flags.is_empty() {
        return None;
    }
    let mut strings = language(&ast.root, limit)?;
    strings.sort();
    strings.dedup();
    Some(strings)
}

fn language(node: &Node, limit: usize) -> Option<Vec<String>> {
    let strings = match node {
        Node::Empty => vec![String::new()],
        Node::Literal(c) => vec![c.to_string()],
        Node::Concat(nodes) => {
            let mut prefixes = vec![String::new()];
            for node in nodes {
                let suffixes = language(node, limit)?;
                prefixes = prefixes
                    .iter()
                    .flat_map(|prefix| suffixes.iter().map(move |suffix| format!("{prefix}{suffix}")))
                    .collect();
                if prefixes.len() > limit {
                    return None;
                }
            }
            prefixes
        }
        Node::Alternation(branches) => {
            let mut strings = Vec::new();
            for branch in branches {
                strings.extend(language(branch, limit)?);
            }
            strings
        }
        Node::Group { kind: GroupKind::Capture(_) | GroupKind::NonCapture, node } => {
            language(node, limit)?
        }
        Node::Repeat { node, min: 0, max: Some(1), .. } => {
            let mut strings = language(node, limit)?;
            strings.push(String::new());
            strings
        }
        _ => return None,
    };
    (strings.len() <= limit).then_some(strings)
}

#[cfg(test)]
mod tests {
    use super::finite_language;

    #[test]
    fn enumerates_simple_patterns() {
        assert_eq!(finite_language("cisco_ios", 8), Some(vec!["cisco_ios".to_owned()]));
        assert_eq!(
            finite_language("(hp|aruba)_procurve", 8),
            Some(vec!["aruba_procurve".to_owned(), "hp_procurve".to_owned()])
        );
        assert_eq!(
            finite_language("a(?:b|c)?d?", 8).map(|strings| strings.join(",")),
            Some("a,ab,abd,ac,acd,ad".to_owned())
        );
    }

    #[test]
    fn gives_up_on_open_ended_patterns() {
        assert_eq!(finite_language(".*vyos.*", 8), None);
        assert_eq!(finite_language(r"cisco_\w+", 8), None);
        assert_eq!(finite_language("a+", 8), None);
        assert_eq!(finite_language("(?i)abc", 8), None);
        assert_eq!(finite_language("(a|b)(c|d)(e|f)", 4), None);
        assert_eq!(finite_language("(", 8), None);
    }
}
