//! Parser for the Python `re` syntax.
//!
//! This follows CPython's `re/_parser.py` (3.13) rule by rule, including the
//! places where Python is lenient (`{` that does not start a quantifier is a
//! literal, `]` right after `[` is a literal, `[` inside a class is a literal)
//! and where it is strict (`a**` is "multiple repeat", `[\w-x]` is a bad range).
//! Errors carry Python's message and point at the position Python reports.

use super::SyntaxError;
use super::ast::{
    Assertion, Ast, Category, Class, ClassItem, Flags, GroupKind, LookKind, Node, RepeatMode,
};

/// Python's `MAXREPEAT`.
const MAX_REPEAT: u64 = u32::MAX as u64;
/// Width used for "unbounded" when checking that look-behinds have a fixed width.
const MAX_WIDTH: u64 = u64::MAX;

pub(crate) fn parse(pattern: &str) -> Result<Ast, SyntaxError> {
    let mut parser = Parser::new(pattern)?;
    let root = parser.parse_alternation(false, 0)?;
    if parser.peek().is_some() {
        // Only an unmatched `)` stops the top-level alternation early.
        return Err(parser.error_here("unbalanced parenthesis"));
    }
    for &(group, position) in &parser.conditional_groups {
        if group as usize > parser.group_names.len() {
            return Err(SyntaxError::new(format!("invalid group reference {group}"), position));
        }
    }
    Ok(Ast { root, flags: parser.global_flags, group_names: parser.group_names })
}

/// One lexical token: a character, or a backslash together with the character after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    Char(char),
    Escape(char),
}

impl Token {
    /// The token as written in the pattern.
    fn text(self) -> String {
        match self {
            Token::Char(c) => c.to_string(),
            Token::Escape(c) => format!("\\{c}"),
        }
    }
}

struct Parser {
    tokens: Vec<Token>,
    /// Character offset of every token, plus the pattern length as a sentinel.
    offsets: Vec<usize>,
    next: usize,
    /// Name of each capture group opened so far (index = group number - 1).
    group_names: Vec<Option<String>>,
    /// (minimum, maximum) width of each capture group, `None` while it is still open.
    group_widths: Vec<Option<(u64, u64)>>,
    global_flags: Flags,
    /// Number of the first capture group that belongs to the look-behind being
    /// parsed; such groups cannot be referenced from inside that look-behind.
    lookbehind_first_group: Option<u32>,
    /// Numeric `(?(N)...)` references, checked once all groups are known.
    conditional_groups: Vec<(u32, usize)>,
}

impl Parser {
    fn new(pattern: &str) -> Result<Parser, SyntaxError> {
        let mut tokens = Vec::new();
        let mut offsets = Vec::new();
        let mut chars = pattern.chars().enumerate();
        let mut length = 0;
        while let Some((offset, c)) = chars.next() {
            length = offset + 1;
            offsets.push(offset);
            if c != '\\' {
                tokens.push(Token::Char(c));
                continue;
            }
            match chars.next() {
                Some((offset, escaped)) => {
                    length = offset + 1;
                    tokens.push(Token::Escape(escaped));
                }
                None => return Err(SyntaxError::new("bad escape (end of pattern)", offset)),
            }
        }
        offsets.push(length);
        Ok(Parser {
            tokens,
            offsets,
            next: 0,
            group_names: Vec::new(),
            group_widths: Vec::new(),
            global_flags: Flags::default(),
            lookbehind_first_group: None,
            conditional_groups: Vec::new(),
        })
    }

    // --- token stream -----------------------------------------------------

    fn peek(&self) -> Option<Token> {
        self.tokens.get(self.next).copied()
    }

    fn peek_char(&self) -> Option<char> {
        match self.peek() {
            Some(Token::Char(c)) => Some(c),
            _ => None,
        }
    }

    fn advance(&mut self) -> Option<Token> {
        let token = self.peek();
        if token.is_some() {
            self.next += 1;
        }
        token
    }

    /// Consumes the next token if it is the plain character `c`.
    fn eat(&mut self, c: char) -> bool {
        let found = self.peek() == Some(Token::Char(c));
        if found {
            self.next += 1;
        }
        found
    }

