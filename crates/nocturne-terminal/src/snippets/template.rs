//! The variables of a snippet: `{{name}}` and `{{name=default}}`.
//!
//! This is substitution and nothing else: a value takes the place of its
//! placeholder, once, on the line the placeholder is on. No loops, no
//! conditions, no expressions — that is what Automation's templates are for.
//! The whole point of keeping it this small is that the text a person
//! confirms is the text that is typed: a value cannot add a line, cannot
//! hold a keystroke that is not a character, and cannot be read as another
//! placeholder.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;
use sha2::Digest as _;

/// Most variables one snippet has. A form with more fields than this is a
/// template for Automation, not something to fill in before a paste.
pub(crate) const MAX_VARIABLES: usize = 32;
/// Longest name of a variable, in characters.
pub(crate) const MAX_VARIABLE_NAME: usize = 64;
/// Longest value or default, in characters: an interface name, a
/// description, an address — never a block of text.
pub(crate) const MAX_VALUE: usize = 1024;
/// Longest rendered text, in bytes: what a stored snippet may be.
pub(crate) const MAX_RENDERED: usize = 256 * 1024;

/// A variable of a snippet, as `terminal.snippets.list` reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Variable {
    pub name: String,
    /// What it is when no value is given. Absent: a value is required.
    /// Present and empty: it may be left out, and is then nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Why a text is not a snippet, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Malformed {
    /// Counted from 1, in lines of the text.
    pub line: usize,
    /// Counted from 1, in characters of that line.
    pub column: usize,
    pub problem: String,
}

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "line {}, column {}: {}",
            self.line, self.column, self.problem
        )
    }
}

enum Piece {
    Text(String),
    /// An index into the variables.
    Variable(usize),
}

/// A snippet's text, taken apart into what is typed as it stands and where
/// a value goes.
pub(crate) struct Template {
    pieces: Vec<Piece>,
    pub variables: Vec<Variable>,
}

/// What is wrong with the value of one variable. It names the variable and
/// the kind of fault and never repeats the value: this ends up in an error
/// message, and error messages are logged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ValueProblem {
    pub name: String,
    /// `missing`, `unknown`, `not_text`, `line_break`, `control_character`
    /// or `too_long`.
    pub problem: &'static str,
    #[serde(skip)]
    pub says: String,
}

pub(crate) enum RenderError {
    Values(Vec<ValueProblem>),
    /// The rendered text would be this many bytes.
    TooLong(usize),
}

/// The text as it will be typed, with every line break as `\n`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Rendered {
    pub text: String,
    pub lines: usize,
}

