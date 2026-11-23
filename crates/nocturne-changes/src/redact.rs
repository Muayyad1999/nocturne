//! What text of a change may leave the engine, and in what form.
//!
//! A change is made of configuration lines, and a configuration is full of
//! secrets that are no vault credential: enable secrets, communities, routing
//! and RADIUS keys, pre-shared keys. A change that only renames a port still
//! carries, in three lines of diff context and in the detail of a failed
//! check, whatever stands next to the line it touches. So nothing a change is
//! made of leaves this crate except through this module, and it leaves in one
//! of two forms:
//!
//! * **The lines themselves** — an intent, a plan's commands, its rollback,
//!   its predicted difference, the command of a protected hit — are kept
//!   exactly as they are, because they are what is hashed, approved and sent.
//!   They are read through [`change`] and [`plan`], which put a placeholder
//!   (`<secret:3>`) where each secret value stands: one table for one answer,
//!   so the same number is the same value wherever it appears in it. The
//!   lines are returned whole only when the caller says `redact: false`, and
//!   that read is written to the audit trail (`changes.view_unredacted`).
//!
//! * **Everything said about them** — a step's or a check's detail, an error,
//!   a warning, a reason for a risk level, a job log line, an event, an audit
//!   detail, what the device answered, the session transcript — goes through
//!   a [`Hider`] as it is made, and holds no value at all: `<hidden>` stands
//!   where one was. It carries no number on purpose. This text is written
//!   down and read back later, the table that gave a number its meaning is
//!   gone by then, and a number from one table beside a number from another
//!   reads as "the same value" when it is not.
//!
//! The rules are the engine's (`nocturne_ai::Redactor`) plus the device
//! type's own `config.secrets` patterns. Over-redaction is the safe way to be
//! wrong: a word hidden by mistake costs a click, a leaked key cannot be
//! taken back.

use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock};

use nocturne_ai::Redactor;
use nocturne_app::error::AppError;
use nocturne_app::{App, inventory, profiles};
use nocturne_drivers::hier::TextDiff;
use nocturne_drivers::profile::Platform;
use nocturne_drivers::session::CommandOutput;
use parking_lot::Mutex;
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};

use crate::model::{Change, Check, Intent, Plan, ProtectedHit, Step, Target, TargetResult};

/// What stands where a value was, in text that is written down.
pub const HIDDEN: &str = "<hidden>";

/// The audit action of a read that asked for the lines whole. Named after
/// `config.view_unredacted`, which records the same thing for a stored
/// configuration.
pub const VIEW_UNREDACTED: &str = "changes.view_unredacted";

/// The engine's placeholder, with or without its `weak` mark — and with the
/// remains of the ones a careless pattern took for the start of a secret in
/// front of it (see `Hider::unnumbered`).
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:<secret:)*<secret:\d+(?::weak)?>").expect("placeholder pattern")
});

/// What a value that is already hidden reads as while the rules run.
///
/// The engine leaves anything that contains one of its own placeholders
/// alone, whatever is attached to it; `<hidden>` it only knows when it stands
/// by itself. A rule that takes "the rest of the word" would take
/// `<hidden>`,` for a new secret and swallow the punctuation with it, so
/// hiding a text twice would not give the same text. The engine numbers from
/// one, so nought is nobody's.
const SHIELD: &str = "<secret:0>";

/// How far into a transcript line a prompt can end, and how many places are
/// tried.
const PROMPT_WITHIN: usize = 96;
const PROMPT_CANDIDATES: usize = 4;

/// The engine's rules plus the given device types' own patterns.
///
/// A device type's pattern is written for one line of configuration, and
/// several say so with `^` and `$`, which as written only mean "start" and
/// "end" while the text is one line. Read that way, a configuration of twenty
/// thousand lines has to be offered to every rule once per line; measured,
/// that doubled the time an apply takes. So each pattern is compiled in
/// multi-line mode, with `\r\n` as a line end the way a session prints it:
/// one pass over a whole text reads each of its lines as the pattern was
/// written for. (The engine's own rules are multi-line already.)
fn rules<'a>(patterns: impl IntoIterator<Item = &'a String>) -> Redactor {
    let mut usable: Vec<String> = Vec::new();
    for pattern in patterns {
        let per_line = format!("(?mR){pattern}");
        // One pattern that does not compile must not switch the others off.
        if !usable.contains(&per_line) && Regex::new(&per_line).is_ok() {
            usable.push(per_line);
        }
    }
    Redactor::with_rules(&usable).unwrap_or_default()
}

fn patterns_of(app: &App, profile_ids: &[String]) -> Vec<String> {
    let registry = profiles::registry(app);
    profile_ids
        .iter()
        .filter_map(|id| registry.get(id))
        .flat_map(|platform| platform.profile.config.secrets.clone())
        .collect()
}