    /// Consumes up to `limit` plain characters accepted by `accept`.
    fn take_while(&mut self, limit: usize, accept: impl Fn(char) -> bool) -> String {
        let mut taken = String::new();
        while taken.len() < limit {
            match self.peek_char() {
                Some(c) if accept(c) => {
                    taken.push(c);
                    self.next += 1;
                }
                _ => break,
            }
        }
        taken
    }

    /// Consumes a name up to and including `terminator`. Returns the name and
    /// the offset at which it starts.
    fn take_name(&mut self, terminator: char) -> Result<(String, usize), SyntaxError> {
        let start = self.offset();
        let mut name = String::new();
        loop {
            match self.advance() {
                None if name.is_empty() => return Err(self.error_here("missing group name")),
                None => {
                    let message = format!("missing {terminator}, unterminated name");
                    return Err(SyntaxError::new(message, start));
                }
                Some(Token::Char(c)) if c == terminator => {
                    if name.is_empty() {
                        return Err(self.error_back("missing group name", 1));
                    }
                    return Ok((name, start));
                }
                Some(token) => name.push_str(&token.text()),
            }
        }
    }

    /// Character offset of the next token (the pattern length at the end).
    fn offset(&self) -> usize {
        self.offsets[self.next]
    }

    fn error_here(&self, message: impl Into<String>) -> SyntaxError {
        SyntaxError::new(message, self.offset())
    }

    /// An error `back` characters before the next token.
    fn error_back(&self, message: impl Into<String>, back: usize) -> SyntaxError {
        SyntaxError::new(message, self.offset().saturating_sub(back))
    }

    // --- grammar ----------------------------------------------------------

    /// `a|b|c` (CPython: `_parse_sub`).
    fn parse_alternation(&mut self, mut verbose: bool, nested: u32) -> Result<Node, SyntaxError> {
        let mut branches = Vec::new();
        loop {
            let at_pattern_start = nested == 0 && branches.is_empty();
            branches.push(self.parse_sequence(verbose, nested + 1, at_pattern_start)?);
            if !self.eat('|') {
                break;
            }
            if nested == 0 {
                verbose = self.global_flags.verbose;
            }
        }
        Ok(if branches.len() == 1 { branches.swap_remove(0) } else { Node::Alternation(branches) })
    }