impl Rendered {
    /// SHA-256 of the text, in hex: what a person confirmed, pinned.
    pub fn sha256(&self) -> String {
        sha2::Sha256::digest(self.text.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

/// Every line ending as one `\n`: CRLF, a bare carriage return and LF.
pub(crate) fn normalise(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// The text as a keyboard would have sent it: one carriage return per line
/// break, because that is what the Enter key is. A line feed after it is a
/// second Enter to most devices, and answers whatever the first one asked.
pub(crate) fn as_typed(text: &str) -> String {
    text.replace('\n', "\r")
}

const PASTE_START: &str = "\u{1b}[200~";
const PASTE_END: &str = "\u{1b}[201~";

/// The text between the markers a program asks for with bracketed paste: a
/// shell then holds the lines instead of running each one. The end marker is
/// taken out of the text first, until none is left, so that the text cannot
/// close the bracket itself and have its remainder run as if typed.
pub(crate) fn bracketed(typed: &str) -> String {
    let mut inside = typed.to_string();
    while inside.contains(PASTE_END) {
        inside = inside.replace(PASTE_END, "");
    }
    format!("{PASTE_START}{inside}{PASTE_END}")
}

/// A character that a confirmation cannot show for what it is, and what to
/// call it.
///
/// A line break is another command. A control character is a keystroke —
/// Ctrl+C, Escape, a backspace that takes back what the sheet showed. And a
/// direction mark makes a line read one way and be typed another.
pub(crate) fn unshowable(ch: char) -> Option<(&'static str, String)> {
    match ch {
        '\n' | '\r' => Some(("line_break", "a line break".to_string())),
        // Not a line break to a device, but drawn as one by whatever shows
        // the text: the confirmation would have a line more than the paste.
        '\u{2028}' | '\u{2029}' => Some((
            "line_break",
            format!("a line separator (U+{:04X})", ch as u32),
        )),
        _ if ch.is_control() => Some((
            "control_character",
            format!("a control character (U+{:04X})", ch as u32),
        )),
        '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => Some((
            "control_character",
            format!("a text-direction mark (U+{:04X})", ch as u32),
        )),
        _ => None,
    }
}

/// The first character of `text` that cannot be shown, if any.
pub(crate) fn first_unshowable(text: &str) -> Option<(&'static str, String)> {
    text.chars().find_map(unshowable)
}

fn is_name(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

const HOW: &str =
    "write `{{name}}` or `{{name=default}}`, or `\\{{` to have two braces typed as they are";

impl Template {
    /// Take a snippet's text apart, or say what is wrong with it and where.
    pub fn parse(text: &str) -> Result<Template, Malformed> {
        let text = normalise(text);
        let mut pieces = Vec::new();
        let mut variables: Vec<Variable> = Vec::new();
        // Where each variable's default was given, for the message when
        // another placeholder gives a different one.
        let mut default_at: Vec<Option<(usize, usize)>> = Vec::new();
        let mut literal = String::new();
        let (mut line, mut column) = (1, 1);
        let mut at = 0;
        // Where the line being read ends. Found once per line, not once per
        // placeholder: a line of fifty thousand `{{a}}` would otherwise be
        // searched to its end fifty thousand times, on every list.
        let line_end_from = |from: usize| text[from..].find('\n').map_or(text.len(), |n| from + n);
        let mut line_end = line_end_from(0);

        while let Some(ch) = text[at..].chars().next() {
            let rest = &text[at..];
            let here = |problem: String| Malformed {
                line,
                column,
                problem,
            };
            if ch == '\n' {
                literal.push(ch);
                line += 1;
                column = 1;
                at += 1;
                line_end = line_end_from(at);
                continue;
            }
            if ch != '\t'
                && let Some((_, what)) = unshowable(ch)
            {
                return Err(here(format!(
                    "{what} cannot be part of a snippet: it would be typed without being shown"
                )));
            }
            // A backslash is only special right in front of `{{`. One makes
            // the braces literal; two are a backslash of the text's own (a
            // Windows path) in front of a real variable.
            if rest.starts_with("\\\\{{") {
                literal.push('\\');
                column += 2;
                at += 2;
                continue;
            }
            if rest.starts_with("\\{{") {
                literal.push_str("{{");
                column += 3;
                at += 3;
                continue;
            }
            if !rest.starts_with("{{") {
                literal.push(ch);
                column += 1;
                at += ch.len_utf8();
                continue;
            }

            let this_line = &text[at..line_end];
            let Some(close) = this_line.find("}}") else {
                return Err(here(format!(
                    "`{{{{` opens a variable that is not closed on this line; {HOW}"
                )));
            };
            let inner = &rest[2..close];
            let (name, default) = match inner.split_once('=') {
                Some((name, default)) => (name, Some(default)),
                None => (inner, None),
            };
            if !is_name(name) {
                return Err(here(format!(
                    "the name of a variable is letters, digits and `_`, with nothing around it; {HOW}"
                )));
            }
            if name.chars().count() > MAX_VARIABLE_NAME {
                return Err(here(format!(
                    "the name of a variable is at most {MAX_VARIABLE_NAME} characters"
                )));
            }
            if let Some(default) = default {
                if let Some((_, what)) = first_unshowable(default) {
                    return Err(here(format!("the default of `{name}` contains {what}")));
                }
                if default.contains("{{") {
                    return Err(here(format!(
                        "the default of `{name}` contains `{{{{`: a default is plain text, not another variable"
                    )));
                }
                if default.chars().count() > MAX_VALUE {
                    return Err(here(format!(
                        "the default of `{name}` is longer than {MAX_VALUE} characters"
                    )));
                }
            }
            let index = match variables.iter().position(|known| known.name == name) {
                Some(index) => {
                    match (&variables[index].default, default, default_at[index]) {
                        (Some(first), Some(second), Some((at_line, at_column)))
                            if first != second =>
                        {
                            return Err(here(format!(
                                "`{name}` is given a different default here than at line {at_line}, column {at_column}; a variable has one"
                            )));
                        }
                        (None, Some(default), _) => {
                            variables[index].default = Some(default.to_string());
                            default_at[index] = Some((line, column));
                        }
                        _ => {}
                    }
                    index
                }
                None => {
                    if variables.len() >= MAX_VARIABLES {
                        return Err(here(format!(
                            "a snippet has at most {MAX_VARIABLES} variables; this is one more"
                        )));
                    }
                    variables.push(Variable {
                        name: name.to_string(),
                        default: default.map(str::to_string),
                    });
                    default_at.push(default.map(|_| (line, column)));
                    variables.len() - 1
                }
            };
            if !literal.is_empty() {
                pieces.push(Piece::Text(std::mem::take(&mut literal)));
            }
            pieces.push(Piece::Variable(index));
            column += rest[..close + 2].chars().count();
            at += close + 2;
        }
        if !literal.is_empty() {
            pieces.push(Piece::Text(literal));
        }
        Ok(Template { pieces, variables })
    }

    /// Put `values` in. Strict: every variable needs a value or a default,
    /// every value needs a variable, and a value is one line of plain text.
    ///
    /// The values go in exactly once: a value that looks like a placeholder
    /// is typed as it is, never filled in again.
    pub fn render(&self, values: &BTreeMap<String, Value>) -> Result<Rendered, RenderError> {
        let mut problems = Vec::new();
        let mut chosen: Vec<&str> = Vec::with_capacity(self.variables.len());
        for variable in &self.variables {
            let name = &variable.name;
            let mut fault = |problem: &'static str, says: String| {
                problems.push(ValueProblem {
                    name: name.clone(),
                    problem,
                    says,
                });
            };
            let value = match (values.get(name), &variable.default) {
                (Some(Value::String(value)), _) => value.as_str(),
                (Some(_), _) => {
                    fault("not_text", format!("the value of `{name}` is not text"));
                    ""
                }
                (None, Some(default)) => default.as_str(),
                (None, None) => {
                    fault("missing", format!("`{name}` has no value and no default"));
                    ""
                }
            };
            if let Some((problem, what)) = first_unshowable(value) {
                fault(problem, format!("the value of `{name}` contains {what}"));
            } else if value.chars().count() > MAX_VALUE {
                fault(
                    "too_long",
                    format!("the value of `{name}` is longer than {MAX_VALUE} characters"),
                );
            }
            chosen.push(value);
        }
        for name in values.keys() {
            if !self.variables.iter().any(|variable| &variable.name == name) {
                // The name came from the caller: say it in a form that can
                // be read, however long or strange it was.
                let shown: String = name
                    .chars()
                    .take(MAX_VARIABLE_NAME)
                    .map(|ch| {
                        if unshowable(ch).is_some() {
                            '\u{fffd}'
                        } else {
                            ch
                        }
                    })
                    .collect();
                problems.push(ValueProblem {
                    says: format!("`{shown}` is not a variable of this snippet"),
                    name: shown,
                    problem: "unknown",
                });
            }
        }
        if !problems.is_empty() {
            return Err(RenderError::Values(problems));
        }

        fn content<'a>(piece: &'a Piece, chosen: &[&'a str]) -> &'a str {
            match piece {
                Piece::Text(literal) => literal,
                Piece::Variable(index) => chosen[*index],
            }
        }
        let content = |piece| content(piece, &chosen);
        // Never an Enter of ours: the line break that ends a snippet is not
        // typed. Only that one — a blank line someone left above it is theirs.
        let ends_in_break = self
            .pieces
            .iter()
            .rev()
            .map(content)
            .find(|content| !content.is_empty())
            .is_some_and(|last| last.ends_with('\n'));
        // Measured before it is built: one variable in fifty thousand places
        // and a value of a thousand characters would be hundreds of megabytes
        // put together only to be refused.
        let bytes = self
            .pieces
            .iter()
            .map(|piece| content(piece).len())
            .sum::<usize>()
            - usize::from(ends_in_break);
        if bytes > MAX_RENDERED {
            return Err(RenderError::TooLong(bytes));
        }
        let mut text = String::with_capacity(bytes + 1);
        for piece in &self.pieces {
            text.push_str(content(piece));
        }
        if ends_in_break {
            text.pop();
        }
        let lines = if text.is_empty() {
            0
        } else {
            text.split('\n').count()
        };
        Ok(Rendered { text, lines })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn variables(text: &str) -> Vec<(String, Option<String>)> {
        Template::parse(text)
            .expect("a snippet")
            .variables
            .into_iter()
            .map(|variable| (variable.name, variable.default))
            .collect()
    }

    fn malformed(text: &str) -> Malformed {
        match Template::parse(text) {
            Ok(_) => panic!("{text:?} was taken for a snippet"),
            Err(malformed) => malformed,
        }
    }

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), json!(value)))
            .collect()
    }

