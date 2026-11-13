//! Answering one rule from one configuration.
//!
//! Everything here works on the configuration **as stored**, not on its
//! redacted form: a rule such as "no SNMP community called `public`" could
//! never fire against text in which the community has already been replaced
//! by a placeholder. Only the evidence that leaves this crate is redacted
//! (see `super::run`), and because redaction keeps a configuration's line
//! count, the line numbers quoted here point at the same lines either way.
//!
//! Lines are compared with runs of blanks collapsed, so the spacing a person
//! typed in a rule does not matter. A `must` / `must_not` entry is treated as
//! a literal line *and* as a regular expression: whichever matches counts.

use nocturne_app::{AppError, AppResult};
use nocturne_drivers::hier::Syntax;
use regex::Regex;

use super::model::{Check, EvidenceLine, Status};
use crate::outline::{self, Outline};

/// The verdict of one rule on one configuration, before it is dressed up as a
/// [`Finding`](super::model::Finding).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    pub status: Status,
    /// Lines of the configuration that justify it, with their line numbers.
    pub evidence: Vec<EvidenceLine>,
    /// What the configuration should have and has not.
    pub missing: Vec<String>,
    pub detail: String,
    /// Section headers that a section rule found wanting.
    pub sections: Vec<String>,
}

impl Verdict {
    fn new(status: Status, detail: impl Into<String>) -> Verdict {
        Verdict {
            status,
            detail: detail.into(),
            ..Verdict::default()
        }
    }

    fn with_evidence(mut self, evidence: Vec<EvidenceLine>) -> Verdict {
        self.evidence = evidence;
        self
    }

    fn with_missing(mut self, missing: Vec<String>) -> Verdict {
        self.missing = missing;
        self
    }
}

/// At most this many lines are quoted as evidence for one finding.
const MAX_EVIDENCE: usize = 20;

fn compile(field: &str, pattern: &str) -> AppResult<Regex> {
    Regex::new(pattern).map_err(|err| {
        AppError::invalid(format!(
            "`{field}` is not a valid regular expression: {err}"
        ))
    })
}

/// A `must` / `must_not` entry: a literal line, and the same text as a
/// pattern when it is a valid one.
struct Entry {
    source: String,
    literal: String,
    pattern: Option<Regex>,
}

impl Entry {
    fn new(source: &str) -> Entry {
        Entry {
            source: source.to_string(),
            literal: outline::collapse(source.trim()),
            pattern: Regex::new(source).ok(),
        }
    }

    fn matches(&self, line: &str) -> bool {
        line == self.literal
            || self
                .pattern
                .as_ref()
                .is_some_and(|pattern| pattern.is_match(line))
    }
}

fn entries(sources: &[String]) -> Vec<Entry> {
    sources.iter().map(|source| Entry::new(source)).collect()
}

/// A configuration ready to be asked questions.
pub struct Subject {
    outline: Outline,
    /// Every logical line as a pattern sees it: indented one space per level
    /// (on RouterOS, the command with its menu), blanks collapsed.
    display: Vec<Option<String>>,
    /// Every logical line as an exact top-level line: `None` for a line that
    /// makes no statement by itself (an indented line, a RouterOS menu).
    statement: Vec<Option<String>>,
}

impl Subject {
    pub fn parse(text: &str, syntax: Syntax) -> Subject {
        let outline = Outline::parse(text, syntax);
        let display = (0..outline.lines.len())
            .map(|index| {
                outline.display(index).map(|line| {
                    let indent = line.len() - line.trim_start().len();
                    format!("{}{}", &line[..indent], outline::collapse(line.trim()))
                })
            })
            .collect();
        let statement = (0..outline.lines.len())
            .map(|index| {
                outline
                    .statement(index)
                    .map(|line| outline::collapse(line.trim()))
            })
            .collect();
        Subject {
            outline,
            display,
            statement,
        }
    }

    pub fn syntax(&self) -> Syntax {
        self.outline.syntax
    }

    fn evidence(&self, index: usize) -> EvidenceLine {
        EvidenceLine {
            line_no: self.outline.lines[index].no,
            // The first physical line is what identifies a banner; the body is not evidence.
            line: self.outline.lines[index]
                .text
                .lines()
                .next()
                .unwrap_or_default()
                .to_string(),
        }
    }

    /// Indices of the top-level lines that are exactly `line`.
    fn exactly(&self, line: &str) -> Vec<usize> {
        let wanted = outline::collapse(line.trim());
        (0..self.outline.lines.len())
            .filter(|index| self.statement[*index].as_deref() == Some(wanted.as_str()))
            .collect()
    }

    /// Indices of the lines the pattern matches.
    fn matching(&self, pattern: &Regex) -> Vec<usize> {
        (0..self.outline.lines.len())
            .filter(|index| {
                self.display[*index]
                    .as_deref()
                    .is_some_and(|line| pattern.is_match(line))
            })
            .collect()
    }

