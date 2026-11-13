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
    engine: Engine,
    group_names: Vec<Option<String>>,
    /// Where each Python group lives in the translated pattern.
    groups: Vec<GroupSlot>,
}

#[derive(Debug)]
enum Engine {
    /// The `regex` crate. `ascii` serves ASCII text; it is missing for the
    /// rare pattern whose ASCII flavour would not be equivalent.
    Linear {
        ascii: Option<regex::bytes::Regex>,
        unicode: UnicodeFlavour,
    },
    /// The `fancy-regex` crate, for what the `regex` crate cannot express.
    Backtracking(Box<fancy_regex::Regex>),
}

/// The Unicode-aware flavour of a pattern, compiled when it is first needed.
#[derive(Debug)]
struct UnicodeFlavour {
    pattern: String,
    compiled: OnceLock<Result<regex::Regex, String>>,
}

impl UnicodeFlavour {
    fn get(&self) -> Result<&regex::Regex, &str> {
        let compiled = self.compiled.get_or_init(|| {
            regex::RegexBuilder::new(&self.pattern)
                .size_limit(SIZE_LIMIT)
                .build()
                .map_err(|error| error.to_string())
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
        let engine = if translation.needs_backtracking {
            let regex = fancy_regex::RegexBuilder::new(&translation.pattern)
                .delegate_size_limit(SIZE_LIMIT)
                .backtrack_limit(BACKTRACK_LIMIT)
                .build()
                .map_err(|error| unsupported(&error))?;
            Engine::Backtracking(Box::new(regex))
        } else {
            let unicode = UnicodeFlavour {
                pattern: translation.pattern,
                compiled: OnceLock::new(),
            };
            let ascii = if emit::has_ascii_flavour(&ast) {
                let pattern = emit::translate(&ast, Charset::Ascii).pattern;
                let regex = regex::bytes::RegexBuilder::new(&pattern)
                    .unicode(false)
                    .size_limit(SIZE_LIMIT)
                    .build()
                    .map_err(|error| unsupported(&error))?;
                Some(regex)
            } else {
                // Nothing else would check that the pattern compiles.
                unicode.get().map_err(|error| unsupported(&error))?;
                None
            };
            Engine::Linear { ascii, unicode }
        };
        Ok(Regex {
            engine,
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
        let found = match &self.engine {
            // `captures` allocates its result before searching; the vast
            // majority of attempts fail, and `is_match` rejects those for free.
            Engine::Linear {
                ascii: Some(ascii), ..
            } if text.is_ascii() => {
                if !ascii.is_match(text.as_bytes()) {
                    return Ok(None);
                }
                ascii
                    .captures(text.as_bytes())
                    .map(|captures| Found::Ascii(captures, text))
            }
            Engine::Linear { unicode, .. } => {
                let unicode = unicode
                    .get()
                    .map_err(|error| MatchError(error.to_owned()))?;
                if !unicode.is_match(text) {
                    return Ok(None);
                }
                unicode.captures(text).map(Found::Unicode)
            }
            Engine::Backtracking(regex) => regex
                .captures(text)
                .map_err(|error| MatchError(error.to_string()))?
                .map(Found::Backtracking),
        };
        Ok(found.map(|found| Captures {
            found,
            groups: &self.groups,
        }))
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

    /// Whether the pattern runs on the backtracking engine.
    pub fn uses_backtracking(&self) -> bool {
        matches!(self.engine, Engine::Backtracking(_))
    }

    /// Whether ASCII text is served by the ASCII flavour.
    pub fn has_ascii_flavour(&self) -> bool {
        matches!(self.engine, Engine::Linear { ascii: Some(_), .. })
    }

    /// Like [`Regex::match_start`], but never uses the ASCII flavour.
    pub fn match_start_unicode<'r, 't>(
        &'r self,
        text: &'t str,
    ) -> Result<Option<Captures<'r, 't>>, MatchError> {
        let Engine::Linear { unicode, .. } = &self.engine else {
            return self.match_start(text);
        };
        let unicode = unicode
            .get()
            .map_err(|error| MatchError(error.to_owned()))?;
        Ok(unicode.captures(text).map(|captures| Captures {
            found: Found::Unicode(captures),
            groups: &self.groups,
        }))
    }
}