    fn rendered(text: &str, pairs: &[(&str, &str)]) -> String {
        match Template::parse(text)
            .expect("a snippet")
            .render(&values(pairs))
        {
            Ok(rendered) => rendered.text,
            Err(RenderError::Values(problems)) => panic!("{problems:?}"),
            Err(RenderError::TooLong(bytes)) => panic!("{bytes} bytes"),
        }
    }

    fn refused(text: &str, given: &BTreeMap<String, Value>) -> Vec<(String, &'static str)> {
        match Template::parse(text).expect("a snippet").render(given) {
            Err(RenderError::Values(problems)) => problems
                .into_iter()
                .map(|problem| (problem.name, problem.problem))
                .collect(),
            Err(RenderError::TooLong(bytes)) => panic!("{bytes} bytes"),
            Ok(rendered) => panic!("rendered as {:?}", rendered.text),
        }
    }

    #[test]
    fn variables_are_found_in_the_order_they_first_appear_with_their_defaults() {
        assert_eq!(
            variables(
                "interface {{port}}\n switchport access vlan {{vlan=10}}\n description {{descr=}} {{port}}"
            ),
            [
                ("port".to_string(), None),
                ("vlan".to_string(), Some("10".to_string())),
                ("descr".to_string(), Some(String::new())),
            ]
        );
        assert_eq!(variables("show clock"), []);
        // A default given once holds for every place the variable stands in.
        assert_eq!(
            variables("{{a}} {{a=x}} {{a}} {{a=x}}"),
            [("a".to_string(), Some("x".to_string()))]
        );
        // The default is everything after the first `=`, as it is written.
        assert_eq!(
            variables("{{path= a=b / c }}"),
            [("path".to_string(), Some(" a=b / c ".to_string()))]
        );
    }

