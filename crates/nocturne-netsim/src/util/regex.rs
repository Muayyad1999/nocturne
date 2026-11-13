//! A tiny backtracking regular-expression engine for the CLI output modifiers
//! (`| include`, `| begin`, ...). Supports literals, `.`, `*`, `+`, `?`, `^`, `$`,
//! `[...]`, `|`, `(...)`, `\` escapes and `_` (delimiter: space, comma, brace,
//! bracket or start/end of line), which covers what operators type in practice.

use std::cell::Cell;

#[derive(Debug, Clone)]
enum Node {
    Char(char),
    Any,
    Class { set: Vec<(char, char)>, negated: bool },
    Start,
    End,
    Delim,
    Cat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat { inner: Box<Node>, min: u32, many: bool },
}

#[derive(Debug, Clone)]
pub(crate) struct Regex {
    root: Node,
}

/// Work bound per match: prevents pathological patterns from stalling the lab.
const STEP_BUDGET: u32 = 20_000;
const MAX_PATTERN: usize = 256;
/// Also bounds the recursion depth of the matcher.
const MAX_TEXT: usize = 512;

impl Regex {
    /// `None` when the pattern is malformed (unbalanced brackets, dangling operator).
    pub(crate) fn new(pattern: &str) -> Option<Regex> {
        let chars: Vec<char> = pattern.chars().collect();
        if chars.len() > MAX_PATTERN {
            return None;
        }
        let mut parser = Parser { chars: &chars, pos: 0, depth: 0 };
        let root = parser.alternation()?;
        (parser.pos == chars.len()).then_some(Regex { root })
    }

    /// Unanchored search, like `grep`.
    pub(crate) fn is_match(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().take(MAX_TEXT).collect();
        let budget = Cell::new(STEP_BUDGET);
        (0..=chars.len()).any(|start| run(&self.root, &chars, start, &budget, &mut |_| true))
    }
}

struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
    depth: u32,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn alternation(&mut self) -> Option<Node> {
        let mut branches = vec![self.sequence()?];
        while self.peek() == Some('|') {
            self.pos += 1;
            branches.push(self.sequence()?);
        }
        Some(if branches.len() == 1 { branches.remove(0) } else { Node::Alt(branches) })
    }

    fn sequence(&mut self) -> Option<Node> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom()?;
            items.push(self.quantified(atom));
        }
        Some(Node::Cat(items))
    }

    fn quantified(&mut self, atom: Node) -> Node {
        let (min, many) = match self.peek() {
            Some('*') => (0, true),
            Some('+') => (1, true),
            Some('?') => (0, false),
            _ => return atom,
        };
        self.pos += 1;
        // Collapse stacked quantifiers (`a**`) instead of nesting them.
        while matches!(self.peek(), Some('*' | '+' | '?')) {
            self.pos += 1;
        }
        Node::Repeat { inner: Box::new(atom), min, many }
    }

    fn atom(&mut self) -> Option<Node> {
        let c = self.peek()?;
        self.pos += 1;
        Some(match c {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '_' => Node::Delim,
            '(' => {
                self.depth += 1;
                if self.depth > 16 {
                    return None;
                }
                let inner = self.alternation()?;
                if self.peek() != Some(')') {
                    return None;
                }
                self.pos += 1;
                self.depth -= 1;
                inner
            }
            '[' => self.class()?,
            '\\' => {
                let escaped = self.peek()?;
                self.pos += 1;
                Node::Char(escaped)
            }
            // A quantifier with nothing to repeat is a literal, as in IOS.
            other => Node::Char(other),
        })
    }

    fn class(&mut self) -> Option<Node> {
        let negated = self.peek() == Some('^');
        if negated {
            self.pos += 1;
        }
        let mut set = Vec::new();
        let mut first = true;
        loop {
            let c = self.peek()?;
            self.pos += 1;
            if c == ']' && !first {
                break;
            }
            first = false;
            if self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some_and(|n| *n != ']') {
                let hi = self.chars[self.pos + 1];
                self.pos += 2;
                set.push((c.min(hi), c.max(hi)));
            } else {
                set.push((c, c));
            }
        }
        Some(Node::Class { set, negated })
    }
}