    /// The lines inside the section that `header` opens, at any depth.
    fn members(&self, header: usize) -> Vec<usize> {
        match self.outline.syntax {
            // A RouterOS menu is written as often as the export felt like it;
            // every command under any copy of it belongs to the same section.
            Syntax::Routeros => (0..self.outline.lines.len())
                .filter(|index| {
                    self.outline.lines[*index].parent.is_some_and(|parent| {
                        self.outline.lines[parent].text == self.outline.lines[header].text
                    })
                })
                .collect(),
            _ => self.outline.descendants(header).collect(),
        }
    }

    fn member_text(&self, index: usize) -> String {
        outline::collapse(self.outline.lines[index].text.trim())
    }
}

/// Section headers, with the indices of the lines inside them.
struct Sections {
    /// `(index of the header, header text)`.
    headers: Vec<(usize, String)>,
}

fn sections(subject: &Subject, parent: &Regex) -> Option<Sections> {
    let all = subject.outline.sections()?;
    let mut headers: Vec<(usize, String)> = Vec::new();
    for section in all {
        let header = outline::collapse(section.header);
        let known = headers.iter().any(|(_, seen)| *seen == header);
        if parent.is_match(&header) && !known {
            headers.push((section.index, header));
        }
    }
    Some(Sections { headers })
}

/// What one section lacks.
#[derive(Default)]
struct SectionResult {
    missing: Vec<String>,
    /// Lines that are there and must not be.
    offending: Vec<usize>,
}

impl SectionResult {
    fn ok(&self) -> bool {
        self.missing.is_empty() && self.offending.is_empty()
    }
}

fn judge_section(
    subject: &Subject,
    header: usize,
    must: &[Entry],
    must_not: &[Entry],
) -> SectionResult {
    let members = subject.members(header);
    let lines: Vec<String> = members
        .iter()
        .map(|index| subject.member_text(*index))
        .collect();
    let mut result = SectionResult::default();
    for entry in must {
        if !lines.iter().any(|line| entry.matches(line)) {
            result.missing.push(entry.source.clone());
        }
    }
    for entry in must_not {
        for (position, line) in lines.iter().enumerate() {
            if entry.matches(line) {
                result.offending.push(members[position]);
            }
        }
    }
    result
}

/// Join section headers for a sentence.
fn listed(headers: &[String]) -> String {
    let shown: Vec<String> = headers
        .iter()
        .take(4)
        .map(|header| format!("`{header}`"))
        .collect();
    match headers.len() {
        0 => String::new(),
        1..=4 => shown.join(", "),
        more => format!("{} and {} more", shown.join(", "), more - 4),
    }
}

fn section_verdict(
    subject: &Subject,
    sections: Sections,
    must: &[String],
    must_not: &[String],
    each: bool,
    what: &str,
) -> Verdict {
    let must = entries(must);
    let must_not = entries(must_not);
    let judged: Vec<(usize, String, SectionResult)> = sections
        .headers
        .into_iter()
        .map(|(index, header)| {
            let result = judge_section(subject, index, &must, &must_not);
            (index, header, result)
        })
        .collect();

    let failing: Vec<&(usize, String, SectionResult)> =
        judged.iter().filter(|(.., result)| !result.ok()).collect();
    let passing: Vec<&(usize, String, SectionResult)> =
        judged.iter().filter(|(.., result)| result.ok()).collect();

    let satisfied = if each {
        failing.is_empty()
    } else {
        !passing.is_empty()
    };
    if satisfied {
        let evidence: Vec<EvidenceLine> = passing
            .iter()
            .take(MAX_EVIDENCE)
            .map(|(index, ..)| subject.evidence(*index))
            .collect();
        return Verdict::new(
            Status::Pass,
            format!(
                "{} of the {} {what} satisfies the rule",
                if each { "every one" } else { "at least one" },
                judged.len(),
            ),
        )
        .with_evidence(evidence);
    }

    let blamed: Vec<&(usize, String, SectionResult)> = if each {
        failing
    } else {
        judged.iter().collect()
    };
    let mut missing: Vec<String> = Vec::new();
    let mut evidence: Vec<EvidenceLine> = Vec::new();
    for (index, _, result) in &blamed {
        evidence.push(subject.evidence(*index));
        for line in &result.offending {
            evidence.push(subject.evidence(*line));
        }
        for entry in &result.missing {
            if !missing.contains(entry) {
                missing.push(entry.clone());
            }
        }
    }
    evidence.truncate(MAX_EVIDENCE);
    let headers: Vec<String> = blamed.iter().map(|(_, header, _)| header.clone()).collect();
    let detail = if each {
        format!(
            "{} of {} {what} {} not satisfy the rule: {}",
            headers.len(),
            judged.len(),
            if headers.len() == 1 { "does" } else { "do" },
            listed(&headers)
        )
    } else {
        format!(
            "none of the {} {what} satisfies the rule: {}",
            judged.len(),
            listed(&headers)
        )
    };
    Verdict {
        status: Status::Fail,
        evidence,
        missing,
        detail,
        sections: headers,
    }
}

