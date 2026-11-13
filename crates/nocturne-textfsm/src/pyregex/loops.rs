//! Loops whose body can match the empty string.
//!
//! This is the one place where a backtracking engine like Python's and the
//! automata behind the `regex` crate disagree on what a pattern means.
//!
//! Python runs the body of `(x)*` once more after every iteration that
//! consumed text. If the body matches the empty string at that point, the
//! iteration counts, its groups are captured, and the loop ends there:
//! `(\s*-*)*` always leaves its group empty, whatever it matched before. The
//! Rust engines never take an iteration that consumes nothing; they would
//! report the last iteration that did.
//!
//! For the shape that occurs in practice the two can be reconciled. If the
//! body matches the empty string in exactly one way, and that way is tried
//! only after every alternative that consumes text (`a*b?`, `x+|\s*`), both
//! engines walk through the same iterations, and Python then appends the empty
//! one. [`final_empty_iteration_groups`] names the groups that iteration
//! captures; the emitter marks the end of such a loop with an empty group, and
//! when the marker took part in the match those groups read as `""`.
//!
//! # The remaining difference
//!
//! Other loops over a possibly empty body are translated as they are and keep
//! the Rust behaviour:
//!
//! * unbounded loops whose body can be empty before another alternative is
//!   tried (`(a*|b)*`), lazily (`(a*?)*`) or depending on an assertion or a
//!   back-reference (`(a*$)*`), and lazy loops over a possibly empty body
//!   (`(a*)*?`): Python may end the loop earlier than Rust does, and the
//!   groups inside may capture different iterations;
//! * bounded loops such as `(x?){2,3}` are unrolled by the Rust engines, which
//!   agrees with Python unless the body refers back to a group or tests one
//!   with `(?(1)...)`, so that it behaves differently after an empty iteration.
//!
//! None of the bundled templates contains such a loop. The test helper
//! `has_divergent_loop` recognises them, so that the comparisons with Python
//! on random patterns can tell this difference from a bug.

use super::ast::{GroupKind, Node, RepeatMode};

/// How a sub-pattern can match the empty string.
#[derive(Debug, PartialEq, Eq)]
enum EmptyMatch {
    /// Not at all.
    Never,
    /// In exactly one way, tried after every way that consumes text. Holds the
    /// capture groups that take part in that match.
    Last(Vec<u32>),
    /// In some other way, or depending on the context.
    Other,
}

/// The capture groups that are reset to `""` by the final empty iteration of
/// `repeat`, if `repeat` is a loop for which that iteration can be emulated
/// and it captures anything.
pub(super) fn final_empty_iteration_groups(repeat: &Node) -> Option<Vec<u32>> {
    let Node::Repeat {
        node: body,
        max: None,
        mode: RepeatMode::Greedy | RepeatMode::Possessive,
        ..
    } = repeat
    else {
        return None;
    };
    match empty_match(body) {
        EmptyMatch::Last(groups) if !groups.is_empty() => Some(groups),
        _ => None,
    }
}

fn empty_match(node: &Node) -> EmptyMatch {
    use EmptyMatch::{Last, Never, Other};
    match node {
        Node::Empty => Last(Vec::new()),
        Node::Literal(_) | Node::Dot | Node::Class(_) => Never,
        Node::Assertion(_) | Node::Look { .. } | Node::Backref(_) => Other,
        Node::Concat(nodes) => {
            // The sequence is empty only if every part is; skipping every part
            // is the last combination the engines try.
            let mut groups = Vec::new();
            let mut all_last = true;
            for node in nodes {
                match empty_match(node) {
                    Never => return Never,
                    Last(found) => groups.extend(found),
                    Other => all_last = false,
                }
            }
            if all_last { Last(groups) } else { Other }
        }
        Node::Alternation(branches) => match branches.split_last() {
            // An empty match of an earlier branch would come before the
            // matches of the later ones.
            Some((last, earlier)) if earlier.iter().all(|branch| empty_match(branch) == Never) => {
                empty_match(last)
            }
            _ => Other,
        },
        Node::Repeat {
            node, min, mode, ..
        } => match (empty_match(node), min, mode) {
            // Greedy: zero repetitions come last.
            (Never, 0, RepeatMode::Greedy) => Last(Vec::new()),
            (Never, 0, _) => Other,
            (Never, _, _) => Never,
            _ => Other,
        },
        Node::Group { kind, node } => match (kind, empty_match(node)) {
            (_, Never) => Never,
            (GroupKind::Capture(number), Last(mut groups)) => {
                groups.push(*number);
                Last(groups)
            }
            (GroupKind::NonCapture | GroupKind::Flags { .. }, Last(groups)) => Last(groups),
            _ => Other,
        },
        Node::Conditional { yes, no, .. } => match (empty_match(yes), empty_match(no)) {
            (Never, Never) => Never,
            _ => Other,
        },
    }
}

