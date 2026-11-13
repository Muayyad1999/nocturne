//! Python `re` patterns on top of the `regex` and `fancy-regex` crates.
//!
//! TextFSM templates are written for Python's `re` module. A pattern is parsed
//! with a faithful port of CPython's regex parser ([`parser`]), rendered in the
//! syntax of the Rust engines ([`emit`]) and compiled with the linear-time
//! `regex` crate, or with the backtracking `fancy-regex` crate when it uses
//! look-around or back-references.
//!
//! Python's classes are Unicode aware, and so is the translation. That has a
//! price in the `regex` crate: `\w` alone compiles to about 50 KiB of tables,
//! against 100 bytes for its ASCII counterpart. Device output is nearly
//! always plain ASCII, though, and on ASCII text a pattern compiled without
//! Unicode support matches exactly the same. So a pattern is compiled in that
//! small flavour first, and in the Unicode-aware flavour only once it meets a
//! text with other characters (see [`emit::Charset`]).

mod ast;
mod emit;
mod language;
mod loops;
mod parser;

use std::sync::OnceLock;

pub(crate) use language::finite_language;

use emit::{Charset, GroupSlot};

#[cfg(test)]
mod tests;

/// Compiled size limit for one pattern. Python has no such limit, and the
/// default of the `regex` crate (10 MiB) would reject Unicode-aware patterns
/// that are unremarkable in Python, such as `\w{1,255}`.
const SIZE_LIMIT: usize = 64 << 20;

/// Steps the backtracking engine may spend on one match before giving up; ten
/// times its default, to stay clear of long lines with several look-arounds.
const BACKTRACK_LIMIT: usize = 10_000_000;

/// A compiled Python regular expression.
///
/// Matching follows `re.match`: the pattern has to match at the start of the
/// text but not necessarily up to its end.
#[derive(Debug)]
pub(crate) struct Regex {
    /// Serves ASCII text. Missing if the pattern needs the backtracking
    /// engine, or if its ASCII flavour would not be equivalent.
    ascii: Option<regex::bytes::Regex>,
    /// Serves all other text.
    unicode: UnicodeFlavour,
    group_names: Vec<Option<String>>,
    /// Where each Python group lives in the translated pattern.
    groups: Vec<GroupSlot>,
}

/// The Unicode-aware flavour of a pattern, compiled when it is first needed.
#[derive(Debug)]
struct UnicodeFlavour {
    pattern: String,
    needs_backtracking: bool,
    compiled: OnceLock<Result<Compiled, String>>,
}

#[derive(Debug)]
enum Compiled {
    /// The `regex` crate.
    Linear(regex::Regex),
    /// The `fancy-regex` crate, for what the `regex` crate cannot express.
    Backtracking(Box<fancy_regex::Regex>),
}

impl UnicodeFlavour {
    fn get(&self) -> Result<&Compiled, &str> {
        let compiled = self.compiled.get_or_init(|| {
            if self.needs_backtracking {
                fancy_regex::RegexBuilder::new(&self.pattern)
                    .delegate_size_limit(SIZE_LIMIT)
                    .backtrack_limit(BACKTRACK_LIMIT)
                    .build()
                    .map(|regex| Compiled::Backtracking(Box::new(regex)))
                    .map_err(|error| error.to_string())
            } else {
                regex::RegexBuilder::new(&self.pattern)
                    .size_limit(SIZE_LIMIT)
                    .build()
                    .map(Compiled::Linear)
                    .map_err(|error| error.to_string())
            }
        });
        compiled.as_ref().map_err(String::as_str)
    }
}

impl Regex {
    pub fn new(pattern: &str) -> Result<Regex, RegexError> {
        let unsupported =
            |error: &dyn std::fmt::Display| RegexError::Unsupported(error.to_string());
        let ast = parser::parse(pattern)?;
        let translation = emit::translate(&ast, Charset::Unicode);
        let unicode = UnicodeFlavour {
            pattern: translation.pattern,
            needs_backtracking: translation.needs_backtracking,
            compiled: OnceLock::new(),
        };
        let ascii = emit::translate(&ast, Charset::Ascii);
        let ascii = if ascii.needs_backtracking || !emit::has_ascii_flavour(&ast) {
            // The Unicode flavour serves every text, so compile it right away.
            unicode.get().map_err(|error| unsupported(&error))?;
            None
        } else {
            let regex = regex::bytes::RegexBuilder::new(&ascii.pattern)
                .unicode(false)
                .size_limit(SIZE_LIMIT)
                .build()
                .map_err(|error| unsupported(&error))?;
            Some(regex)
        };
        Ok(Regex {
            ascii,
            unicode,
            group_names: ast.group_names,
            groups: translation.groups,
        })
    }