    /// A sequence of quantified atoms (CPython: `_parse`).
    fn parse_sequence(
        &mut self,
        mut verbose: bool,
        nested: u32,
        at_pattern_start: bool,
    ) -> Result<Node, SyntaxError> {
        let mut items: Vec<Node> = Vec::new();
        while let Some(token) = self.peek() {
            if matches!(token, Token::Char('|' | ')')) {
                break;
            }
            let start = self.offset();
            self.next += 1;
            let c = match token {
                Token::Escape(c) => {
                    items.push(self.parse_escape(c, start)?);
                    continue;
                }
                Token::Char(c) => c,
            };
            if verbose {
                if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c') {
                    continue;
                }
                if c == '#' {
                    while !matches!(self.advance(), None | Some(Token::Char('\n'))) {}
                    continue;
                }
            }
            match c {
                '[' => items.push(self.parse_class(start)?),
                '*' | '+' | '?' | '{' => self.parse_quantifier(c, start, &mut items)?,
                '.' => items.push(Node::Dot),
                '^' => items.push(Node::Assertion(Assertion::LineStart)),
                '$' => items.push(Node::Assertion(Assertion::LineEnd)),
                '(' => {
                    let allow_global_flags = at_pattern_start && items.is_empty();
                    match self.parse_group(start, verbose, nested, allow_global_flags)? {
                        Group::Node(node) => items.push(node),
                        Group::Comment => {}
                        Group::GlobalFlags => verbose = self.global_flags.verbose,
                    }
                }
                _ => items.push(Node::Literal(c)),
            }
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.swap_remove(0),
            _ => Node::Concat(items),
        })
    }

    /// `*`, `+`, `?`, `{m,n}` and their lazy / possessive forms. The quantifier
    /// character, found at `start`, has already been consumed.
    fn parse_quantifier(
        &mut self,
        quantifier: char,
        start: usize,
        items: &mut Vec<Node>,
    ) -> Result<(), SyntaxError> {
        let (min, max) = match quantifier {
            '?' => (0, Some(1)),
            '*' => (0, None),
            '+' => (1, None),
            _ => match self.parse_counted_repeat(start)? {
                Some(bounds) => bounds,
                None => {
                    // Not a quantifier after all: Python treats the brace as a literal.
                    items.push(Node::Literal('{'));
                    return Ok(());
                }
            },
        };
        let node = match items.pop() {
            None | Some(Node::Assertion(_)) => {
                return Err(SyntaxError::new("nothing to repeat", start));
            }
            Some(Node::Repeat { .. }) => return Err(SyntaxError::new("multiple repeat", start)),
            Some(node) => Box::new(node),
        };
        let mode = if self.eat('?') {
            RepeatMode::Lazy
        } else if self.eat('+') {
            RepeatMode::Possessive
        } else {
            RepeatMode::Greedy
        };
        items.push(Node::Repeat { node, min, max, mode });
        Ok(())
    }

    /// Parses the inside of `{m,n}` after the opening brace at `start`. Returns
    /// `None`, with the position restored to just after the brace, when this is
    /// not a quantifier.
    fn parse_counted_repeat(
        &mut self,
        start: usize,
    ) -> Result<Option<(u32, Option<u32>)>, SyntaxError> {
        let after_brace = self.next;
        if self.peek_char() == Some('}') {
            return Ok(None);
        }
        let low = self.take_while(usize::MAX, |c| c.is_ascii_digit());
        let high = if self.eat(',') {
            self.take_while(usize::MAX, |c| c.is_ascii_digit())
        } else {
            low.clone()
        };
        if !self.eat('}') {
            self.next = after_brace;
            return Ok(None);
        }
        let bound = |digits: &str| -> Result<Option<u32>, SyntaxError> {
            if digits.is_empty() {
                return Ok(None);
            }
            match digits.parse::<u64>() {
                Ok(value) if value < MAX_REPEAT => Ok(Some(value as u32)),
                _ => Err(SyntaxError::new("the repetition number is too large", start + 1)),
            }
        };
        let min = bound(&low)?.unwrap_or(0);
        let max = bound(&high)?;
        if max.is_some_and(|max| max < min) {
            return Err(SyntaxError::new("min repeat greater than max repeat", start + 1));
        }
        Ok(Some((min, max)))
    }

    /// A bracketed class; the opening `[` at `start` has already been consumed.
    fn parse_class(&mut self, start: usize) -> Result<Node, SyntaxError> {
        let unterminated = || SyntaxError::new("unterminated character set", start);
        let negated = self.eat('^');
        let mut items = Vec::new();
        loop {
            let first_start = self.offset();
            let first_token = self.advance().ok_or_else(unterminated)?;
            // A `]` directly after `[` or `[^` is a literal.
            if first_token == Token::Char(']') && !items.is_empty() {
                break;
            }
            let first = self.parse_class_atom(first_token, first_start)?;
            if !self.eat('-') {
                items.push(first);
                continue;
            }
            let second_start = self.offset();
            let second_token = self.advance().ok_or_else(unterminated)?;
            if second_token == Token::Char(']') {
                // A trailing `-` is a literal.
                items.push(first);
                items.push(ClassItem::Char('-'));
                break;
            }
            let second = self.parse_class_atom(second_token, second_start)?;
            match (first, second) {
                (ClassItem::Char(low), ClassItem::Char(high)) if low <= high => {
                    items.push(ClassItem::Range(low, high));
                }
                _ => {
                    let range = format!("{}-{}", first_token.text(), second_token.text());
                    return Err(SyntaxError::new(
                        format!("bad character range {range}"),
                        first_start,
                    ));
                }
            }
        }
        Ok(Node::Class(Class { negated, items }))
    }

    fn parse_class_atom(&mut self, token: Token, start: usize) -> Result<ClassItem, SyntaxError> {
        let escaped = match token {
            Token::Char(c) => return Ok(ClassItem::Char(c)),
            Token::Escape(c) => c,
        };
        if let Some(category) = category_escape(escaped) {
            return Ok(ClassItem::Category(category));
        }
        let literal = match escaped {
            // Inside a class `\b` is a backspace.
            'b' => '\x08',
            '0'..='7' => {
                // Up to three octal digits.
                let mut digits = String::from(escaped);
                digits.push_str(&self.take_while(2, |c| c.is_digit(8)));
                octal_literal(&digits, start)?
            }
            _ => self.parse_literal_escape(escaped, start)?,
        };
        Ok(ClassItem::Char(literal))
    }

    /// An escape outside a class (CPython: `_escape`).
    fn parse_escape(&mut self, escaped: char, start: usize) -> Result<Node, SyntaxError> {
        if let Some(category) = category_escape(escaped) {
            return Ok(Node::Class(Class::category(category)));
        }
        let assertion = match escaped {
            'A' => Some(Assertion::TextStart),
            'Z' => Some(Assertion::TextEnd),
            'b' => Some(Assertion::WordBoundary),
            'B' => Some(Assertion::NotWordBoundary),
            _ => None,
        };
        if let Some(assertion) = assertion {
            return Ok(Node::Assertion(assertion));
        }
        match escaped {
            '0' => {
                let mut digits = String::from(escaped);
                digits.push_str(&self.take_while(2, |c| c.is_digit(8)));
                Ok(Node::Literal(octal_literal(&digits, start)?))
            }
            '1'..='9' => self.parse_numeric_escape(escaped, start),
            _ => Ok(Node::Literal(self.parse_literal_escape(escaped, start)?)),
        }
    }

    /// `\N`, `\NN` (group references) or `\NNN` (an octal character).
    fn parse_numeric_escape(&mut self, first: char, start: usize) -> Result<Node, SyntaxError> {
        let mut digits = String::from(first);
        if let Some(second) = self.peek_char().filter(char::is_ascii_digit) {
            self.next += 1;
            digits.push(second);
            let third = self.peek_char().filter(|c| c.is_digit(8));
            if let (true, true, Some(third)) = (first.is_digit(8), second.is_digit(8), third) {
                self.next += 1;
                digits.push(third);
                return Ok(Node::Literal(octal_literal(&digits, start)?));
            }
        }
        // Python points behind the backslash here.
        let invalid = || SyntaxError::new(format!("invalid group reference {digits}"), start + 1);
        let group: u32 = digits.parse().map_err(|_| invalid())?;
        if group as usize > self.group_names.len() {
            return Err(invalid());
        }
        self.check_group_reference(group, start)?;
        Ok(Node::Backref(group))
    }

    /// Escapes that denote a single character and are valid both inside and outside classes.
    fn parse_literal_escape(&mut self, escaped: char, start: usize) -> Result<char, SyntaxError> {
        let hex = |parser: &mut Parser, digits: usize| -> Result<char, SyntaxError> {
            let value = parser.take_while(digits, |c| c.is_ascii_hexdigit());
            if value.len() != digits {
                return Err(SyntaxError::new(format!("incomplete escape \\{escaped}{value}"), start));
            }
            u32::from_str_radix(&value, 16)
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(|| SyntaxError::new(format!("bad escape \\{escaped}{value}"), start))
        };
        match escaped {
            'a' => Ok('\x07'),
            'f' => Ok('\x0c'),
            'n' => Ok('\n'),
            'r' => Ok('\r'),
            't' => Ok('\t'),
            'v' => Ok('\x0b'),
            'x' => hex(self, 2),
            'u' => hex(self, 4),
            'U' => hex(self, 8),
            'N' => {
                Err(SyntaxError::new("named Unicode escapes (\\N{...}) are not supported", start))
            }
            c if c.is_ascii_alphanumeric() => {
                Err(SyntaxError::new(format!("bad escape \\{c}"), start))
            }
            c => Ok(c),
        }
    }

    /// Everything that starts with `(`; the parenthesis at `start` has already
    /// been consumed.
    fn parse_group(
        &mut self,
        start: usize,
        verbose: bool,
        nested: u32,
        allow_global_flags: bool,
    ) -> Result<Group, SyntaxError> {
        let unknown_extension =
            |text: String| SyntaxError::new(format!("unknown extension ?{text}"), start + 1);
        let mut kind = GroupKind::NonCapture;
        // The name of a capture group and where it is written.
        let mut capture: Option<(Option<String>, usize)> = Some((None, start));
        let mut body_verbose = verbose;
        if self.eat('?') {
            capture = None;
            let token = self.advance().ok_or_else(|| self.error_here("unexpected end of pattern"))?;
            let Token::Char(c) = token else {
                return Err(unknown_extension(token.text()));
            };
            match c {
                'P' => {
                    if self.eat('<') {
                        let (name, name_start) = self.take_name('>')?;
                        check_group_name(&name, name_start)?;
                        capture = Some((Some(name), name_start));
                    } else if self.eat('=') {
                        let (name, name_start) = self.take_name(')')?;
                        check_group_name(&name, name_start)?;
                        let group = self.group_number(&name, name_start)?;
                        self.check_group_reference(group, name_start)?;
                        return Ok(Group::Node(Node::Backref(group)));
                    } else {
                        let token = self
                            .advance()
                            .ok_or_else(|| self.error_here("unexpected end of pattern"))?;
                        return Err(unknown_extension(format!("P{}", token.text())));
                    }
                }
                ':' => {}
                '#' => {
                    loop {
                        match self.advance() {
                            None => {
                                return Err(SyntaxError::new(
                                    "missing ), unterminated comment",
                                    start,
                                ));
                            }
                            Some(Token::Char(')')) => break,
                            Some(_) => {}
                        }
                    }
                    return Ok(Group::Comment);
                }
                '=' => return self.parse_look(LookKind::Ahead, start, verbose, nested),
                '!' => return self.parse_look(LookKind::NegativeAhead, start, verbose, nested),
                '<' => {
                    let kind = match self.advance() {
                        None => return Err(self.error_here("unexpected end of pattern")),
                        Some(Token::Char('=')) => LookKind::Behind,
                        Some(Token::Char('!')) => LookKind::NegativeBehind,
                        Some(token) => return Err(unknown_extension(format!("<{}", token.text()))),
                    };
                    return self.parse_look(kind, start, verbose, nested);
                }
                '(' => return self.parse_conditional(start, verbose, nested),
                '>' => kind = GroupKind::Atomic,
                'i' | 'L' | 'm' | 's' | 'x' | 'a' | 'u' | '-' => {
                    let Some((on, off)) = self.parse_flags(c)? else {
                        if !allow_global_flags {
                            return Err(SyntaxError::new(
                                "global flags not at the start of the expression",
                                start,
                            ));
                        }
                        return Ok(Group::GlobalFlags);
                    };
                    body_verbose = (verbose || on.verbose) && !off.verbose;
                    if !(on.is_empty() && off.is_empty()) {
                        kind = GroupKind::Flags { on, off };
                    }
                }
                c => return Err(unknown_extension(c.to_string())),
            }
        }

        let group = match capture {
            Some((name, name_start)) => {
                let number = self.group_names.len() as u32 + 1;
                if let Some(name) = &name
                    && let Ok(existing) = self.group_number(name, name_start)
                {
                    return Err(SyntaxError::new(
                        format!(
                            "redefinition of group name '{name}' as group {number}; was group {existing}"
                        ),
                        name_start,
                    ));
                }
                self.group_names.push(name);
                self.group_widths.push(None);
                kind = GroupKind::Capture(number);
                Some(number)
            }
            None => None,
        };
        let body = self.parse_alternation(body_verbose, nested + 1)?;
        self.expect_close(start)?;
        if let Some(number) = group {
            self.group_widths[number as usize - 1] = Some(width(&body, &self.group_widths));
        }
        Ok(Group::Node(Node::Group { kind, node: Box::new(body) }))
    }

    /// The body of a look-around; the introducer has already been consumed.
    fn parse_look(
        &mut self,
        kind: LookKind,
        start: usize,
        verbose: bool,
        nested: u32,
    ) -> Result<Group, SyntaxError> {
        let outermost_lookbehind = kind.is_behind() && self.lookbehind_first_group.is_none();
        if outermost_lookbehind {
            self.lookbehind_first_group = Some(self.group_names.len() as u32 + 1);
        }
        let body = self.parse_alternation(verbose, nested + 1)?;
        if outermost_lookbehind {
            self.lookbehind_first_group = None;
        }
        self.expect_close(start)?;
        let (shortest, longest) = width(&body, &self.group_widths);
        if kind.is_behind() && shortest != longest {
            return Err(SyntaxError::new("look-behind requires fixed-width pattern", start));
        }
        Ok(Group::Node(Node::Look { kind, node: Box::new(body) }))
    }

    /// `(?(group)yes|no)`; everything up to and including `(?(` has been consumed.
    fn parse_conditional(
        &mut self,
        start: usize,
        verbose: bool,
        nested: u32,
    ) -> Result<Group, SyntaxError> {
        let (name, name_start) = self.take_name(')')?;
        let group = if name.bytes().all(|b| b.is_ascii_digit()) {
            let group: u32 = name.parse().map_err(|_| {
                SyntaxError::new(format!("invalid group reference {name}"), name_start)
            })?;
            if group == 0 {
                return Err(SyntaxError::new("bad group number", name_start));
            }
            self.conditional_groups.push((group, name_start));
            group
        } else {
            check_group_name(&name, name_start)?;
            self.group_number(&name, name_start)?
        };
        if self.lookbehind_first_group.is_some() {
            self.check_group_reference(group, self.offset())?;
        }
        let yes = self.parse_sequence(verbose, nested + 1, false)?;
        let no = if self.eat('|') {
            let no = self.parse_sequence(verbose, nested + 1, false)?;
            if self.peek_char() == Some('|') {
                return Err(self.error_here("conditional backref with more than two branches"));
            }
            no
        } else {
            Node::Empty
        };
        self.expect_close(start)?;
        Ok(Group::Node(Node::Conditional { group, yes: Box::new(yes), no: Box::new(no) }))
    }

    /// Inline flags after `(?`. `first` is the first flag character (or `-`),
    /// already consumed. Returns `None` for a global flag group `(?i)` and the
    /// flags to switch on and off for a scoped group `(?i-s:`.
    fn parse_flags(&mut self, first: char) -> Result<Option<(Flags, Flags)>, SyntaxError> {
        let mut on = Flags::default();
        let mut off = Flags::default();
        let mut c = first;
        if c != '-' {
            loop {
                set_flag(&mut on, c).map_err(|message| self.error_here(message))?;
                c = self.next_flag_char("missing -, : or )")?;
                if matches!(c, ')' | '-' | ':') {
                    break;
                }
                if !is_flag(c) {
                    let message =
                        if c.is_alphabetic() { "unknown flag" } else { "missing -, : or )" };
                    return Err(self.error_back(message, 1));
                }
            }
        }
        if c == ')' {
            self.global_flags = self.global_flags.union(on);
            return Ok(None);
        }
        if c == '-' {
            c = self.next_flag_char("missing flag")?;
            if !is_flag(c) {
                let message = if c.is_alphabetic() { "unknown flag" } else { "missing flag" };
                return Err(self.error_back(message, 1));
            }
            loop {
                if matches!(c, 'a' | 'u' | 'L') {
                    return Err(self.error_here(
                        "bad inline flags: cannot turn off flags 'a', 'u' and 'L'",
                    ));
                }
                set_flag(&mut off, c).map_err(|message| self.error_here(message))?;
                c = self.next_flag_char("missing :")?;
                if c == ':' {
                    break;
                }
                if !is_flag(c) {
                    let message = if c.is_alphabetic() { "unknown flag" } else { "missing :" };
                    return Err(self.error_back(message, 1));
                }
            }
        }
        if on.intersects(off) {
            return Err(self.error_back("bad inline flags: flag turned on and off", 1));
        }
        Ok(Some((on, off)))
    }

    /// The next character of a flag group; `missing` is the error at the end
    /// of the pattern.
    fn next_flag_char(&mut self, missing: &str) -> Result<char, SyntaxError> {
        match self.advance() {
            Some(Token::Char(c)) => Ok(c),
            Some(Token::Escape(_)) => Err(self.error_back(missing, 2)),
            None => Err(self.error_here(missing)),
        }
    }

    fn expect_close(&mut self, group_start: usize) -> Result<(), SyntaxError> {
        if self.eat(')') {
            Ok(())
        } else {
            Err(SyntaxError::new("missing ), unterminated subpattern", group_start))
        }
    }

    fn group_number(&self, name: &str, position: usize) -> Result<u32, SyntaxError> {
        self.group_names
            .iter()
            .position(|existing| existing.as_deref() == Some(name))
            .map(|index| index as u32 + 1)
            .ok_or_else(|| SyntaxError::new(format!("unknown group name '{name}'"), position))
    }

    /// Back-references may only point at groups that are already closed and,
    /// inside a look-behind, at groups defined before that look-behind.
    /// `position` is where the reference is written.
    fn check_group_reference(&self, group: u32, position: usize) -> Result<(), SyntaxError> {
        let closed = (group as usize)
            .checked_sub(1)
            .and_then(|index| self.group_widths.get(index))
            .is_some_and(Option::is_some);
        if !closed {
            return Err(SyntaxError::new("cannot refer to an open group", position));
        }
        if self.lookbehind_first_group.is_some_and(|first| group >= first) {
            return Err(self.error_here(
                "cannot refer to group defined in the same lookbehind subpattern",
            ));
        }
        Ok(())
    }
}

