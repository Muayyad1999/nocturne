//! Syntax tree for Python `re` patterns.

/// A parsed Python regular expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ast {
    pub root: Node,
    /// Flags set by a leading `(?imsx)` group; they apply to the whole pattern.
    pub flags: Flags,
    /// Name of every capture group, indexed by group number minus one.
    pub group_names: Vec<Option<String>>,
}

impl Ast {
    /// Number of capture groups, not counting the implicit group 0.
    pub fn group_count(&self) -> usize {
        self.group_names.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Node {
    /// Matches the empty string.
    Empty,
    Literal(char),
    /// `.`
    Dot,
    Class(Class),
    Assertion(Assertion),
    Concat(Vec<Node>),
    Alternation(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: u32,
        /// `None` means unbounded.
        max: Option<u32>,
        mode: RepeatMode,
    },
    Group {
        kind: GroupKind,
        node: Box<Node>,
    },
    Look {
        kind: LookKind,
        node: Box<Node>,
    },
    /// `\1` or `(?P=name)`.
    Backref(u32),
    /// `(?(group)yes|no)`.
    Conditional {
        group: u32,
        yes: Box<Node>,
        no: Box<Node>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RepeatMode {
    Greedy,
    /// `*?`, `+?`, `??`, `{m,n}?`
    Lazy,
    /// `*+`, `++`, `?+`, `{m,n}+`
    Possessive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GroupKind {
    /// `(...)` or `(?P<name>...)`; the number is the 1-based group index.
    Capture(u32),
    /// `(?:...)`
    NonCapture,
    /// `(?i-s:...)`
    Flags { on: Flags, off: Flags },
    /// `(?>...)`
    Atomic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LookKind {
    Ahead,
    NegativeAhead,
    Behind,
    NegativeBehind,
}

impl LookKind {
    pub fn is_behind(self) -> bool {
        matches!(self, LookKind::Behind | LookKind::NegativeBehind)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Assertion {
    /// `^`
    LineStart,
    /// `$`
    LineEnd,
    /// `\A`
    TextStart,
    /// `\Z`
    TextEnd,
    /// `\b`
    WordBoundary,
    /// `\B`
    NotWordBoundary,
}

/// A bracketed character class, or a bare category escape such as `\d`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Class {
    pub negated: bool,
    pub items: Vec<ClassItem>,
}

impl Class {
    pub fn category(category: Category) -> Class {
        Class { negated: false, items: vec![ClassItem::Category(category)] }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClassItem {
    Char(char),
    /// Inclusive range.
    Range(char, char),
    Category(Category),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Category {
    /// `\d`
    Digit,
    /// `\D`
    NotDigit,
    /// `\s`
    Space,
    /// `\S`
    NotSpace,
    /// `\w`
    Word,
    /// `\W`
    NotWord,
}

/// The inline flags that have an effect on `str` patterns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Flags {
    /// `i`
    pub ignore_case: bool,
    /// `m`
    pub multi_line: bool,
    /// `s`
    pub dot_all: bool,
    /// `x`
    pub verbose: bool,
}

impl Flags {
    pub fn is_empty(self) -> bool {
        self == Flags::default()
    }

    pub fn union(self, other: Flags) -> Flags {
        Flags {
            ignore_case: self.ignore_case || other.ignore_case,
            multi_line: self.multi_line || other.multi_line,
            dot_all: self.dot_all || other.dot_all,
            verbose: self.verbose || other.verbose,
        }
    }

    pub fn intersects(self, other: Flags) -> bool {
        (self.ignore_case && other.ignore_case)
            || (self.multi_line && other.multi_line)
            || (self.dot_all && other.dot_all)
            || (self.verbose && other.verbose)
    }
}