/// Whether a value the rules picked up may be hidden wherever else it turns
/// up. The rules are generous and also collect the odd ordinary word;
/// replacing those everywhere would shred the text they are in.
fn distinctive(value: &str) -> bool {
    if value.chars().count() < 8 {
        return false;
    }
    let kinds = [
        value.chars().any(|ch| ch.is_ascii_lowercase()),
        value.chars().any(|ch| ch.is_ascii_uppercase()),
        value.chars().any(|ch| ch.is_ascii_digit()),
        value.chars().any(|ch| !ch.is_ascii_alphanumeric()),
    ];
    kinds.into_iter().filter(|present| *present).count() >= 2
}

/// The values a redactor has recognised that may be hidden out of context,
/// and one pattern that finds any of them.
///
/// A core switch holds hundreds of distinct keys and its session log runs to
/// hundreds of kilobytes: looking for each value in turn is a pass over the
/// text per value, and was measured in seconds. The pattern is one pass.
#[derive(Clone, Default)]
struct Known {
    /// Longest first, so that a value containing another is replaced whole.
    values: Vec<String>,
    /// `None` when there is nothing to look for, or the values are too many
    /// for one pattern: then they are looked for one by one.
    any: Option<Regex>,
}

impl Known {
    fn of(redactor: &Redactor) -> Known {
        let mut values: Vec<String> = redactor
            .export()
            .entries
            .into_iter()
            .map(|(_, value)| value)
            .filter(|value| distinctive(value))
            .collect();
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        let any = if values.is_empty() {
            None
        } else {
            let alternatives: Vec<String> =
                values.iter().map(|value| regex::escape(value)).collect();
            Regex::new(&alternatives.join("|")).ok()
        };
        Known { values, any }
    }

    /// The text with `stand_in` where each known value stood.
    fn replaced(&self, text: &str, stand_in: &str) -> String {
        match &self.any {
            Some(any) if any.is_match(text) => any.replace_all(text, stand_in).into_owned(),
            Some(_) => text.to_string(),
            None => {
                let mut out = text.to_string();
                for value in &self.values {
                    if out.contains(value.as_str()) {
                        out = out.replace(value.as_str(), stand_in);
                    }
                }
                out
            }
        }
    }

    /// The known values that stand in `text`, longest first.
    fn found_in(&self, text: &str) -> Vec<&str> {
        match &self.any {
            Some(any) => {
                let found: BTreeSet<&str> = any.find_iter(text).map(|at| at.as_str()).collect();
                self.values
                    .iter()
                    .map(String::as_str)
                    .filter(|value| found.contains(value))
                    .collect()
            }
            None => self
                .values
                .iter()
                .map(String::as_str)
                .filter(|value| text.contains(value))
                .collect(),
        }
    }
}

/// Every rule over a text of any length.
///
/// One pass reads each of its lines (the device type's patterns are
/// multi-line, see `rules`) and a private key, which spans lines, whole. But a
/// quote left open on its line lets a rule that reads to the closing quote go
/// on into the next lines, past the values standing there; a text that has
/// one is first read a line at a time. (A configuration rarely does: devices
/// close their quotes, so this costs nothing where the text is large.)
fn ruled(redactor: &Redactor, text: &str) -> String {
    let open_quote = text.contains('\n')
        && text
            .split('\n')
            .any(|line| line.matches('"').count() % 2 == 1);
    if !open_quote {
        return redactor.redact(text);
    }
    let lines: Vec<String> = text.split('\n').map(|line| redactor.redact(line)).collect();
    redactor.redact(&lines.join("\n"))
}