    /// The named capture groups as `(group number, name)`, in pattern order.
    pub fn named_groups(&self) -> impl Iterator<Item = (usize, &str)> {
        self.group_names
            .iter()
            .enumerate()
            .filter_map(|(index, name)| name.as_deref().map(|name| (index + 1, name)))
    }

    /// `re.match(pattern, text)`.
    pub fn match_start<'r, 't>(
        &'r self,
        text: &'t str,
    ) -> Result<Option<Captures<'r, 't>>, MatchError> {
        match &self.ascii {
            Some(ascii) if text.is_ascii() => {
                // `captures` allocates its result before searching; the vast
                // majority of attempts fail, and `is_match` rejects those for free.
                if !ascii.is_match(text.as_bytes()) {
                    return Ok(None);
                }
                let found = ascii.captures(text.as_bytes());
                Ok(found.map(|captures| self.captures(Found::Ascii(captures, text))))
            }
            _ => self.match_start_unicode(text),
        }
    }

    /// [`Regex::match_start`] with the Unicode-aware flavour, whatever the text.
    pub fn match_start_unicode<'r, 't>(
        &'r self,
        text: &'t str,
    ) -> Result<Option<Captures<'r, 't>>, MatchError> {
        let compiled = self
            .unicode
            .get()
            .map_err(|error| MatchError(error.to_owned()))?;
        let found = match compiled {
            Compiled::Linear(regex) => {
                if !regex.is_match(text) {
                    return Ok(None);
                }
                regex.captures(text).map(Found::Unicode)
            }
            Compiled::Backtracking(regex) => regex
                .captures(text)
                .map_err(|error| MatchError(error.to_string()))?
                .map(Found::Backtracking),
        };
        Ok(found.map(|found| self.captures(found)))
    }

    fn captures<'r, 't>(&'r self, found: Found<'t>) -> Captures<'r, 't> {
        Captures {
            found,
            groups: &self.groups,
        }
    }
}

/// The groups of a successful match.
pub(crate) struct Captures<'r, 't> {
    found: Found<'t>,
    groups: &'r [GroupSlot],
}

/// The captures of whichever engine matched.
enum Found<'t> {
    /// Byte offsets into the ASCII text next to it.
    Ascii(regex::bytes::Captures<'t>, &'t str),
    Unicode(regex::Captures<'t>),
    Backtracking(fancy_regex::Captures<'t, str>),
}

impl<'t> Captures<'_, 't> {
    /// The text captured by a group, numbered as in the Python pattern (0 is
    /// the whole match), or `None` when the group did not take part in the match.
    pub fn get(&self, group: usize) -> Option<&'t str> {
        let Some(index) = group.checked_sub(1) else {
            return self.emitted(0);
        };
        let slot = self.groups.get(index)?;
        slot.empty_iteration
            .and_then(|marker| self.emitted(marker))
            .or_else(|| self.emitted(slot.primary))
    }

    /// A group by its number in the translated pattern.
    fn emitted(&self, group: usize) -> Option<&'t str> {
        match &self.found {
            Found::Ascii(captures, text) => captures.get(group).map(|m| &text[m.range()]),
            Found::Unicode(captures) => captures.get(group).map(|m| m.as_str()),
            Found::Backtracking(captures) => captures.get(group).map(|m| m.as_str()),
        }
    }
}

/// A pattern that could not be compiled.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RegexError {
    /// Not a valid Python regular expression.
    #[error(transparent)]
    Syntax(#[from] SyntaxError),
    /// Valid in Python, but beyond what the Rust engines can compile.
    #[error("unsupported regular expression: {0}")]
    Unsupported(String),
}

/// A syntax error in a Python regular expression, worded like Python's `re.error`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message} at position {position}")]
pub(crate) struct SyntaxError {
    pub message: String,
    /// Character offset into the pattern.
    pub position: usize,
}

impl SyntaxError {
    fn new(message: impl Into<String>, position: usize) -> SyntaxError {
        SyntaxError {
            message: message.into(),
            position,
        }
    }
}

/// A match could not be decided: the backtracking engine exceeded its step
/// limit, or the Unicode-aware flavour of the pattern turned out too large
/// when a text outside ASCII first required it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub(crate) struct MatchError(pub String);

#[cfg(test)]
impl Regex {
    /// Number of capture groups, not counting the whole match.
    pub fn group_count(&self) -> usize {
        self.group_names.len()
    }

    /// Whether the pattern runs on the backtracking engine for every text.
    pub fn uses_backtracking(&self) -> bool {
        self.ascii.is_none() && self.unicode.needs_backtracking
    }

    /// Whether ASCII text is served by the ASCII flavour.
    pub fn has_ascii_flavour(&self) -> bool {
        self.ascii.is_some()
    }
}