    #[test]
    fn a_malformed_placeholder_is_refused_with_the_place_it_is_in() {
        for (text, line, column, says) in [
            ("show {{", 1, 6, "not closed"),
            ("a\nb {{x\n}}", 2, 3, "not closed"),
            ("{{}}", 1, 1, "letters, digits"),
            ("ping {{ host }}", 1, 6, "letters, digits"),
            ("ping {{host name}}", 1, 6, "letters, digits"),
            ("docker ps --format '{{.Names}}'", 1, 21, "letters, digits"),
            ("{{=default}}", 1, 1, "letters, digits"),
            ("vlan {{vlan-id}}", 1, 6, "letters, digits"),
            ("é {{naïve}}", 1, 3, "letters, digits"),
            ("{{a=1}}\n  {{a=2}}", 2, 3, "different default"),
            ("{{a={{b}}}}", 1, 1, "plain text"),
            ("{{a=x\ty}}", 1, 1, "control character (U+0009)"),
            ("show\u{1b}[201~ clock", 1, 5, "control character (U+001B)"),
            ("ok\nreload\u{8}\u{8}", 2, 7, "control character (U+0008)"),
            ("say \u{202e}dlrow", 1, 5, "text-direction mark (U+202E)"),
        ] {
            let malformed = malformed(text);
            assert_eq!(
                (malformed.line, malformed.column),
                (line, column),
                "{text:?}"
            );
            assert!(malformed.problem.contains(says), "{text:?}: {malformed}");
        }
        let long = format!("{{{{{}}}}}", "n".repeat(MAX_VARIABLE_NAME + 1));
        assert!(malformed(&long).problem.contains("at most 64"));
        let many: String = (0..=MAX_VARIABLES)
            .map(|n| format!("{{{{v{n}}}}} "))
            .collect();
        let too_many = malformed(&many);
        assert!(too_many.problem.contains("at most 32"), "{too_many}");
        // The message never repeats what the text said: it is logged.
        let secret = malformed("enable secret {{hunter2 hunter2}}");
        assert!(!secret.to_string().contains("hunter2"), "{secret}");
    }

