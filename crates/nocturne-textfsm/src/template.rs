//! The TextFSM template language: `Value` declarations followed by states
//! and their rules.
//!
//! The parser mirrors `TextFSM._Parse` of the Python reference implementation
//! statement by statement, including its strictness about white space.

use crate::error::{RunError, TemplateError};
use crate::pyregex::Regex;
use crate::value::Record;
use crate::{fsm, text};

/// Longest value or state name the language allows.
const MAX_NAME_LEN: usize = 48;

/// A compiled TextFSM template.
///
/// Compiling is the expensive part: every rule becomes a regular expression.
/// A compiled template is immutable and can be shared between threads; each
/// [`run`](Template::run) starts from a clean state.
///
/// The language is implemented the way the Python reference implementation
/// (`google/textfsm`) behaves, down to its quirks. What differs:
///
/// * Where Python puts `None` into a list (a `List` value in an optional part
///   of a rule that took no part in the match), the item is the string
///   `"None"`, which is what ntc-templates' `parse_output` returns for it.
/// * A `List` value whose regex contains nested *named* groups records the
///   matched text like any other `List` value; Python records a dictionary of
///   the nested groups.
/// * Regular expressions are translated from Python's syntax and run on the
///   `regex` crate, or on `fancy-regex` when they use look-around or
///   back-references. The classes `\d`, `\w` and `\s` are Unicode aware as in
///   Python. A loop whose body can match the empty string other than as its
///   last alternative, such as `(a*|b)*` or `(a*?)*`, may match differently.
///   The `(?a)` flag and `\N{...}` escapes are not supported.
#[derive(Debug)]
pub struct Template {
    header: Vec<String>,
    pub(crate) values: Vec<ValueSpec>,
    pub(crate) states: Vec<State>,
    /// Index of the `Start` state.
    pub(crate) start: usize,
    /// Whether reaching the end of the input records the pending row. A
    /// template switches this off by declaring an empty `EOF` state.
    pub(crate) records_at_end: bool,
}

impl Template {
    /// Compiles a template.
    ///
    /// The error names the offending line of `source`.
    pub fn parse(source: &str) -> Result<Template, TemplateError> {
        let mut lines = Lines::new(source);
        let values = parse_values(&mut lines)?;
        let mut states: Vec<StateSource> = Vec::new();
        while let Some(state) = parse_state(&mut lines, &values, &states)? {
            states.push(state);
        }
        link(values, states, lines.count().max(1))
    }

    /// Value names in declaration order (the table header).
    pub fn header(&self) -> &[String] {
        &self.header
    }

    /// Run the state machine over CLI output and return the recorded rows.
    ///
    /// Lines are split the way Python's `str.splitlines` does, so `\r\n` and a
    /// lone `\r` both end a line.
    pub fn run(&self, input: &str) -> Result<Vec<Record>, RunError> {
        fsm::run(self, input)
    }

    /// Names of the values declared with the `Key` option.
    pub(crate) fn keys(&self) -> impl Iterator<Item = &str> {
        self.values
            .iter()
            .filter(|value| value.options.contains(&ValueOption::Key))
            .map(|value| value.name.as_str())
    }

    /// Compiles the Unicode-aware flavour of every rule, which otherwise
    /// happens when a rule first meets a line that is not plain ASCII.
    #[cfg(test)]
    pub(crate) fn compile_unicode_flavours(&self) -> Result<(), String> {
        for rule in self.states.iter().flat_map(|state| &state.rules) {
            rule.regex
                .match_start_unicode("")
                .map_err(|error| format!("line {}: {error}", rule.line))?;
        }
        Ok(())
    }
}

