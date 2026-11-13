//! Renders a Python regex syntax tree in the syntax understood by both the
//! `regex` and the `fancy-regex` crate.
//!
//! What is translated rather than copied:
//!
//! * `\s`, `\w` and friends: Python `str` patterns are Unicode aware. `\d` is
//!   `Nd` in both worlds, but Python's `\s` also matches the ASCII separators
//!   `\x1c`–`\x1f`, and its `\w` is exactly "letters, numbers and underscore"
//!   (`[\p{L}\p{N}_]`), which is not what the Rust engines call `\w`.
//! * `\Z` is the end of the text (`\z` in Rust; `fancy-regex` gives `\Z` another meaning).
//! * Literals are re-escaped from scratch. Python accepts `\<`, `\>`, `\-`, `\ `
//!   and so on as plain characters, whereas `\<` and `\>` are word-boundary
//!   assertions in Rust; an unescaped `[`, `&&`, `--` or `~~` inside a class is
//!   plain text in Python but an operator in Rust.
//! * Capture groups are emitted without names and referenced by number
//!   (`(?P=name)`, `(?(name)...)`); [`GroupSlot`] maps Python's group numbers
//!   to the emitted ones.
//! * A quantified look-around such as `(?<=x)+` (legal in Python, rejected by
//!   the Rust engines) is reduced to a single, possibly optional, evaluation.
//! * The final empty iteration of a loop, see [`super::loops`], which also
//!   describes the one known difference from Python that remains.
//!
//! `$` is emitted as-is: outside multi-line mode it matches only at the very end
//! of the text in Rust, while Python also accepts the position before a final
//! `\n`. The two agree on text without line breaks, which is all this crate
//! ever matches against (input lines and commands).
//!
//! A pattern can be rendered in two flavours, see [`Charset`].

use super::ast::{
    Assertion, Ast, Category, Class, ClassItem, Flags, GroupKind, LookKind, Node, RepeatMode,
};
use super::loops::final_empty_iteration_groups;

/// The flavour of a translated pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Charset {
    /// For any text. The classes are Unicode aware, as in Python.
    Unicode,
    /// For ASCII text only, to be compiled without Unicode support. On ASCII
    /// text it matches exactly what the Unicode flavour matches, provided
    /// [`has_ascii_flavour`] holds; the compiled pattern is far smaller and
    /// faster because `\w`, `\d`, `.` and negated classes shrink from tables
    /// of Unicode ranges to a few bytes.
    Ascii,
}

/// The members of the three character categories, to be put inside brackets.
struct Categories {
    digit: &'static str,
    space: &'static str,
    word: &'static str,
}

impl Charset {
    fn categories(self) -> Categories {
        match self {
            Charset::Unicode => Categories {
                digit: r"\d",
                // Python's `\s`: Unicode white space plus the separators FS, GS, RS and US.
                space: r"\s\x1c-\x1f",
                // Python's `\w`: every letter and number, and the underscore.
                word: r"\p{L}\p{N}_",
            },
            // The ASCII characters of the above.
            Charset::Ascii => Categories {
                digit: "0-9",
                space: r"\t\n\x0b\x0c\r \x1c-\x1f",
                word: "A-Za-z0-9_",
            },
        }
    }
}

/// Characters that need a backslash outside a character class.
const SPECIAL: &str = r"\.+*?()|[]{}^$";
/// Characters that need a backslash inside a character class.
const SPECIAL_IN_CLASS: &str = r"\[]^-&~";

pub(crate) struct Translation {
    /// The pattern in Rust syntax, anchored at the start of the text like `re.match`.
    pub pattern: String,
    /// Whether the pattern needs a backtracking engine (look-around,
    /// back-references, atomic groups, possessive quantifiers, conditionals).
    pub needs_backtracking: bool,
    /// Where to find each Python capture group (index = group number - 1).
    /// The same for both flavours.
    pub groups: Vec<GroupSlot>,
}

/// Location of one Python capture group in the emitted pattern.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GroupSlot {
    /// The group itself.
    pub primary: usize,
    /// An empty group emitted right after the loop this group sits in. When it
    /// took part in the match, Python's final empty iteration has reset the
    /// group to the empty string.
    pub empty_iteration: Option<usize>,
}