    #[test]
    fn what_is_not_a_placeholder_is_typed_as_it_stands() {
        // Braces that do not open one, and the escape for two that would.
        assert_eq!(rendered("a { b } c }} d {x}", &[]), "a { b } c }} d {x}");
        assert_eq!(
            rendered("docker ps --format '\\{{.Names}}'", &[]),
            "docker ps --format '{{.Names}}'"
        );
        assert_eq!(variables("\\{{name}}"), []);
        // A backslash that is the text's own stays; in front of a variable
        // it is written twice, so that it is not taken for the escape.
        assert_eq!(
            rendered("C:\\Users\\\\{{user}}\\x", &[("user", "ops")]),
            "C:\\Users\\ops\\x"
        );
        assert_eq!(variables("C:\\Users\\{{user}}"), []);
        // Tabs are indentation, and blank lines are the person's.
        assert_eq!(rendered("a\n\tb\n\nc", &[]), "a\n\tb\n\nc");
    }

    #[test]
    fn values_take_the_place_of_their_placeholders_once() {
        let text =
            "interface {{port}}\n description {{descr=uplink}}\n switchport access vlan {{vlan}}\n";
        assert_eq!(
            rendered(text, &[("port", "Gi0/2"), ("vlan", "20")]),
            "interface Gi0/2\n description uplink\n switchport access vlan 20"
        );
        // A given value wins over the default; an empty one is a value too.
        assert_eq!(
            rendered(text, &[("port", "Gi0/2"), ("vlan", "20"), ("descr", "")]),
            "interface Gi0/2\n description \n switchport access vlan 20"
        );
        // A value that looks like a placeholder, or like the escape, is text.
        assert_eq!(
            rendered("say {{a}} {{b}}", &[("a", "{{b}}"), ("b", "\\{{a}}")]),
            "say {{b}} \\{{a}}"
        );
    }

    #[test]
    fn line_endings_become_one_kind_and_only_the_last_one_is_dropped() {
        let parsed = Template::parse("one\r\ntwo\rthree\n").expect("a snippet");
        let rendered = parsed.render(&BTreeMap::new()).ok().expect("rendered");
        assert_eq!(rendered.text, "one\ntwo\nthree");
        assert_eq!(rendered.lines, 3);
        assert_eq!(as_typed(&rendered.text), "one\rtwo\rthree");
        // A blank line left at the end is an Enter the text contains.
        let counted = |text: &str| {
            let rendered = Template::parse(text)
                .expect("a snippet")
                .render(&BTreeMap::new())
                .ok()
                .expect("rendered");
            (rendered.text, rendered.lines)
        };
        assert_eq!(counted("show clock\n\n"), ("show clock\n".to_string(), 2));
        assert_eq!(counted("show clock"), ("show clock".to_string(), 1));
        // Nothing at all is no lines, not one empty one.
        assert_eq!(counted("{{nothing=}}"), (String::new(), 0));
    }

    #[test]
    fn rendering_is_strict_about_every_value() {
        let text = "interface {{port}}\n description {{descr=uplink}}";
        assert_eq!(
            refused(text, &values(&[])),
            [("port".to_string(), "missing")]
        );
        assert_eq!(
            refused(text, &values(&[("port", "Gi0/1"), ("vlan", "10")])),
            [("vlan".to_string(), "unknown")]
        );
        for (value, problem) in [
            ("Gi0/1\nreload", "line_break"),
            ("Gi0/1\rreload", "line_break"),
            ("Gi0/1\r\nreload", "line_break"),
            ("Gi0/1\u{2028}reload", "line_break"),
            ("Gi0/1\u{3}", "control_character"),
            ("Gi0/1\u{1b}[201~", "control_character"),
            ("Gi0/1\t", "control_character"),
            ("Gi0/1\u{0}", "control_character"),
            ("Gi0/1\u{7f}", "control_character"),
            ("Gi0/1\u{85}", "control_character"),
            ("\u{202e}1/0iG", "control_character"),
        ] {
            assert_eq!(
                refused(text, &values(&[("port", value)])),
                [("port".to_string(), problem)],
                "{value:?}"
            );
        }
        let long = "x".repeat(MAX_VALUE + 1);
        assert_eq!(
            refused(text, &values(&[("port", &long)])),
            [("port".to_string(), "too_long")]
        );
        // Only text is a value: a number is not quietly turned into one.
        let mut given = values(&[]);
        given.insert("port".into(), json!(12));
        assert_eq!(refused(text, &given), [("port".to_string(), "not_text")]);
        // Everything that is wrong is said at once, variables first.
        let mut given = values(&[("descr", "a\nb"), ("zzz", "1")]);
        given.insert("aaa".into(), json!(null));
        assert_eq!(
            refused(text, &given),
            [
                ("port".to_string(), "missing"),
                ("descr".to_string(), "line_break"),
                ("aaa".to_string(), "unknown"),
                ("zzz".to_string(), "unknown"),
            ]
        );
    }

