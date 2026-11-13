//! Python `re` patterns on top of the `regex` and `fancy-regex` crates.
//!
//! TextFSM templates are written for Python's `re` module. A pattern is parsed
//! with a faithful port of CPython's regex parser ([`parser`]), rendered in the
//! syntax of the Rust engines ([`emit`]) and compiled with the linear-time
//! `regex` crate, or with the backtracking `fancy-regex` crate when it uses
//! look-around or back-references.

mod ast;
mod emit;
mod language;
mod parser;

pub(crate) use language::finite_language;

use emit::GroupSlot;

#[cfg(test)]
mod tests;

/// Compiled size limit for one pattern. Rule patterns embed the regex of every
/// value they use and classes are Unicode aware, so the default of 10 MiB is
/// occasionally too small.
const SIZE_LIMIT: usize = 64 << 20;

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
    Plain(regex::Regex),
    Backtracking(Box<fancy_regex::Regex>),
}

impl Regex {
    pub fn new(pattern: &str) -> Result<Regex, RegexError> {
        let ast = parser::parse(pattern)?;
        let translation = emit::translate(&ast);
        let engine = if translation.needs_backtracking {
            fancy_regex::RegexBuilder::new(&translation.pattern)
                .delegate_size_limit(SIZE_LIMIT)
                .build()
                .map(|regex| Engine::Backtracking(Box::new(regex)))
                .map_err(|error| RegexError::Unsupported(error.to_string()))?
        } else {
            regex::RegexBuilder::new(&translation.pattern)
                .size_limit(SIZE_LIMIT)
                .build()
                .map(Engine::Plain)
                .map_err(|error| RegexError::Unsupported(error.to_string()))?
        };
        Ok(Regex { engine, group_names: ast.group_names, groups: translation.groups })
    }

    /// The named capture groups as `(group number, name)`, in pattern order.
    pub fn named_groups(&self) -> impl Iterator<Item = (usize, &str)> {
        self.group_names
            .iter()
            .enumerate()
            .filter_map(|(index, name)| name.as_deref().map(|name| (index + 1, name)))
    }

    /// Number of capture groups, not counting the whole match.
    #[cfg(test)]
    pub fn group_count(&self) -> usize {
        self.group_names.len()
    }

    /// Whether the pattern runs on the backtracking engine.
    #[cfg(test)]
    pub fn uses_backtracking(&self) -> bool {
        matches!(self.engine, Engine::Backtracking(_))
    }

    /// `re.match(pattern, text)`.
    pub fn match_start<'r, 't>(
        &'r self,
        text: &'t str,
    ) -> Result<Option<Captures<'r, 't>>, MatchError> {
        let found = match &self.engine {
            Engine::Plain(regex) => {
                // `captures` allocates its result before searching; the vast
                // majority of attempts fail, and `is_match` rejects those for free.
                if !regex.is_match(text) {
                    return Ok(None);
                }
                regex.captures(text).map(EngineCaptures::Plain)
            }
            Engine::Backtracking(regex) => regex
                .captures(text)
                .map_err(|error| MatchError(error.to_string()))?
                .map(EngineCaptures::Backtracking),
        };
        Ok(found.map(|found| Captures { found, groups: &self.groups }))
    }
}

/// The groups of a successful match.
pub(crate) struct Captures<'r, 't> {
    found: EngineCaptures<'t>,
    groups: &'r [GroupSlot],
}

enum EngineCaptures<'t> {
    Plain(regex::Captures<'t>),
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
            EngineCaptures::Plain(captures) => captures.get(group).map(|m| m.as_str()),
            EngineCaptures::Backtracking(captures) => captures.get(group).map(|m| m.as_str()),
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
        SyntaxError { message: message.into(), position }
    }
}

/// The backtracking engine gave up on a match (it limits the number of steps).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub(crate) struct MatchError(pub String);