/// What a parenthesised construct turned out to be.
enum Group {
    Node(Node),
    /// `(?#...)`: produces nothing, so a following quantifier applies to the item before it.
    Comment,
    /// `(?i)`: recorded in `Parser::global_flags`.
    GlobalFlags,
}

fn category_escape(escaped: char) -> Option<Category> {
    Some(match escaped {
        'd' => Category::Digit,
        'D' => Category::NotDigit,
        's' => Category::Space,
        'S' => Category::NotSpace,
        'w' => Category::Word,
        'W' => Category::NotWord,
        _ => return None,
    })
}

fn octal_literal(digits: &str, position: usize) -> Result<char, SyntaxError> {
    let out_of_range = || {
        SyntaxError::new(
            format!("octal escape value \\{digits} outside of range 0-0o377"),
            position,
        )
    };
    let value = u32::from_str_radix(digits, 8).map_err(|_| out_of_range())?;
    if value > 0o377 {
        return Err(out_of_range());
    }
    char::from_u32(value).ok_or_else(out_of_range)
}

fn is_flag(c: char) -> bool {
    matches!(c, 'i' | 'L' | 'm' | 's' | 'x' | 'a' | 'u')
}

fn set_flag(flags: &mut Flags, c: char) -> Result<(), &'static str> {
    match c {
        'i' => flags.ignore_case = true,
        'm' => flags.multi_line = true,
        's' => flags.dot_all = true,
        'x' => flags.verbose = true,
        // Unicode matching is the default for `str` patterns.
        'u' => {}
        'a' => return Err("the ASCII flag (?a) is not supported"),
        _ => return Err("bad inline flags: cannot use 'L' flag with a str pattern"),
    }
    Ok(())
}