fn is_delim(c: char) -> bool {
    matches!(c, ' ' | ',' | '{' | '}' | '(' | ')' | '[' | ']' | '\t')
}

/// Match `node` at `pos`, calling `k` with every end position until it accepts.
fn run(node: &Node, text: &[char], pos: usize, budget: &Cell<u32>, k: &mut dyn FnMut(usize) -> bool) -> bool {
    let left = budget.get();
    if left == 0 {
        return false;
    }
    budget.set(left - 1);
    match node {
        Node::Char(c) => text.get(pos) == Some(c) && k(pos + 1),
        Node::Any => pos < text.len() && k(pos + 1),
        Node::Class { set, negated } => match text.get(pos) {
            Some(c) => (set.iter().any(|(lo, hi)| lo <= c && c <= hi) != *negated) && k(pos + 1),
            None => false,
        },
        Node::Start => pos == 0 && k(pos),
        Node::End => pos == text.len() && k(pos),
        Node::Delim => {
            (pos == 0 || pos == text.len()) && k(pos) || text.get(pos).is_some_and(|c| is_delim(*c)) && k(pos + 1)
        }
        Node::Cat(items) => run_seq(items, text, pos, budget, k),
        Node::Alt(branches) => branches.iter().any(|b| run(b, text, pos, budget, k)),
        Node::Repeat { inner, min, many } => run_repeat(inner, *min, *many, text, pos, budget, k),
    }
}

fn run_seq(items: &[Node], text: &[char], pos: usize, budget: &Cell<u32>, k: &mut dyn FnMut(usize) -> bool) -> bool {
    match items.split_first() {
        None => k(pos),
        Some((first, rest)) => run(first, text, pos, budget, &mut |next| run_seq(rest, text, next, budget, k)),
    }
}

fn run_repeat(
    inner: &Node,
    min: u32,
    many: bool,
    text: &[char],
    pos: usize,
    budget: &Cell<u32>,
    k: &mut dyn FnMut(usize) -> bool,
) -> bool {
    // Greedy: try one more repetition first, then fall back to stopping here.
    let more = run(inner, text, pos, budget, &mut |next| {
        // An empty repetition would loop forever.
        if next == pos {
            return false;
        }
        if many { run_repeat(inner, min.saturating_sub(1), true, text, next, budget, k) } else { k(next) }
    });
    more || (min == 0 && k(pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pattern: &str, text: &str) -> bool {
        Regex::new(pattern).unwrap().is_match(text)
    }

    #[test]
    fn basics() {
        assert!(m("Gig", "interface GigabitEthernet0/1"));
        assert!(!m("gig", "interface GigabitEthernet0/1"));
        assert!(m("^interface", "interface Vlan1"));
        assert!(!m("^Vlan", "interface Vlan1"));
        assert!(m("up$", "line protocol is up"));
        assert!(m("a.c", "xabcx") && !m("a.c", "ac"));
        assert!(m("ab*c", "ac") && m("ab*c", "abbbc") && !m("ab+c", "ac"));
        assert!(m("colou?r", "color") && m("colou?r", "colour"));
        assert!(m("Fa0/[1-3]$", "Fa0/2") && !m("Fa0/[1-3]$", "Fa0/4"));
        assert!(m("[^0-9]$", "abc") && !m("[^0-9]$", "ab1"));
        assert!(m("up|down", "administratively down") && m("(Fa|Gi)0/1", "Gi0/1"));
        assert!(m("_10_", "vlan 10 name") && m("_10_", "10") && !m("_10_", "vlan 100"));
        assert!(m(r"10\.0\.0\.1", "10.0.0.1") && !m(r"10\.0\.0\.1", "10x0y0z1"));
        assert!(m("", "anything"));
        assert!(m("*", "a*b"));
    }

    #[test]
    fn malformed_and_pathological() {
        assert!(Regex::new("(abc").is_none());
        assert!(Regex::new("[abc").is_none());
        assert!(Regex::new("abc)").is_none());
        assert!(Regex::new(&"a".repeat(1000)).is_none());
        // Catastrophic backtracking is cut off by the step budget rather than hanging.
        let evil = Regex::new("(a*)*(a*)*(a*)*b").unwrap();
        assert!(!evil.is_match(&"a".repeat(200)));
        assert!(m("(a*)*", "b"));
    }
}
