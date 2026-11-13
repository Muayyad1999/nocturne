//! Comparing a configuration with another one.
//!
//! Rendered text is the wrong thing to compare: a device puts a new `vlan 30`
//! among the other VLANs while the prediction appends it at the end, and
//! nobody would call that a difference. What matters is the set of lines and
//! the section each one is in, so that is what is compared — and what is
//! reported when they differ.

use std::collections::BTreeSet;

use nocturne_drivers::hier::{ConfigTree, Node, Syntax};

/// Every line of a configuration, each with the section it sits in.
pub fn lines_of(tree: &ConfigTree) -> BTreeSet<String> {
    fn walk(nodes: &[Node], prefix: &str, out: &mut BTreeSet<String>) {
        for node in nodes {
            let path = if prefix.is_empty() {
                node.text.trim().to_string()
            } else {
                format!("{prefix} / {}", node.text.trim())
            };
            out.insert(path.clone());
            walk(&node.children, &path, out);
        }
    }
    let mut lines = BTreeSet::new();
    walk(&tree.roots, "", &mut lines);
    lines
}

/// What is in `expected` but not on the device, and what is on the device but
/// was not expected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Differences {
    pub missing: Vec<String>,
    pub unexpected: Vec<String>,
}

impl Differences {
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.unexpected.is_empty()
    }

    /// A sentence for a check's detail, listing at most `limit` lines of each.
    pub fn describe(&self, limit: usize) -> String {
        let show = |label: &str, lines: &[String]| {
            if lines.is_empty() {
                return String::new();
            }
            let shown: Vec<String> = lines
                .iter()
                .take(limit)
                .map(|line| format!("`{line}`"))
                .collect();
            let more = lines.len().saturating_sub(shown.len());
            format!(
                "{label}: {}{}",
                shown.join(", "),
                if more > 0 {
                    format!(" (and {more} more)")
                } else {
                    String::new()
                }
            )
        };
        [
            show("missing from the device", &self.missing),
            show("on the device but not predicted", &self.unexpected),
        ]
        .iter()
        .filter(|part| !part.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("; ")
    }
}

pub fn differences(expected: &ConfigTree, actual: &ConfigTree) -> Differences {
    let expected = lines_of(expected);
    let actual = lines_of(actual);
    Differences {
        missing: expected.difference(&actual).cloned().collect(),
        unexpected: actual.difference(&expected).cloned().collect(),
    }
}

/// The same, from text.
pub fn differences_of(expected: &str, actual: &str, syntax: Syntax) -> Differences {
    differences(
        &ConfigTree::parse(expected, syntax),
        &ConfigTree::parse(actual, syntax),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_is_not_a_difference_but_content_is() {
        let a = "vlan 10\n name Staff\nvlan 20\n name Guests\ninterface Gi0/1\n description x\n";
        let reordered =
            "interface Gi0/1\n description x\nvlan 20\n name Guests\nvlan 10\n name Staff\n";
        assert!(differences_of(a, reordered, Syntax::Ios).is_empty());

        let changed = "vlan 10\n name Staff\ninterface Gi0/1\n description y\n";
        let differences = differences_of(a, changed, Syntax::Ios);
        assert_eq!(
            differences.missing,
            vec![
                "interface Gi0/1 / description x",
                "vlan 20",
                "vlan 20 / name Guests"
            ]
        );
        assert_eq!(
            differences.unexpected,
            vec!["interface Gi0/1 / description y"]
        );
        assert!(
            differences
                .describe(2)
                .contains("missing from the device: `interface Gi0/1 / description x`"),
            "{}",
            differences.describe(2)
        );
        assert!(differences.describe(2).contains("and 1 more"));
    }
}