/// Every rule over a line, or over a path of lines (`line vty 0 4 /
/// password …`).
///
/// Read whole first and then part by part. Whole, because a path of a flat
/// configuration (RouterOS `/export terse`, Junos `set`, a file of
/// `key=value`) is one line, and a ` / ` in it stands inside a quoted value
/// (`comment="core / edge"`, `password="a / b"`): cut there first, half of
/// the value would reach no rule. Part by part, because several rules only
/// know a secret by the word its line begins with. A text with no ` / ` is
/// read once: a second pass would find nothing, and a careless pattern can
/// read the engine's own placeholder as the start of a new secret.
fn ruled_path(redactor: &Redactor, text: &str) -> String {
    let whole = ruled(redactor, text);
    if !whole.contains(" / ") {
        return whole;
    }
    whole
        .split(" / ")
        .map(|part| redactor.redact(part))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Whether text carries a placeholder of either form: it was copied out of a
/// view that hides values, and sending it would configure the placeholder.
pub fn has_placeholder(text: &str) -> bool {
    has_numbered_placeholder(text) || text.contains(HIDDEN)
}

/// Whether text carries a placeholder of a view (`<secret:3>`): something
/// only Nocturne writes.
pub fn has_numbered_placeholder(text: &str) -> bool {
    Redactor::has_placeholders(text)
}

// ------------------------------------------------------------ what is said

/// Hides secret values in text that is written down or sent somewhere as it
/// is made. One per piece of work on one device (a plan, an apply, a
/// rollback), taught the configuration and the lines it is about.
pub struct Hider {
    redactor: Redactor,
    /// The values that are hidden wherever they stand, as of the last thing
    /// this was taught; `None` once it has been taught more. It is one
    /// pattern of every value, so it is built when a text is next hidden, not
    /// each time a configuration is read — an apply reads three of them in a
    /// row — and shared rather than copied, values and all, into every line.
    known: Mutex<Option<Arc<Known>>>,
}

impl Hider {
    /// The engine's rules plus these patterns.
    pub fn for_patterns(patterns: &[String]) -> Hider {
        Hider {
            redactor: rules(patterns),
            known: Mutex::new(None),
        }
    }

    pub fn for_platform(platform: &Platform) -> Hider {
        Hider::for_patterns(&platform.profile.config.secrets)
    }

    /// For device types by id. One that is not installed any more adds
    /// nothing; the engine's own rules always apply.
    pub fn for_profiles(app: &App, profile_ids: &[String]) -> Hider {
        Hider::for_patterns(&patterns_of(app, profile_ids))
    }

    /// For a device by id, when all there is to go by is the inventory.
    pub fn for_device(app: &App, device_id: &str) -> Hider {
        let profile = inventory::get(app, device_id)
            .map(|device| device.profile)
            .unwrap_or_default();
        Hider::for_profiles(app, &[profile])
    }

    /// Read a configuration, or lines of one, for the values in it, so that
    /// they are recognised afterwards where nothing around them says what
    /// they are: in a device's answer, in an error, after a prompt.
    pub fn learn(&self, text: &str) {
        let _ = ruled(&self.redactor, text);
        *self.known.lock() = None;
    }

    pub fn learn_lines(&self, lines: &[String]) {
        self.learn_each(lines.iter().map(String::as_str));
    }

    fn learn_each<'a>(&self, lines: impl IntoIterator<Item = &'a str>) {
        let text = lines.into_iter().collect::<Vec<_>>().join("\n");
        self.learn(&text);
    }

    /// The values known by now.
    fn known(&self) -> Arc<Known> {
        let mut known = self.known.lock();
        known
            .get_or_insert_with(|| Arc::new(Known::of(&self.redactor)))
            .clone()
    }

    /// The text with what is already hidden, and every value known by now,
    /// behind a placeholder the rules leave alone.
    fn shielded(&self, text: &str) -> String {
        self.known().replaced(&text.replace(HIDDEN, SHIELD), SHIELD)
    }

    /// Every placeholder, the engine's and the shield, as it is written down.
    ///
    /// A device type's pattern that takes whatever follows `secret:` (one
    /// written for `secret: …` in a file) reads the engine's own placeholder
    /// as a new secret, and leaves `<secret:<secret:7>` where one was. The
    /// value had already gone; what is left of the placeholder goes with it.
    fn unnumbered(text: &str) -> String {
        if text.contains("<secret:") {
            PLACEHOLDER.replace_all(text, HIDDEN).into_owned()
        } else {
            text.to_string()
        }
    }

    /// One configuration line.
    pub fn line(&self, line: &str) -> String {
        Self::unnumbered(&ruled(&self.redactor, &self.shielded(line)))
    }

    pub fn lines(&self, lines: &[String]) -> Vec<String> {
        lines.iter().map(|line| self.line(line)).collect()
    }

    /// A line with the sections it sits in, as the comparison of two
    /// configurations names it (`line vty 0 4 / password …`), or a line of a
    /// flat configuration that has a ` / ` of its own (see `ruled_path`).
    pub fn path(&self, path: &str) -> String {
        Self::unnumbered(&ruled_path(&self.redactor, &self.shielded(path)))
    }

    /// A sentence, an error, a device's answer.
    ///
    /// What this crate (and the engines under it) quote between backticks is
    /// a configuration line or a path of them, so each quotation is read as
    /// one — a rule that takes "the rest of the word" would otherwise take
    /// the closing backtick with it. Then the whole text is read once more
    /// as it stands, for what was not quoted.
    pub fn text(&self, text: &str) -> String {
        let shielded = self.shielded(text);
        let quoted = if shielded.contains('`') {
            let mut out = String::with_capacity(shielded.len());
            for (index, piece) in shielded.split('`').enumerate() {
                if index > 0 {
                    out.push('`');
                }
                if index % 2 == 1 {
                    out.push_str(&ruled_path(&self.redactor, piece));
                } else {
                    out.push_str(piece);
                }
            }
            out
        } else {
            shielded
        };
        Self::unnumbered(&ruled(&self.redactor, &quoted))
    }

    /// A session transcript.
    ///
    /// The rules are written for configuration, where `password …` opens a
    /// line. In a transcript the same words follow a prompt
    /// (`SW1(config-line)#password …`), so whatever follows a character a
    /// prompt can end in is offered to the rules again as if it began a line.
    /// A wrong guess hides a word that was no secret, or finds nothing.
    pub fn transcript(&self, text: &str) -> String {
        let whole = ruled(&self.redactor, &self.shielded(text));
        let mut out = String::with_capacity(whole.len());
        for (index, line) in whole.split('\n').enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(&self.after_prompts(line));
        }
        Self::unnumbered(&out)
    }

    fn after_prompts(&self, line: &str) -> String {
        let starts: Vec<usize> = line
            .char_indices()
            .take_while(|(at, _)| *at < PROMPT_WITHIN)
            .filter(|(_, ch)| matches!(ch, '#' | '>' | '$' | '%' | ']'))
            .map(|(at, ch)| at + ch.len_utf8())
            .take(PROMPT_CANDIDATES)
            .collect();
        for start in starts {
            let rest = &line[start..];
            let redacted = self.redactor.redact(rest);
            if redacted != rest {
                return format!("{}{redacted}", &line[..start]);
            }
        }
        line.to_string()
    }

    /// A configuration line as it was sent, with what the device said to it.
    pub fn sent(&self, line: &CommandOutput) -> CommandOutput {
        CommandOutput {
            command: self.line(&line.command),
            output: self.text(&line.output),
            error: line.error.as_ref().map(|error| self.text(error)),
            elapsed_ms: line.elapsed_ms,
        }
    }

    pub fn check(&self, check: Check) -> Check {
        Check {
            detail: self.text(&check.detail),
            ..check
        }
    }

    /// Every string in a JSON value (the details of an error, of an audit
    /// entry). Keys are this crate's own field names and are left alone.
    pub fn json(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.text(text)),
            Value::Array(items) => Value::Array(items.iter().map(|item| self.json(item)).collect()),
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, item)| (key.clone(), self.json(item)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    pub fn error(&self, err: impl Into<AppError>) -> AppError {
        let err = err.into();
        AppError {
            code: err.code,
            message: self.text(&err.message),
            details: err.details.as_ref().map(|details| self.json(details)),
        }
    }

    /// What a plan says about its own lines: the warnings, the reasons for
    /// its risk level, and why a resource is protected. The lines themselves
    /// (`commands`, `rollback`, `diff`, a hit's `command`) are not touched.
    pub fn said_about(&self, plan: &mut Plan) {
        for warning in &mut plan.warnings {
            *warning = self.text(warning);
        }
        for reason in &mut plan.risk.reasons {
            *reason = self.text(reason);
        }
        for hit in &mut plan.protected_hits {
            hit.subject = self.text(&hit.subject);
            hit.why = self.text(&hit.why);
        }
    }

    /// Protected hits for an error's details: the commands in them are lines.
    pub fn hits(&self, hits: &[ProtectedHit]) -> Vec<ProtectedHit> {
        hits.iter()
            .map(|hit| ProtectedHit {
                kind: hit.kind.clone(),
                subject: self.text(&hit.subject),
                command: self.line(&hit.command),
                why: self.text(&hit.why),
            })
            .collect()
    }

    /// What was recorded on one device. An apply hides each piece as it
    /// records it; this is for a record written before it did.
    fn result(&self, result: &mut TargetResult) {
        for Step { detail, .. } in &mut result.steps {
            *detail = self.text(detail);
        }
        for line in &mut result.sent {
            *line = self.sent(line);
        }
        if let Some(error) = &mut result.error {
            *error = self.text(error);
        }
        for check in &mut result.verification {
            check.detail = self.text(&check.detail);
        }
        result.transcript = self.transcript(&result.transcript);
    }
}