/// Whether the tree contains a loop on which this crate and Python may
/// disagree (see the module documentation). The tests that compare against
/// Python on random patterns use it to tell known differences from bugs.
#[cfg(test)]
pub(super) fn has_divergent_loop(node: &Node) -> bool {
    fn refers_to_groups(node: &Node) -> bool {
        match node {
            Node::Backref(_) | Node::Conditional { .. } => true,
            Node::Concat(nodes) | Node::Alternation(nodes) => nodes.iter().any(refers_to_groups),
            Node::Group { node, .. } | Node::Look { node, .. } | Node::Repeat { node, .. } => {
                refers_to_groups(node)
            }
            _ => false,
        }
    }
    let children: Vec<&Node> = match node {
        Node::Concat(nodes) | Node::Alternation(nodes) => nodes.iter().collect(),
        Node::Group { node, .. } | Node::Look { node, .. } => vec![node],
        Node::Conditional { yes, no, .. } => vec![yes, no],
        Node::Repeat {
            node: body,
            max,
            mode,
            ..
        } => {
            let divergent = match (empty_match(body), max) {
                (EmptyMatch::Never, _) | (_, Some(0 | 1)) => false,
                // An unbounded loop that is not emulated.
                (EmptyMatch::Other, None) => true,
                (EmptyMatch::Last(_), None) => *mode == RepeatMode::Lazy,
                // A bounded loop is unrolled, which goes wrong only if an
                // iteration can behave differently after an empty one.
                (_, Some(_)) => refers_to_groups(body),
            };
            if divergent {
                return true;
            }
            vec![body]
        }
        _ => Vec::new(),
    };
    children.into_iter().any(has_divergent_loop)
}

#[cfg(test)]
mod tests {
    use super::super::parser;
    use super::*;

    /// The groups reset by the final empty iteration of the first loop in `pattern`.
    fn reset_groups(pattern: &str) -> Option<Vec<u32>> {
        fn first_loop(node: &Node) -> Option<&Node> {
            match node {
                Node::Repeat { max: None, .. } => Some(node),
                Node::Concat(nodes) | Node::Alternation(nodes) => nodes.iter().find_map(first_loop),
                Node::Group { node, .. } | Node::Look { node, .. } | Node::Repeat { node, .. } => {
                    first_loop(node)
                }
                _ => None,
            }
        }
        let ast = parser::parse(pattern).unwrap();
        let mut groups = final_empty_iteration_groups(first_loop(&ast.root).unwrap())?;
        groups.sort_unstable();
        Some(groups)
    }

    #[test]
    fn bodies_that_end_with_one_empty_match() {
        assert_eq!(reset_groups(r"(\s*-*)*"), Some(vec![1]));
        assert_eq!(reset_groups(r"(a?)+"), Some(vec![1]));
        assert_eq!(reset_groups(r"(?:(a*)(b{0,3}))*"), Some(vec![1, 2]));
        assert_eq!(reset_groups(r"((a)|b*)*"), Some(vec![1]));
        assert_eq!(reset_groups(r"(?:x+|(?i:(y*)))*"), Some(vec![1]));
        assert_eq!(reset_groups(r"((a)|(b*)){2,}"), Some(vec![1, 3]));
        assert_eq!(reset_groups(r"(a|)*"), Some(vec![1]));
        assert_eq!(reset_groups(r"(a*)*+"), Some(vec![1]));
    }

    #[test]
    fn loops_that_need_no_emulation() {
        // The body always consumes text.
        assert_eq!(reset_groups(r"(\w+\s*)*"), None);
        assert_eq!(reset_groups(r"(a|b)+"), None);
        // No group takes part in the empty match.
        assert_eq!(reset_groups(r"(?:a*(?:b|c?))*"), None);
        assert_eq!(reset_groups(r"(?:(a)|b*)*"), None);
    }

    #[test]
    fn loops_that_are_left_alone() {
        // The empty match is not the last one tried.
        assert_eq!(reset_groups(r"(a*|b)*"), None);
        assert_eq!(reset_groups(r"(|a)*"), None);
        assert_eq!(reset_groups(r"(a*?)*"), None);
        assert_eq!(reset_groups(r"(a??)*"), None);
        // Emptiness depends on the context.
        assert_eq!(reset_groups(r"(a*$)*"), None);
        assert_eq!(reset_groups(r"(a*\b)*"), None);
        assert_eq!(reset_groups(r"(a*(?=b))*"), None);
        assert_eq!(reset_groups(r"(x)(\1?)*"), None);
        // Nested loops over something empty.
        assert_eq!(reset_groups(r"((?:a*)*)*"), None);
        // Lazy loops.
        assert_eq!(reset_groups(r"(a*)*?"), None);
    }
}