/// A `Value` declaration.
#[derive(Debug)]
pub(crate) struct ValueSpec {
    pub name: String,
    /// In declaration order, which decides the order of the checks when a row is recorded.
    pub options: Vec<ValueOption>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ValueOption {
    /// The row is dropped unless this value is set.
    Required,
    /// The value survives `Record` and `Clear`.
    Filldown,
    /// A match is copied into earlier rows that lack this value.
    Fillup,
    /// Part of the row's identity; only used when tables are merged.
    Key,
    /// Matches accumulate into a list.
    List,
}

#[derive(Debug)]
pub(crate) struct State {
    pub rules: Vec<Rule>,
}

#[derive(Debug)]
pub(crate) struct Rule {
    /// 1-based line in the template.
    pub line: usize,
    pub regex: Regex,
    /// `(capture group, value index)` for every named group that is a value.
    pub assignments: Vec<(usize, usize)>,
    pub line_op: LineOp,
    pub record_op: RecordOp,
    /// Where to go after a `Next`; `None` stays in the current state.
    pub transition: Option<Transition>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum LineOp {
    /// Read the next line and start again at the first rule.
    #[default]
    Next,
    /// Keep the line and try the following rules.
    Continue,
    /// Abort, optionally with a message.
    Error(Option<String>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RecordOp {
    #[default]
    NoRecord,
    Record,
    Clear,
    Clearall,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Transition {
    State(usize),
    /// Stop without recording the pending row.
    End,
    /// Stop as if the input had ended.
    Eof,
}

// --- parsing -----------------------------------------------------------------

/// The template's lines without trailing white space, with their numbers.
struct Lines<'a> {
    lines: Vec<&'a str>,
    next: usize,
}

impl<'a> Lines<'a> {
    fn new(source: &'a str) -> Lines<'a> {
        Lines {
            lines: text::template_lines(source).map(text::rstrip).collect(),
            next: 0,
        }
    }

    /// The next line and its 1-based number.
    fn next(&mut self) -> Option<(usize, &'a str)> {
        let line = *self.lines.get(self.next)?;
        self.next += 1;
        Some((self.next, line))
    }

    fn count(&self) -> usize {
        self.lines.len()
    }
}

fn is_comment(line: &str) -> bool {
    line.trim_start_matches(text::is_space).starts_with('#')
}

/// A parsed `Value` line.
struct Declaration {
    spec: ValueSpec,
    /// The value's regex as a named group, as substituted for `${NAME}` in rules.
    capture: String,
}

/// The block of `Value` lines at the top, up to the first blank line.
fn parse_values(lines: &mut Lines<'_>) -> Result<Vec<Declaration>, TemplateError> {
    let mut values: Vec<Declaration> = Vec::new();
    while let Some((number, line)) = lines.next() {
        if line.is_empty() {
            break;
        }
        if is_comment(line) {
            continue;
        }
        if !line.starts_with("Value ") {
            let message = if values.is_empty() {
                "no Value definitions found"
            } else {
                "expected a blank line after the last Value"
            };
            return Err(TemplateError::new(number, message));
        }
        let value = parse_value(line).map_err(|message| TemplateError::new(number, message))?;
        if values
            .iter()
            .any(|existing| existing.spec.name == value.spec.name)
        {
            return Err(TemplateError::new(
                number,
                format!("duplicate declaration of Value '{}'", value.spec.name),
            ));
        }
        values.push(value);
    }
    Ok(values)
}

/// `Value [Options] NAME (regex)`
fn parse_value(line: &str) -> Result<Declaration, String> {
    let tokens: Vec<&str> = line.split(' ').collect();
    if tokens.len() < 3 {
        return Err("expected 'Value [Options] NAME (regex)'".to_owned());
    }
    // The options are present unless the third token already starts the regex.
    let (options, name, regex) = if tokens[2].starts_with('(') {
        (Vec::new(), tokens[1], tokens[2..].join(" "))
    } else {
        (parse_options(tokens[1])?, tokens[2], tokens[3..].join(" "))
    };
    if name.chars().count() > MAX_NAME_LEN {
        return Err(format!(
            "Value name '{name}' is longer than {MAX_NAME_LEN} characters"
        ));
    }
    let Some(inner) = regex
        .strip_prefix('(')
        .filter(|_| regex.ends_with(')') && !regex.ends_with("\\)"))
    else {
        return Err(format!(
            "the regex of Value '{name}' must be enclosed in '(' and ')': {regex}"
        ));
    };
    Regex::new(&regex)
        .map_err(|error| format!("invalid regular expression in Value '{name}': {error}"))?;
    Ok(Declaration {
        capture: format!("(?P<{name}>{inner}"),
        spec: ValueSpec {
            name: name.to_owned(),
            options,
        },
    })
}

fn parse_options(list: &str) -> Result<Vec<ValueOption>, String> {
    let mut options = Vec::new();
    for name in list.split(',') {
        let option = match name {
            "Required" => ValueOption::Required,
            "Filldown" => ValueOption::Filldown,
            "Fillup" => ValueOption::Fillup,
            "Key" => ValueOption::Key,
            "List" => ValueOption::List,
            _ => return Err(format!("unknown Value option '{name}'")),
        };
        if options.contains(&option) {
            return Err(format!("duplicate Value option '{name}'"));
        }
        options.push(option);
    }
    Ok(options)
}

/// A state as written, with transitions still referring to states by name.
struct StateSource {
    name: String,
    line: usize,
    rules: Vec<RuleSource>,
}

struct RuleSource {
    rule: Rule,
    /// The state named after the operators, or the message of an `Error`.
    target: Option<String>,
}

/// The next state: its name on a line of its own, then rules up to a blank line.
fn parse_state(
    lines: &mut Lines<'_>,
    values: &[Declaration],
    earlier: &[StateSource],
) -> Result<Option<StateSource>, TemplateError> {
    let (line, name) = loop {
        match lines.next() {
            None => return Ok(None),
            Some((_, text)) if text.is_empty() || is_comment(text) => continue,
            Some(found) => break found,
        }
    };
    let reserved = [
        "Continue", "Next", "Error", "Clear", "Clearall", "Record", "NoRecord",
    ];
    if !name.chars().all(text::is_word)
        || name.chars().count() > MAX_NAME_LEN
        || reserved.contains(&name)
    {
        return Err(TemplateError::new(
            line,
            format!("invalid state name '{name}'"),
        ));
    }
    if earlier.iter().any(|state| state.name == name) {
        return Err(TemplateError::new(
            line,
            format!("duplicate state name '{name}'"),
        ));
    }
    let mut rules = Vec::new();
    while let Some((number, text)) = lines.next() {
        if text.is_empty() {
            break;
        }
        if is_comment(text) {
            continue;
        }
        if ![" ^", "  ^", "\t^"]
            .iter()
            .any(|indent| text.starts_with(indent))
        {
            return Err(TemplateError::new(
                number,
                "a rule must start with one or two spaces or a tab, followed by '^'",
            ));
        }
        rules.push(parse_rule(text, number, values)?);
    }
    Ok(Some(StateSource {
        name: name.to_owned(),
        line,
        rules,
    }))
}

/// `^regex [-> action]`
fn parse_rule(
    line: &str,
    number: usize,
    values: &[Declaration],
) -> Result<RuleSource, TemplateError> {
    let error = |message: String| TemplateError::new(number, message);
    let line = text::strip(line);
    let (pattern, action) = split_action(line);
    // The reference skips the substitution altogether when no value is declared.
    let pattern = if values.is_empty() {
        pattern.to_owned()
    } else {
        substitute(pattern, values).map_err(error)?
    };
    let regex = Regex::new(&pattern)
        .map_err(|cause| error(format!("invalid regular expression '{pattern}': {cause}")))?;
    let assignments = regex
        .named_groups()
        .filter_map(|(group, name)| {
            let value = values.iter().position(|value| value.spec.name == name)?;
            Some((group, value))
        })
        .collect();

    let action = match action {
        None => Action::default(),
        Some(action) => {
            parse_action(action).ok_or_else(|| error(format!("badly formatted rule '{line}'")))?
        }
    };
    let quoted = action.target.is_some_and(|target| target.starts_with('"'));
    let (line_op, target) = match (action.line_op, action.target) {
        (LineOp::Error(_), target) => {
            let unquoted = |text: &str| {
                text.strip_prefix('"')
                    .and_then(|text| text.strip_suffix('"'))
                    .unwrap_or(text)
                    .to_owned()
            };
            let message = target.map(unquoted);
            (LineOp::Error(message), None)
        }
        (LineOp::Continue, Some(target)) => {
            return Err(error(format!(
                "'Continue' cannot change the state (to '{target}')"
            )));
        }
        (_, Some(_)) if quoted => {
            return Err(error(
                "a state name may only contain letters, digits and '_'".to_owned(),
            ));
        }
        (line_op, target) => (line_op, target.map(str::to_owned)),
    };
    let rule = Rule {
        line: number,
        regex,
        assignments,
        line_op,
        record_op: action.record_op,
        transition: None,
    };
    Ok(RuleSource { rule, target })
}

/// Splits a rule at its last ` ->` into the regex and the action text.
fn split_action(line: &str) -> (&str, Option<&str>) {
    for (arrow, _) in line.rmatch_indices("->") {
        if let Some(space) = line[..arrow]
            .chars()
            .next_back()
            .filter(|&c| text::is_space(c))
        {
            return (&line[..arrow - space.len_utf8()], Some(&line[arrow + 2..]));
        }
    }
    (line, None)
}

/// Replaces `${NAME}` and `$NAME` by the value's regex and `$$` by `$`, like
/// Python's `string.Template.substitute`.
fn substitute(pattern: &str, values: &[Declaration]) -> Result<String, String> {
    let is_name_start = |c: char| c == '_' || c.is_ascii_alphabetic();
    let is_name_char = |c: char| c == '_' || c.is_ascii_alphanumeric();
    let mut result = String::with_capacity(pattern.len());
    let mut rest = pattern;
    while let Some(dollar) = rest.find('$') {
        result.push_str(&rest[..dollar]);
        let after = &rest[dollar + 1..];
        let (name, remainder) = if let Some(remainder) = after.strip_prefix('$') {
            result.push('$');
            rest = remainder;
            continue;
        } else if after.starts_with(is_name_start) {
            after.split_at(after.find(|c| !is_name_char(c)).unwrap_or(after.len()))
        } else if let Some((name, remainder)) = after
            .strip_prefix('{')
            .and_then(|braced| braced.split_once('}'))
            && name.starts_with(is_name_start)
            && name.chars().all(is_name_char)
        {
            (name, remainder)
        } else {
            return Err(format!(
                "invalid '$' in '{pattern}': write '$$' for the end of the line and '${{NAME}}' for a value"
            ));
        };
        let value = values
            .iter()
            .find(|value| value.spec.name == name)
            .ok_or_else(|| format!("unknown value '{name}' in '{pattern}'"))?;
        result.push_str(&value.capture);
        rest = remainder;
    }
    result.push_str(rest);
    Ok(result)
}

/// The part of a rule after `->`, as written.
#[derive(Debug, Default, PartialEq, Eq)]
struct Action<'a> {
    line_op: LineOp,
    record_op: RecordOp,
    /// A state name, or the (possibly quoted) message of an `Error`.
    target: Option<&'a str>,
}

/// `[LineOp][.RecordOp] [NewState]`, where the operators may be missing.
/// Returns `None` for anything else.
fn parse_action(action: &str) -> Option<Action<'_>> {
    if action.is_empty() {
        return Some(Action::default());
    }
    // White space is required after the arrow.
    let rest = action
        .strip_prefix(text::is_space)?
        .trim_start_matches(text::is_space);
    let is_target = |text: &str| {
        (!text.is_empty() && text.chars().all(text::is_word))
            || (text.len() >= 2 && text.starts_with('"') && text.ends_with('"'))
    };
    let (operators, target) = match rest.split_once(text::is_space) {
        Some((operators, target)) => (operators, Some(target.trim_start_matches(text::is_space))),
        None => (rest, None),
    };
    if let Some((line_op, record_op)) = parse_operators(operators) {
        return match target {
            Some(target) if !is_target(target) => None,
            target => Some(Action {
                line_op,
                record_op,
                target,
            }),
        };
    }
    // No operators: the whole text is the new state.
    is_target(rest).then(|| Action {
        target: Some(rest),
        ..Action::default()
    })
}

/// `LineOp`, `LineOp.RecordOp` or `RecordOp`.
fn parse_operators(text: &str) -> Option<(LineOp, RecordOp)> {
    let line_op = |name: &str| match name {
        "Next" => Some(LineOp::Next),
        "Continue" => Some(LineOp::Continue),
        "Error" => Some(LineOp::Error(None)),
        _ => None,
    };
    let record_op = |name: &str| match name {
        "NoRecord" => Some(RecordOp::NoRecord),
        "Record" => Some(RecordOp::Record),
        "Clear" => Some(RecordOp::Clear),
        "Clearall" => Some(RecordOp::Clearall),
        _ => None,
    };
    match text.split_once('.') {
        Some((line, record)) => Some((line_op(line)?, record_op(record)?)),
        None => line_op(text)
            .map(|line| (line, RecordOp::default()))
            .or_else(|| record_op(text).map(|record| (LineOp::default(), record))),
    }
}

/// Checks the state graph and resolves state names (`TextFSM._ValidateFSM`).
fn link(
    values: Vec<Declaration>,
    states: Vec<StateSource>,
    last_line: usize,
) -> Result<Template, TemplateError> {
    let is_terminal = |name: &str| matches!(name, "End" | "EOF");
    // `End` and `EOF` are never entered, so they get no slot.
    let names: Vec<String> = states
        .iter()
        .filter(|state| !is_terminal(&state.name))
        .map(|state| state.name.clone())
        .collect();
    let Some(start) = names.iter().position(|name| name == "Start") else {
        return Err(TemplateError::new(last_line, "missing state 'Start'"));
    };
    for terminal in ["End", "EOF"] {
        if let Some(state) = states.iter().find(|state| state.name == terminal)
            && !state.rules.is_empty()
        {
            return Err(TemplateError::new(
                state.line,
                format!("the '{terminal}' state must be empty"),
            ));
        }
    }
    let records_at_end = !states.iter().any(|state| state.name == "EOF");
    let mut linked = Vec::with_capacity(names.len());
    for state in states.into_iter().filter(|state| !is_terminal(&state.name)) {
        let mut rules = Vec::with_capacity(state.rules.len());
        for RuleSource { mut rule, target } in state.rules {
            rule.transition = match target.as_deref() {
                None => None,
                Some("End") => Some(Transition::End),
                Some("EOF") => Some(Transition::Eof),
                Some(target) => {
                    let index = names
                        .iter()
                        .position(|name| name == target)
                        .ok_or_else(|| {
                            TemplateError::new(
                                rule.line,
                                format!(
                                    "state '{target}' not found, referenced in state '{}'",
                                    state.name
                                ),
                            )
                        })?;
                    Some(Transition::State(index))
                }
            };
            rules.push(rule);
        }
        linked.push(State { rules });
    }
    let values: Vec<ValueSpec> = values.into_iter().map(|value| value.spec).collect();
    Ok(Template {
        header: values.iter().map(|value| value.name.clone()).collect(),
        values,
        states: linked,
        start,
        records_at_end,
    })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn error(source: &str) -> (usize, String) {
        let error = Template::parse(source).expect_err("the template must be rejected");
        (error.line(), error.message().to_owned())
    }

    #[test]
    fn compiled_templates_can_be_shared_between_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Template>();
    }

    #[test]
    fn header_lists_the_values_in_declaration_order() {
        let template = Template::parse("Value B (b)\nValue Key,List A (a)\n\nStart\n").unwrap();
        assert_eq!(template.header(), ["B", "A"]);
        assert_eq!(template.keys().collect::<Vec<_>>(), ["A"]);
    }

    #[test]
    fn accepts_any_line_ending() {
        for ending in ["\n", "\r\n", "\r"] {
            let source = ["Value A (\\S+)", "", "Start", "  ^${A} -> Record", ""].join(ending);
            let rows = Template::parse(&source).unwrap().run("x\ny").unwrap();
            assert_eq!(rows.len(), 2, "{ending:?}");
        }
    }

    /// Expected values: what the reference's `MATCH_ACTION` regex yields.
    #[test]
    fn splits_rules_at_the_last_arrow() {
        assert_eq!(split_action("^a -> Record"), ("^a", Some(" Record")));
        assert_eq!(split_action("^a -> b -> Next"), ("^a -> b", Some(" Next")));
        assert_eq!(split_action("^a->b"), ("^a->b", None));
        assert_eq!(split_action("^a  ->"), ("^a ", Some("")));
        assert_eq!(split_action("^a\t-> x"), ("^a", Some(" x")));
        assert_eq!(split_action("^a"), ("^a", None));
        assert_eq!(split_action("^a ->b -> "), ("^a ->b", Some(" ")));
        assert_eq!(split_action("-> x"), ("-> x", None));
        assert_eq!(split_action(" -> x"), ("", Some(" x")));
        assert_eq!(split_action("^a\u{a0}-> x"), ("^a", Some(" x")));
        assert_eq!(split_action("^a -> -> "), ("^a ->", Some(" ")));
    }

    /// Expected values: what the reference's `ACTION_RE`, `ACTION2_RE` and
    /// `ACTION3_RE` yield, tried in that order.
    #[test]
    fn parses_actions() {
        use LineOp::{Continue, Error, Next};
        use RecordOp::{Clear, Clearall, NoRecord, Record};
        let cases: &[(&str, LineOp, RecordOp, Option<&str>)] = &[
            ("", Next, NoRecord, None),
            (" Next", Next, NoRecord, None),
            (" Continue", Continue, NoRecord, None),
            (" Error", Error(None), NoRecord, None),
            (" Record", Next, Record, None),
            (" NoRecord", Next, NoRecord, None),
            (" Clear", Next, Clear, None),
            (" Clearall", Next, Clearall, None),
            (" Next.Record", Next, Record, None),
            (" Continue.Clearall", Continue, Clearall, None),
            (" Error.Clear", Error(None), Clear, None),
            (" Next.NoRecord Start", Next, NoRecord, Some("Start")),
            (" Record Start", Next, Record, Some("Start")),
            (" Start", Next, NoRecord, Some("Start")),
            (" NextState", Next, NoRecord, Some("NextState")),
            (" Recorder", Next, NoRecord, Some("Recorder")),
            (" Clearall2", Next, NoRecord, Some("Clearall2")),
            (" Error \"a b\"", Error(None), NoRecord, Some("\"a b\"")),
            (" Error word", Error(None), NoRecord, Some("word")),
            (" Error.Record \"x\"", Error(None), Record, Some("\"x\"")),
            (
                " Error \"a\" \"b\"",
                Error(None),
                NoRecord,
                Some("\"a\" \"b\""),
            ),
            ("   Next.Clear   Start", Next, Clear, Some("Start")),
            ("\tRecord\tStart", Next, Record, Some("Start")),
            (" Next\u{a0}Start", Next, NoRecord, Some("Start")),
            (" \u{e9}tat", Next, NoRecord, Some("\u{e9}tat")),
            (" \"a b\"", Next, NoRecord, Some("\"a b\"")),
            (" Record \"a b\"", Next, Record, Some("\"a b\"")),
            (" Next \"\"", Next, NoRecord, Some("\"\"")),
            (" Continue Start", Continue, NoRecord, Some("Start")),
        ];
        for (text, line_op, record_op, target) in cases {
            let expected = Action {
                line_op: line_op.clone(),
                record_op: *record_op,
                target: *target,
            };
            assert_eq!(parse_action(text), Some(expected), "{text:?}");
        }
        for text in [
            "Record",
            " Next.Save",
            " Record.Next",
            " Next.",
            " .Record",
            " Next Start Start",
            " Clear.Record",
            " \"a",
            " \"",
            " a-b",
            " Next \"a\" b",
            " Error a b",
            " Next.Record.Clear",
            "  ",
        ] {
            assert_eq!(parse_action(text), None, "{text:?}");
        }
    }

    #[test]
    fn substitutes_values_like_string_template() {
        let values = [
            Declaration {
                spec: ValueSpec {
                    name: "A".to_owned(),
                    options: Vec::new(),
                },
                capture: "(?P<A>a)".to_owned(),
            },
            Declaration {
                spec: ValueSpec {
                    name: "A_1".to_owned(),
                    options: Vec::new(),
                },
                capture: "(?P<A_1>b)".to_owned(),
            },
        ];
        let substitute = |pattern| substitute(pattern, &values);
        assert_eq!(
            substitute("^${A} $A$$").as_deref(),
            Ok("^(?P<A>a) (?P<A>a)$")
        );
        assert_eq!(
            substitute("$A_1-${A_1}x").as_deref(),
            Ok("(?P<A_1>b)-(?P<A_1>b)x")
        );
        assert_eq!(substitute("\\$$ $$$$").as_deref(), Ok("\\$ $$"));
        assert_eq!(substitute("plain").as_deref(), Ok("plain"));
        // The name runs as far as it can.
        assert!(
            substitute("$A_12")
                .unwrap_err()
                .contains("unknown value 'A_12'")
        );
        assert!(
            substitute("${B}")
                .unwrap_err()
                .contains("unknown value 'B'")
        );
        for invalid in ["a$", "$ ", "${A", "${}", "${A-1}", "$1", "$(", "$\u{e9}"] {
            assert!(
                substitute(invalid).unwrap_err().starts_with("invalid '$'"),
                "{invalid:?}"
            );
        }
    }

    #[test]
    fn reports_the_line_of_an_error() {
        let (line, message) = error("Value A (\\S+)\nValue B (\\d+\n\nStart\n");
        assert_eq!(line, 2);
        assert!(message.contains("must be enclosed"), "{message}");

        let (line, message) = error("Value A (\\S+)\n\nStart\n  ^${A}\n  ^(${A} -> Record\n");
        assert_eq!(line, 5);
        assert!(
            message.starts_with("invalid regular expression '^((?P<A>\\S+)': missing ), unterminated subpattern at position 1"),
            "{message}"
        );

        let (line, message) = error("Value A (\\S+)\n\nStart\n  ^${A} -> Record Other\n");
        assert_eq!(
            (line, message.as_str()),
            (4, "state 'Other' not found, referenced in state 'Start'")
        );

        let (line, message) = error("Value A (\\S+)\n\nStart\n  ^a -> Next.Save\n");
        assert_eq!(
            (line, message.as_str()),
            (4, "badly formatted rule '^a -> Next.Save'")
        );

        // Errors about the template as a whole point at its last line.
        assert_eq!(
            error("Value A (\\S+)\n\nBegin\n  ^a\n"),
            (4, "missing state 'Start'".to_owned())
        );
        assert_eq!(error(""), (1, "missing state 'Start'".to_owned()));
        assert_eq!(
            error("Value A (a)\n\nEOF\n  ^a\n\nStart\n\nEnd\n  ^b\n"),
            (8, "the 'End' state must be empty".to_owned())
        );
        let displayed = Template::parse("Start\n").unwrap_err().to_string();
        assert_eq!(displayed, "template line 1: no Value definitions found");
    }
}