// ------------------------------------------------------------ what is shown

/// The placeholders of one answer.
struct Shown {
    redactor: Redactor,
    known: Arc<Known>,
}

impl Shown {
    /// For an answer made of these lines (and intended configurations, each
    /// read whole). They are all read before any of them is written, so that
    /// a value is known in the first line it stands in as well as in the
    /// last, and is numbered in the order it is read.
    fn of<'a>(patterns: &[String], texts: impl IntoIterator<Item = &'a str>) -> Shown {
        let redactor = rules(patterns);
        for text in texts {
            let _ = ruled(&redactor, text);
        }
        let known = Arc::new(Known::of(&redactor));
        Shown { redactor, known }
    }

    /// A hider for what is said about the lines of this answer, knowing the
    /// values they were just read for rather than reading them all again.
    fn hider(&self, patterns: &[String]) -> Hider {
        Hider {
            redactor: rules(patterns),
            known: Mutex::new(Some(self.known.clone())),
        }
    }

    /// A line, or an intended configuration of many.
    fn text(&self, text: &str) -> String {
        let mut out = ruled(&self.redactor, text);
        // A value the rules know from one line, standing in another where
        // nothing says what it is (a description, a remark).
        let left: Vec<String> = self
            .known
            .found_in(&out)
            .into_iter()
            .map(str::to_string)
            .collect();
        for value in left {
            out = self.redactor.redact_value(&out, &value);
        }
        out
    }

    fn lines(&self, lines: &mut [String]) {
        for line in lines {
            *line = self.text(line);
        }
    }

    fn intent(&self, intent: &mut Intent) {
        match intent {
            Intent::Lines { lines } => self.lines(lines),
            Intent::Intended { config, .. } => *config = self.text(config),
        }
    }

    fn diff(&self, diff: &mut TextDiff) {
        for line in &mut diff.lines {
            line.text = self.text(&line.text);
        }
    }

    fn plan(&self, plan: &mut Plan) {
        self.lines(&mut plan.commands);
        self.lines(&mut plan.rollback);
        self.diff(&mut plan.diff);
        for hit in &mut plan.protected_hits {
            hit.command = self.text(&hit.command);
        }
    }
}