pub(crate) fn translate(ast: &Ast, charset: Charset) -> Translation {
    let mut groups = vec![GroupSlot::default(); ast.group_count()];
    number_groups(&ast.root, &mut groups, &mut 0);
    let mut emitter = Emitter {
        out: String::new(),
        needs_backtracking: false,
        groups: &groups,
        charset,
        categories: charset.categories(),
    };
    emitter.out.push_str(r"\A");
    emitter.open_flag_group(ast.flags, Flags::default());
    emitter.alternation(&ast.root);
    emitter.out.push(')');
    let Emitter {
        out,
        needs_backtracking,
        ..
    } = emitter;
    Translation {
        pattern: out,
        needs_backtracking,
        groups,
    }
}

/// Whether the [`Charset::Ascii`] flavour of the pattern is equivalent to the
/// Unicode flavour on ASCII text.
///
/// It is, unless the pattern ignores case and mentions a character outside
/// ASCII: a few of those, such as the Kelvin sign, are case variants of ASCII
/// letters, which only Unicode-aware matching knows about.
pub(crate) fn has_ascii_flavour(ast: &Ast) -> bool {
    fn ignores_case(node: &Node) -> bool {
        match node {
            Node::Group { kind, node } => {
                matches!(kind, GroupKind::Flags { on, .. } if on.ignore_case) || ignores_case(node)
            }
            Node::Concat(nodes) | Node::Alternation(nodes) => nodes.iter().any(ignores_case),
            Node::Repeat { node, .. } | Node::Look { node, .. } => ignores_case(node),
            Node::Conditional { yes, no, .. } => ignores_case(yes) || ignores_case(no),
            _ => false,
        }
    }
    fn mentions_non_ascii(node: &Node) -> bool {
        match node {
            Node::Literal(c) => !c.is_ascii(),
            Node::Class(class) => class.items.iter().any(|item| match item {
                ClassItem::Char(c) | ClassItem::Range(_, c) => !c.is_ascii(),
                ClassItem::Category(_) => false,
            }),
            Node::Concat(nodes) | Node::Alternation(nodes) => nodes.iter().any(mentions_non_ascii),
            Node::Group { node, .. } | Node::Repeat { node, .. } | Node::Look { node, .. } => {
                mentions_non_ascii(node)
            }
            Node::Conditional { yes, no, .. } => mentions_non_ascii(yes) || mentions_non_ascii(no),
            _ => false,
        }
    }
    !((ast.flags.ignore_case || ignores_case(&ast.root)) && mentions_non_ascii(&ast.root))
}

/// Assigns the number each group gets in the emitted pattern, walking the tree
/// in the order [`Emitter`] writes it.
fn number_groups(node: &Node, groups: &mut [GroupSlot], emitted: &mut usize) {
    match node {
        Node::Concat(nodes) | Node::Alternation(nodes) => {
            nodes
                .iter()
                .for_each(|node| number_groups(node, groups, emitted));
        }
        Node::Group { kind, node } => {
            if let GroupKind::Capture(number) = kind {
                *emitted += 1;
                groups[*number as usize - 1].primary = *emitted;
            }
            number_groups(node, groups, emitted);
        }
        Node::Repeat { node: body, .. } => {
            number_groups(body, groups, emitted);
            if let Some(reset) = final_empty_iteration_groups(node) {
                *emitted += 1;
                for number in reset {
                    groups[number as usize - 1].empty_iteration = Some(*emitted);
                }
            }
        }
        Node::Look { node, .. } => number_groups(node, groups, emitted),
        Node::Conditional { yes, no, .. } => {
            number_groups(yes, groups, emitted);
            number_groups(no, groups, emitted);
        }
        Node::Empty
        | Node::Literal(_)
        | Node::Dot
        | Node::Class(_)
        | Node::Assertion(_)
        | Node::Backref(_) => {}
    }
}

/// Whether `node` is a look-around, possibly inside non-capturing groups.
fn is_look_around(node: &Node) -> bool {
    match node {
        Node::Look { .. } => true,
        Node::Group {
            kind: GroupKind::NonCapture | GroupKind::Flags { .. },
            node,
        } => is_look_around(node),
        _ => false,
    }
}