    #[test]
    fn a_fault_names_the_variable_and_never_repeats_the_value() {
        let parsed = Template::parse("enable secret {{secret}}").expect("a snippet");
        let Err(RenderError::Values(problems)) =
            parsed.render(&values(&[("secret", "hunter2\nhunter2")]))
        else {
            panic!("a value with a line break was accepted");
        };
        assert_eq!(
            problems[0].says,
            "the value of `secret` contains a line break"
        );
        // The name of a value nobody asked for is the caller's text too.
        let Err(RenderError::Values(problems)) =
            parsed.render(&values(&[("secret", "x"), ("a\nb\u{1b}", "y")]))
        else {
            panic!("an unknown name was accepted");
        };
        assert_eq!(problems[0].name, "a\u{fffd}b\u{fffd}");
    }

    #[test]
    fn the_hash_is_of_the_text_that_is_shown() {
        let rendered = Rendered {
            text: "show clock".into(),
            lines: 1,
        };
        // `printf 'show clock' | sha256sum`
        assert_eq!(
            rendered.sha256(),
            "cb75924da5e7bb871e288717e54dc1eb5065c55e8ab1a974be3319cc7f4cd590"
        );
        let other = Rendered {
            text: "show clock ".into(),
            lines: 1,
        };
        assert_ne!(rendered.sha256(), other.sha256());
    }

    #[test]
    fn a_bracketed_paste_cannot_be_closed_by_its_own_text() {
        assert_eq!(bracketed("ls\rpwd"), "\u{1b}[200~ls\rpwd\u{1b}[201~");
        assert_eq!(
            bracketed("a\u{1b}[201~b\u{1b}[20\u{1b}[201~1~c"),
            "\u{1b}[200~abc\u{1b}[201~"
        );
    }

    #[test]
    fn a_text_too_long_once_filled_in_is_refused_by_its_size_as_typed() {
        // Sixteen bytes a line with a value of ten: exactly as long as a
        // rendered text may be, the last line break not counted.
        let lines = MAX_RENDERED / 16;
        let text = "abcde{{v}}\n".repeat(lines);
        let parsed = Template::parse(&text).expect("a snippet");
        let fits = parsed
            .render(&values(&[("v", "0123456789")]))
            .ok()
            .expect("it fits");
        assert_eq!(fits.text.len(), MAX_RENDERED - 1);
        assert_eq!(fits.lines, lines);
        // One character more in each place and it does not; the size said
        // is the size it would have been typed at.
        match parsed.render(&values(&[("v", "0123456789a")])) {
            Err(RenderError::TooLong(bytes)) => assert_eq!(bytes, MAX_RENDERED - 1 + lines),
            Err(RenderError::Values(problems)) => panic!("{problems:?}"),
            Ok(rendered) => panic!("{} bytes were accepted", rendered.text.len()),
        }
        // The one line break that is dropped does not count against it.
        let exactly = format!("{}\n", "x".repeat(MAX_RENDERED));
        let rendered = Template::parse(&exactly)
            .expect("a snippet")
            .render(&values(&[]))
            .ok()
            .expect("it fits");
        assert_eq!(rendered.text.len(), MAX_RENDERED);
        let over = format!("{}\n\n", "x".repeat(MAX_RENDERED));
        assert!(matches!(
            Template::parse(&over).expect("a snippet").render(&values(&[])),
            Err(RenderError::TooLong(bytes)) if bytes == MAX_RENDERED + 1
        ));
    }

    #[test]
    fn a_line_of_many_placeholders_is_read_like_any_other() {
        // One variable in every place a line of the largest snippet has room
        // for: the places are found, in order, and filled.
        let places = 256 * 1024 / 6;
        let text = "{{a}} ".repeat(places);
        let parsed = Template::parse(&text).expect("a snippet");
        assert_eq!(parsed.variables.len(), 1);
        let rendered = parsed
            .render(&values(&[("a", "x")]))
            .ok()
            .expect("rendered");
        assert_eq!(rendered.text, "x ".repeat(places));
        // A fault at the far end of such a line is still found where it is.
        let broken = format!("{text}\n{text}{{{{");
        let malformed = malformed(&broken);
        assert_eq!((malformed.line, malformed.column), (2, places * 6 + 1));
    }
}