/// Python requires group names to be identifiers.
fn check_group_name(name: &str, position: usize) -> Result<(), SyntaxError> {
    let mut chars = name.chars();
    let valid = chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
        && chars.all(|c| c == '_' || c.is_alphanumeric());
    if valid {
        Ok(())
    } else {
        Err(SyntaxError::new(format!("bad character in group name '{name}'"), position))
    }
}

/// The (minimum, maximum) number of characters `node` can match, as computed
/// by CPython's `getwidth`. `group_widths` holds the widths of closed groups.
fn width(node: &Node, group_widths: &[Option<(u64, u64)>]) -> (u64, u64) {
    let group_width = |number: u32| group_widths.get(number as usize - 1).copied().flatten();
    match node {
        Node::Empty | Node::Assertion(_) | Node::Look { .. } => (0, 0),
        Node::Literal(_) | Node::Dot | Node::Class(_) => (1, 1),
        Node::Concat(nodes) => nodes.iter().fold((0, 0), |(low, high), node| {
            let (node_low, node_high) = width(node, group_widths);
            (low.saturating_add(node_low), high.saturating_add(node_high))
        }),
        Node::Alternation(branches) => {
            branches.iter().fold((MAX_WIDTH, 0), |(low, high), branch| {
                let (branch_low, branch_high) = width(branch, group_widths);
                (low.min(branch_low), high.max(branch_high))
            })
        }
        Node::Repeat { node, min, max, .. } => {
            let (low, high) = width(node, group_widths);
            let high = match max {
                Some(max) => high.saturating_mul(u64::from(*max)),
                None if high == 0 => 0,
                None => MAX_WIDTH,
            };
            (low.saturating_mul(u64::from(*min)), high)
        }
        Node::Group { kind: GroupKind::Capture(number), node } => {
            group_width(*number).unwrap_or_else(|| width(node, group_widths))
        }
        Node::Group { node, .. } => width(node, group_widths),
        // A group is always closed before it can be referenced.
        Node::Backref(number) => group_width(*number).unwrap_or((0, MAX_WIDTH)),
        Node::Conditional { yes, no, .. } => {
            let (yes_low, yes_high) = width(yes, group_widths);
            let (no_low, no_high) = width(no, group_widths);
            (yes_low.min(no_low), yes_high.max(no_high))
        }
    }
}