/// Evaluate one check against one configuration.
pub fn evaluate(check: &Check, subject: &Subject) -> AppResult<Verdict> {
    Ok(match check {
        Check::Present { line } => match subject.exactly(line).first() {
            Some(index) => Verdict::new(Status::Pass, format!("`{}` is configured", line.trim()))
                .with_evidence(vec![subject.evidence(*index)]),
            None => Verdict::new(
                Status::Fail,
                format!("`{}` is not in the configuration", line.trim()),
            )
            .with_missing(vec![line.trim().to_string()]),
        },
        Check::Absent { line } => {
            let found = subject.exactly(line);
            match found.first() {
                Some(index) => Verdict::new(
                    Status::Fail,
                    format!("`{}` is configured and should not be", line.trim()),
                )
                .with_evidence(vec![subject.evidence(*index)]),
                None => Verdict::new(
                    Status::Pass,
                    format!("`{}` is not in the configuration", line.trim()),
                ),
            }
        }
        Check::Matches { pattern } => {
            let regex = compile("pattern", pattern)?;
            let found = subject.matching(&regex);
            match found.first() {
                Some(index) => Verdict::new(
                    Status::Pass,
                    format!("a line matching `{pattern}` is configured"),
                )
                .with_evidence(vec![subject.evidence(*index)]),
                None => Verdict::new(
                    Status::Fail,
                    format!("no line of the configuration matches `{pattern}`"),
                )
                .with_missing(vec![pattern.clone()]),
            }
        }
        Check::NotMatches { pattern } => {
            let regex = compile("pattern", pattern)?;
            let found = subject.matching(&regex);
            if found.is_empty() {
                Verdict::new(
                    Status::Pass,
                    format!("no line of the configuration matches `{pattern}`"),
                )
            } else {
                Verdict::new(
                    Status::Fail,
                    format!(
                        "{} line{} match{} `{pattern}` and should not",
                        found.len(),
                        if found.len() == 1 { "" } else { "s" },
                        if found.len() == 1 { "es" } else { "" }
                    ),
                )
                .with_evidence(
                    found
                        .iter()
                        .take(MAX_EVIDENCE)
                        .map(|index| subject.evidence(*index))
                        .collect(),
                )
            }
        }
        Check::Section {
            parent,
            each,
            must,
            must_not,
        } => {
            let regex = compile("parent", parent)?;
            let Some(found) = sections(subject, &regex) else {
                return Ok(Verdict::new(
                    Status::NotApplicable,
                    "the configuration of this device type has no sections to look in",
                ));
            };
            if found.headers.is_empty() {
                return Ok(Verdict::new(
                    Status::NotApplicable,
                    format!("the configuration has no section matching `{parent}`"),
                ));
            }
            section_verdict(
                subject,
                found,
                must,
                must_not,
                *each,
                &format!("sections matching `{parent}`"),
            )
        }
        Check::SectionWhen {
            parent,
            when,
            must,
            must_not,
        } => {
            let regex = compile("parent", parent)?;
            let condition = compile("when", when)?;
            let Some(found) = sections(subject, &regex) else {
                return Ok(Verdict::new(
                    Status::NotApplicable,
                    "the configuration of this device type has no sections to look in",
                ));
            };
            let selected: Vec<(usize, String)> = found
                .headers
                .into_iter()
                .filter(|(index, _)| {
                    subject
                        .members(*index)
                        .iter()
                        .any(|member| condition.is_match(&subject.member_text(*member)))
                })
                .collect();
            if selected.is_empty() {
                return Ok(Verdict::new(
                    Status::NotApplicable,
                    format!("no section matching `{parent}` contains a line matching `{when}`"),
                ));
            }
            section_verdict(
                subject,
                Sections { headers: selected },
                must,
                must_not,
                true,
                &format!("sections matching `{parent}` that contain `{when}`"),
            )
        }
        Check::Snippet { config } => {
            let wanted = Outline::parse(config, subject.syntax()).tree();
            if wanted.is_empty() {
                return Err(AppError::invalid(
                    "the snippet holds no configuration lines",
                ));
            }
            let missing =
                outline::render_lines(&outline::missing(&wanted, &subject.outline.tree()));
            if missing.is_empty() {
                Verdict::new(
                    Status::Pass,
                    "every line of the snippet is in the configuration",
                )
            } else {
                let lines = missing.len();
                Verdict::new(
                    Status::Fail,
                    format!(
                        "{lines} line{} of the snippet {} missing",
                        if lines == 1 { "" } else { "s" },
                        if lines == 1 { "is" } else { "are" }
                    ),
                )
                .with_missing(missing)
            }
        }
    })
}