/// Every line of a plan that is kept whole, in the order they are read.
fn plan_lines(plan: &Plan) -> impl Iterator<Item = &str> {
    plan.commands
        .iter()
        .chain(&plan.rollback)
        .map(String::as_str)
        .chain(plan.diff.lines.iter().map(|line| line.text.as_str()))
        .chain(plan.protected_hits.iter().map(|hit| hit.command.as_str()))
}

fn intent_lines(intent: &Intent) -> Vec<&str> {
    match intent {
        Intent::Lines { lines } => lines.iter().map(String::as_str).collect(),
        Intent::Intended { config, .. } => config.lines().collect(),
    }
}

/// The lines one device of a change is made of, an intended configuration
/// as one text: a configuration of twenty thousand lines is read in one pass
/// whole, and in twenty thousand line by line.
fn target_texts(target: &Target) -> Vec<&str> {
    let mut texts = match &target.intent {
        Intent::Lines { lines } => lines.iter().map(String::as_str).collect(),
        Intent::Intended { config, .. } => vec![config.as_str()],
    };
    if let Some(plan) = &target.plan {
        texts.extend(plan_lines(plan));
    }
    texts
}

/// How many different placeholders stand in these (already redacted) lines.
fn placeholders_in<'a>(lines: impl IntoIterator<Item = &'a str>) -> usize {
    let mut found: BTreeSet<&str> = BTreeSet::new();
    for line in lines {
        found.extend(PLACEHOLDER.find_iter(line).map(|token| token.as_str()));
    }
    found.len()
}

/// A change as it is answered to a caller.
#[derive(Clone, Debug, Serialize)]
pub struct ChangeView {
    #[serde(flatten)]
    pub change: Change,
    /// Whether placeholders stand where the secret values of the intents and
    /// plans are. `false` only for a read that asked for the lines whole.
    pub redacted: bool,
    /// How many different values are hidden in this answer. `0` means the
    /// lines shown are exactly the lines that will be sent.
    pub hidden: usize,
}

/// A plan as it is answered to a caller (`changes.preview`).
#[derive(Clone, Debug, Serialize)]
pub struct PlanView {
    #[serde(flatten)]
    pub plan: Plan,
    pub redacted: bool,
    pub hidden: usize,
}