struct Emitter<'a> {
    out: String,
    needs_backtracking: bool,
    groups: &'a [GroupSlot],
    charset: Charset,
    categories: Categories,
}

impl Emitter<'_> {
    /// Emits `node` in a position where a top-level `|` is allowed.
    fn alternation(&mut self, node: &Node) {
        match node {
            Node::Alternation(branches) => {
                for (index, branch) in branches.iter().enumerate() {
                    if index > 0 {
                        self.out.push('|');
                    }
                    self.sequence(branch);
                }
            }
            other => self.sequence(other),
        }
    }

    /// Emits `node` as (part of) a concatenation.
    fn sequence(&mut self, node: &Node) {
        match node {
            Node::Concat(nodes) => nodes.iter().for_each(|node| self.item(node)),
            other => self.item(other),
        }
    }

    fn item(&mut self, node: &Node) {
        match node {
            Node::Empty => {}
            Node::Literal(c) => self.literal(*c, SPECIAL),
            Node::Dot => self.out.push('.'),
            Node::Class(class) => self.class(class),
            Node::Assertion(assertion) => self.out.push_str(match assertion {
                Assertion::LineStart => "^",
                Assertion::LineEnd => "$",
                Assertion::TextStart => r"\A",
                Assertion::TextEnd => r"\z",
                Assertion::WordBoundary => r"\b",
                Assertion::NotWordBoundary => r"\B",
            }),
            Node::Concat(_) | Node::Alternation(_) => self.non_capturing(node),
            Node::Repeat {
                node: body,
                min,
                max,
                mode,
            } => {
                self.repeat(body, *min, *max, *mode);
                if final_empty_iteration_groups(node).is_some() {
                    self.out.push_str("()");
                }
            }
            Node::Group { kind, node } => {
                match kind {
                    GroupKind::Capture(_) => self.out.push('('),
                    GroupKind::NonCapture => self.out.push_str("(?:"),
                    GroupKind::Flags { on, off } => self.open_flag_group(*on, *off),
                    GroupKind::Atomic => {
                        self.needs_backtracking = true;
                        self.out.push_str("(?>");
                    }
                }
                self.alternation(node);
                self.out.push(')');
            }
            Node::Look { kind, node } => {
                self.needs_backtracking = true;
                self.out.push_str(match kind {
                    LookKind::Ahead => "(?=",
                    LookKind::NegativeAhead => "(?!",
                    LookKind::Behind => "(?<=",
                    LookKind::NegativeBehind => "(?<!",
                });
                self.alternation(node);
                self.out.push(')');
            }
            Node::Backref(group) => {
                self.needs_backtracking = true;
                // The group keeps a following digit from being read as part of the number.
                self.out
                    .push_str(&format!(r"(?:\{})", self.emitted_number(*group)));
            }
            Node::Conditional { group, yes, no } => {
                self.needs_backtracking = true;
                self.out
                    .push_str(&format!("(?({})", self.emitted_number(*group)));
                self.conditional_branch(yes);
                if **no != Node::Empty {
                    self.out.push('|');
                    self.conditional_branch(no);
                }
                self.out.push(')');
            }
        }
    }

    fn conditional_branch(&mut self, node: &Node) {
        match node {
            Node::Alternation(_) => self.non_capturing(node),
            other => self.sequence(other),
        }
    }

    fn emitted_number(&self, group: u32) -> usize {
        self.groups[group as usize - 1].primary
    }

    fn non_capturing(&mut self, node: &Node) {
        self.out.push_str("(?:");
        self.alternation(node);
        self.out.push(')');
    }

    /// Opens `(?flags-flags:`. Only `i`, `m` and `s` exist on the Rust side;
    /// verbose mode is resolved by the parser.
    fn open_flag_group(&mut self, on: Flags, off: Flags) {
        let letters = |flags: Flags| {
            [
                (flags.ignore_case, 'i'),
                (flags.multi_line, 'm'),
                (flags.dot_all, 's'),
            ]
            .into_iter()
            .filter_map(|(set, letter)| set.then_some(letter))
            .collect::<String>()
        };
        let (on, off) = (letters(on), letters(off));
        self.out.push_str("(?");
        self.out.push_str(&on);
        if !off.is_empty() {
            self.out.push('-');
            self.out.push_str(&off);
        }
        self.out.push(':');
    }

    fn repeat(&mut self, node: &Node, min: u32, max: Option<u32>, mode: RepeatMode) {
        if is_look_around(node) {
            // Repeating a zero-width assertion evaluates it once; with a minimum
            // of zero it may also be skipped.
            match (min, mode) {
                (0, RepeatMode::Lazy) => {
                    self.out.push_str("(?:|");
                    self.item(node);
                    self.out.push(')');
                }
                (0, _) => {
                    self.out.push_str("(?:");
                    self.item(node);
                    self.out.push_str("|)");
                }
                _ => self.item(node),
            }
            return;
        }
        if mode == RepeatMode::Possessive {
            // `a*+` is `(?>a*)`.
            self.needs_backtracking = true;
            self.out.push_str("(?>");
        }
        match node {
            Node::Literal(_) | Node::Dot | Node::Class(_) | Node::Group { .. } => self.item(node),
            other => self.non_capturing(other),
        }
        let quantifier = match (min, max) {
            (0, None) => "*".to_owned(),
            (1, None) => "+".to_owned(),
            (0, Some(1)) => "?".to_owned(),
            (min, None) => format!("{{{min},}}"),
            (min, Some(max)) if min == max => format!("{{{min}}}"),
            (min, Some(max)) => format!("{{{min},{max}}}"),
        };
        self.out.push_str(&quantifier);
        match mode {
            RepeatMode::Greedy => {}
            RepeatMode::Lazy => self.out.push('?'),
            RepeatMode::Possessive => self.out.push(')'),
        }
    }

    fn class(&mut self, class: &Class) {
        if let (false, [ClassItem::Category(category)]) = (class.negated, class.items.as_slice()) {
            // A bare `\d`, `\S`, ... outside brackets.
            self.category(*category);
            return;
        }
        let start = self.out.len();
        self.out.push_str(if class.negated { "[^" } else { "[" });
        let members = self.out.len();
        for item in &class.items {
            match *item {
                ClassItem::Char(c) => self.class_range(c, c),
                ClassItem::Range(low, high) => self.class_range(low, high),
                ClassItem::Category(Category::Digit) => self.out.push_str(self.categories.digit),
                ClassItem::Category(Category::Space) => self.out.push_str(self.categories.space),
                ClassItem::Category(Category::Word) => self.out.push_str(self.categories.word),
                // A nested class.
                ClassItem::Category(negated) => self.category(negated),
            }
        }
        if self.out.len() == members {
            // The ASCII flavour of a class without ASCII members.
            self.out.truncate(start);
            self.out.push_str(if class.negated {
                r"[\x00-\x7f]"
            } else {
                r"[^\x00-\x7f]"
            });
        } else {
            self.out.push(']');
        }
    }

    /// A category as a class of its own.
    fn category(&mut self, category: Category) {
        let (negated, members) = match category {
            Category::Digit => (false, self.categories.digit),
            Category::NotDigit => (true, self.categories.digit),
            Category::Space => (false, self.categories.space),
            Category::NotSpace => (true, self.categories.space),
            Category::Word => (false, self.categories.word),
            Category::NotWord => (true, self.categories.word),
        };
        self.out.push_str(if negated { "[^" } else { "[" });
        self.out.push_str(members);
        self.out.push(']');
    }

    /// A class member. The ASCII flavour keeps the ASCII part of it.
    fn class_range(&mut self, low: char, high: char) {
        let high = match self.charset {
            Charset::Unicode => high,
            Charset::Ascii if low.is_ascii() => high.min('\x7f'),
            Charset::Ascii => return,
        };
        self.literal(low, SPECIAL_IN_CLASS);
        if high != low {
            self.out.push('-');
            self.literal(high, SPECIAL_IN_CLASS);
        }
    }

    fn literal(&mut self, c: char, special: &str) {
        if special.contains(c) {
            self.out.push('\\');
            self.out.push(c);
        } else if c.is_ascii_control() {
            self.out.push_str(&format!(r"\x{:02x}", u32::from(c)));
        } else {
            self.out.push(c);
        }
    }
}