/// The device types whose patterns apply to a change: the one each plan was
/// computed for, or the device's own while there is no plan.
fn profiles_of(app: &App, change: &Change) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for target in &change.targets {
        let id = profile_of(app, target);
        if !id.is_empty() && !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

fn profile_of(app: &App, target: &Target) -> String {
    match &target.plan {
        Some(plan) if !plan.profile.is_empty() => plan.profile.clone(),
        _ => inventory::get(app, &target.device_id)
            .map(|device| device.profile)
            .unwrap_or_default(),
    }
}

/// A hider for what is said about one device of a change, taught its lines.
pub fn hider_for_target(app: &App, target: &Target) -> Hider {
    let hide = Hider::for_profiles(app, &[profile_of(app, target)]);
    hide.learn_each(target_texts(target));
    hide
}

/// A hider for what is said about a change as a whole, taught its lines.
pub fn hider_for(app: &App, change: &Change) -> Hider {
    let hide = Hider::for_profiles(app, &profiles_of(app, change));
    hide.learn_each(change.targets.iter().flat_map(target_texts));
    hide
}

/// The view of a stored change. Never store what this returns: with
/// `redact` the lines in it are not the lines that would be sent.
///
/// The second value is, per target, how many values the lines of that target
/// hold — what an unredacted read reveals, for its audit entry.
fn view(app: &App, stored: &Change, redact: bool) -> (ChangeView, Vec<usize>) {
    let patterns = patterns_of(app, &profiles_of(app, stored));
    let shown = Shown::of(&patterns, stored.targets.iter().flat_map(target_texts));
    let hider = shown.hider(&patterns);

    let mut change = stored.clone();
    let mut revealed = Vec::with_capacity(change.targets.len());
    for target in &mut change.targets {
        let mut intent = target.intent.clone();
        shown.intent(&mut intent);
        let mut plan = target.plan.clone();
        if let Some(plan) = &mut plan {
            shown.plan(plan);
        }
        revealed.push(placeholders_in(
            intent_lines(&intent)
                .into_iter()
                .chain(plan.iter().flat_map(plan_lines)),
        ));
        if redact {
            target.intent = intent;
            target.plan = plan;
        }
        if let Some(plan) = &mut target.plan {
            hider.said_about(plan);
        }
        // What an apply records, it records hidden, and says so. Only a
        // record from before it did is gone over here, each time it is read.
        if let Some(result) = &mut target.result
            && !result.redacted
        {
            hider.result(result);
            result.redacted = true;
        }
    }
    let hidden = if redact {
        placeholders_in(change.targets.iter().flat_map(target_texts))
    } else {
        0
    };
    (
        ChangeView {
            change,
            redacted: redact,
            hidden,
        },
        revealed,
    )
}

/// A change with a placeholder where each secret value stands: what every
/// method answers unless it was asked otherwise.
pub fn change(app: &App, stored: &Change) -> ChangeView {
    view(app, stored, true).0
}

/// A change with its lines exactly as they will be sent, for the person who
/// has to approve them — and the record that somebody looked.
pub fn change_unredacted(app: &App, stored: &Change) -> ChangeView {
    let (whole, revealed) = view(app, stored, false);
    let mut recorded = false;
    for (target, values) in stored.targets.iter().zip(&revealed) {
        if *values == 0 {
            continue;
        }
        let name = inventory::get(app, &target.device_id)
            .map(|device| device.name)
            .unwrap_or_else(|_| target.device_id.clone());
        audit_unredacted(
            app,
            Some(&target.device_id),
            format!(
                "Viewed the lines of change `{}` for `{name}` with their secret values",
                stored.title
            ),
            json!({"change_id": stored.id, "values": values}),
        );
        recorded = true;
    }
    if !recorded {
        // The read was asked for whole all the same, and that is on record.
        audit_unredacted(
            app,
            None,
            format!(
                "Viewed change `{}` unredacted; nothing in it was hidden",
                stored.title
            ),
            json!({"change_id": stored.id, "values": 0}),
        );
    }
    whole
}

/// A plan that belongs to no change (`changes.preview`), as it is answered.
pub fn plan(
    app: &App,
    device_id: &str,
    device_name: &str,
    computed: Plan,
    redact: bool,
) -> PlanView {
    let patterns = patterns_of(app, std::slice::from_ref(&computed.profile));
    let shown = Shown::of(&patterns, plan_lines(&computed));
    let mut hidden_lines = computed.clone();
    shown.plan(&mut hidden_lines);
    let values = placeholders_in(plan_lines(&hidden_lines));
    if redact {
        return PlanView {
            plan: hidden_lines,
            redacted: true,
            hidden: values,
        };
    }
    audit_unredacted(
        app,
        Some(device_id),
        format!(
            "Viewed the lines of a previewed change for `{device_name}` with their secret values"
        ),
        json!({"values": values}),
    );
    PlanView {
        plan: computed,
        redacted: false,
        hidden: 0,
    }
}

/// Record that somebody asked to see the lines of a change with the secret
/// values in them.
fn audit_unredacted(app: &App, device_id: Option<&str>, message: String, detail: Value) {
    app.audit(VIEW_UNREDACTED, device_id, message, detail);
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_drivers::hier::DiffLine;

    fn hider() -> Hider {
        Hider::for_patterns(&[])
    }

    /// The patterns of a device type as it ships.
    fn shipped(id: &str) -> Vec<String> {
        let (registry, _) = nocturne_drivers::profile::Registry::builtin();
        registry
            .get(id)
            .unwrap_or_else(|| panic!("no built-in device type `{id}`"))
            .profile
            .config
            .secrets
            .clone()
    }

    #[test]
    fn a_value_with_a_slash_in_it_is_not_cut_in_two_by_the_path_it_is_named_in() {
        // Nothing is learnt here: these are found by the rules or not at all.
        // A value this short is not looked for out of context either.
        let routeros = Hider::for_patterns(&shipped("mikrotik_routeros"));
        let line =
            r#"/ppp secret add comment="vpn / roaming" name=u1 password="ab / cd" service=pppoe"#;
        let hidden =
            r#"/ppp secret add comment="vpn / roaming" name=u1 password="<hidden>" service=pppoe"#;
        assert_eq!(routeros.path(line), hidden);
        assert_eq!(
            routeros.text(&format!("the device rejected `{line}`: % Invalid input")),
            format!("the device rejected `{hidden}`: % Invalid input")
        );
        assert_eq!(
            Hider::for_patterns(&shipped("juniper_junos"))
                .path(r#"set snmp community "ab / cd" authorization read-only"#),
            r#"set snmp community "<hidden>" authorization read-only"#
        );
        assert_eq!(
            Hider::for_patterns(&shipped("linux")).path(r#"psk="ab / cd""#),
            r#"psk="<hidden>""#
        );
        // A path of sections is still read a part at a time: these rules
        // only know a secret by the word its line begins with.
        let ios = hider();
        assert_eq!(
            ios.path("line vty 0 4 / password lowercasevty"),
            "line vty 0 4 / password <hidden>"
        );
        assert_eq!(
            ios.path("key chain KC / key 1 / key-string lowercasekc"),
            "key chain KC / key 1 / key-string <hidden>"
        );
    }

    #[test]
    fn a_pattern_that_reads_the_engines_own_marker_as_a_secret_leaves_nothing_of_it() {
        // The Linux device type hides `secret: …`, which `<secret:3>` is too.
        let hide = Hider::for_patterns(&shipped("linux"));
        let once = hide
            .text("on the device but not predicted: `psk=\"<hidden>\"`, `secret: lowercasesecret`");
        assert!(!once.contains("<secret:"), "{once}");
        assert!(!once.contains("lowercasesecret"), "{once}");
        assert_eq!(hide.text(&once), once, "hiding it again changes nothing");
        // Hidden once, the line is whole. Hidden again, the pattern takes the
        // marker and the quote after it — never anything of the value.
        let line = hide.line(r#"psk="lowercasepsk""#);
        assert_eq!(line, r#"psk="<hidden>""#);
        let again = hide.line(&line);
        assert!(
            again.starts_with(r#"psk="<hidden>"#) && !again.contains("<secret:"),
            "{again}"
        );
    }

    #[test]
    fn a_quote_left_open_does_not_carry_a_rule_past_the_next_lines_value() {
        // A rule that reads up to the closing quote, given one nobody closed:
        // read whole, it takes the next line's opening quote for the end of
        // the value, and the value after that for no value at all.
        let answer = "% Invalid input: password=\"abc\n/user add name=ops password=\"Wh0le!Secret\" group=full";
        let hidden = hider().text(answer);
        assert!(!hidden.contains("Wh0le!Secret"), "{hidden}");
        assert!(hidden.ends_with(" group=full"), "{hidden}");
    }

    #[test]
    fn a_quoted_line_keeps_its_backticks_and_loses_its_value() {
        let hide = hider();
        assert_eq!(
            hide.text(
                "the device rejected `enable secret 5 $1$abcd$efghijklmnopqrstuv`: % Invalid input"
            ),
            "the device rejected `enable secret 5 <hidden>`: % Invalid input"
        );
        // A path of sections: the line password is only a secret to a rule
        // that sees its line begin with `password`.
        assert_eq!(
            hide.text("missing from the device: `line vty 0 4 / password Pl4inLinePw`, `vlan 10 / name users`"),
            "missing from the device: `line vty 0 4 / password <hidden>`, `vlan 10 / name users`"
        );
        // Hiding what is already hidden changes nothing — also where the
        // marker has punctuation attached, which a rule that takes "the rest
        // of the word" would otherwise swallow with it.
        for hidden in [
            "the device rejected `enable secret 5 <hidden>`: % Invalid input",
            "community: <hidden>, key <hidden>; password=<hidden>.",
        ] {
            assert_eq!(hide.text(hidden), hidden);
        }
        let once =
            hide.text("on the device but not predicted: `snmp-server community S3cr3tRW RW`");
        assert_eq!(hide.text(&once), once);
        assert_eq!(
            once,
            "on the device but not predicted: `snmp-server community <hidden> RW`"
        );
    }

    #[test]
    fn a_value_learnt_from_a_configuration_is_hidden_where_nothing_says_what_it_is() {
        let hide = hider();
        hide.learn("hostname SW1\nsnmp-server community N0ctComm-R0 RO\nline vty 0 4\n password Pl4inLinePw\n");
        assert_eq!(
            hide.text("%SNMP-3-AUTHFAIL: bad community N0ctComm-R0 from 192.0.2.9"),
            "%SNMP-3-AUTHFAIL: bad community <hidden> from 192.0.2.9"
        );
        assert_eq!(hide.text("the hostname is SW1"), "the hostname is SW1");
        // An ordinary word the rules took for a value is not chased through
        // every other text.
        hide.learn("snmp-server community public RO\n");
        assert_eq!(hide.text("a public address"), "a public address");
    }

    #[test]
    fn a_device_types_pattern_is_given_one_line_at_a_time() {
        // Written, as these are, for a line: without the multi-line flag its
        // `$` is the end of the text.
        let patterns = vec![r"^\s*wlan-key (?P<secret>\S+)$".to_string()];
        let config = "hostname AP1\n wlan-key lowercasewlankey\nend\n";

        let hide = Hider::for_patterns(&patterns);
        assert_eq!(
            hide.text(config),
            "hostname AP1\n wlan-key <hidden>\nend\n",
            "a device's answer of several lines"
        );
        assert_eq!(
            hide.transcript(&format!("AP1#show running-config\n{config}AP1#")),
            "AP1#show running-config\nhostname AP1\n wlan-key <hidden>\nend\nAP1#"
        );
        assert_eq!(
            hide.transcript("AP1#show running-config\r\n wlan-key lowercasewlankey\r\nAP1#"),
            "AP1#show running-config\r\n wlan-key <hidden>\r\nAP1#",
            "as a session prints it"
        );

        let shown = Shown::of(&patterns, config.lines());
        let mut intent = Intent::Intended {
            config: config.to_string(),
            exact: true,
        };
        shown.intent(&mut intent);
        assert_eq!(
            intent,
            Intent::Intended {
                config: "hostname AP1\n wlan-key <secret:1>\nend\n".into(),
                exact: true
            },
            "an intended configuration keeps its lines"
        );
    }

    #[test]
    fn a_secret_typed_at_a_prompt_is_hidden_in_a_transcript() {
        let hide = hider();
        let transcript = "SW1(config)#line vty 0 4\nSW1(config-line)#password lowercasepw\nSW1(config-line)#exit\nSW1#show running-config\n password lowercasepw\nSW1#";
        let hidden = hide.transcript(transcript);
        assert!(!hidden.contains("lowercasepw"), "{hidden}");
        assert_eq!(
            hidden,
            "SW1(config)#line vty 0 4\nSW1(config-line)#password <hidden>\nSW1(config-line)#exit\nSW1#show running-config\n password <hidden>\nSW1#"
        );
        assert!(!hidden.contains("<secret:"), "no number without its table");
    }

    fn secret_plan() -> Plan {
        Plan {
            commands: vec![
                "snmp-server community N0ctComm-R0 RO".into(),
                "interface GigabitEthernet0/2".into(),
                "description the key is N0ctComm-R0".into(),
                "exit".into(),
            ],
            rollback: vec!["no snmp-server community N0ctComm-R0 RO".into()],
            diff: TextDiff {
                lines: vec![
                    DiffLine {
                        tag: " ".into(),
                        text: "enable secret 5 $1$abcd$efghijklmnopqrstuv".into(),
                        old_line: Some(3),
                        new_line: Some(3),
                    },
                    DiffLine {
                        tag: "+".into(),
                        text: "snmp-server community N0ctComm-R0 RO".into(),
                        old_line: None,
                        new_line: Some(4),
                    },
                ],
                added: 1,
                removed: 0,
            },
            plan_hash: "the-hash".into(),
            ..Plan::default()
        }
    }

    #[test]
    fn one_value_has_one_number_wherever_it_stands_in_an_answer() {
        let stored = secret_plan();
        let shown = Shown::of(&[], plan_lines(&stored));
        let mut plan = stored.clone();
        shown.plan(&mut plan);
        assert_eq!(
            plan.commands,
            vec![
                "snmp-server community <secret:1> RO",
                "interface GigabitEthernet0/2",
                "description the key is <secret:1>",
                "exit"
            ]
        );
        assert_eq!(
            plan.rollback,
            vec!["no snmp-server community <secret:1> RO"]
        );
        assert_eq!(plan.diff.lines[0].text, "enable secret 5 <secret:2>");
        assert_eq!(
            plan.diff.lines[1].text,
            "snmp-server community <secret:1> RO"
        );
        assert_eq!(placeholders_in(plan_lines(&plan)), 2);
        // The view is a copy: what is kept, and its hash, are untouched.
        assert_eq!(stored, secret_plan());
        assert_eq!(plan.plan_hash, stored.plan_hash);
    }

    #[test]
    fn the_hash_is_of_the_lines_that_are_sent_whatever_is_shown() {
        use crate::model::Options;
        use crate::plan::plan_hash;
        use nocturne_app::model::Device;

        let device = Device {
            id: "d1".into(),
            name: "SW1".into(),
            ..Device::default()
        };
        let options = Options::default();
        let hash = |plan: &Plan| {
            plan_hash(
                &device,
                &plan.commands,
                &plan.rollback,
                &plan.base_sha256,
                &options,
            )
        };
        let mut stored = secret_plan();
        stored.base_sha256 = "the-base".into();
        stored.plan_hash = hash(&stored);

        let shown = Shown::of(&[], plan_lines(&stored));
        let mut view = stored.clone();
        shown.plan(&mut view);
        assert_ne!(view.commands, stored.commands, "something was hidden");

        // The view carries the hash of the plan that is kept…
        assert_eq!(view.plan_hash, stored.plan_hash);
        // …which hashes the same after being shown as before…
        assert_eq!(hash(&stored), stored.plan_hash);
        // …and is not the hash of what is on the screen: an approval binds
        // the lines that will be sent, placeholders or no placeholders.
        assert_ne!(hash(&view), stored.plan_hash);
    }

    #[test]
    fn known_values_are_found_the_same_with_one_pattern_or_one_by_one() {
        let redactor = Redactor::new();
        let _ = redactor.redact("snmp-server community N0ctComm-R0 RO");
        let _ = redactor.redact("snmp-server community N0ctComm-R0-Long RW");
        let _ = redactor.redact("snmp-server community public RO");
        let with_pattern = Known::of(&redactor);
        assert!(with_pattern.any.is_some());
        assert_eq!(
            with_pattern.values,
            vec!["N0ctComm-R0-Long", "N0ctComm-R0"],
            "longest first, and `public` is an ordinary word"
        );
        let one_by_one = Known {
            any: None,
            ..with_pattern.clone()
        };
        let text = "trap from N0ctComm-R0-Long, then N0ctComm-R0, then public";
        for known in [&with_pattern, &one_by_one] {
            assert_eq!(
                known.replaced(text, "<hidden>"),
                "trap from <hidden>, then <hidden>, then public"
            );
            assert_eq!(known.found_in("only N0ctComm-R0 here"), vec!["N0ctComm-R0"]);
            assert!(known.found_in("nothing here").is_empty());
        }
    }

    #[test]
    fn a_placeholder_of_either_form_is_recognised() {
        assert!(has_placeholder("snmp-server community <secret:2> RO"));
        assert!(has_placeholder("enable secret 5 <secret:1:weak>"));
        assert!(has_placeholder("key <hidden>"));
        assert!(!has_placeholder("snmp-server community S3cr3t RO"));
        assert!(!has_placeholder("description <secret> hidden uplink"));
    }
}
