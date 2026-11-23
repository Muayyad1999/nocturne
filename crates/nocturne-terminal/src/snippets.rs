//! Saved commands.
//!
//! A snippet is a piece of text an engineer keeps around to paste into a
//! terminal — a block of configuration, a troubleshooting sequence, the
//! incantation nobody remembers. They are small and few, so they live as
//! ordinary documents.
//!
//! What is stored is `{id, name, text, profile, tags}` and two timestamps,
//! as it always was. The variables of a snippet are not stored: they are
//! read out of its text each time it is listed, so a snippet saved before
//! there were variables needs no migration — its text is read by today's
//! rules, and one that does not pass them is listed with what is wrong
//! ([`Row::problem`]) instead of being dropped or guessed at.

mod send;
mod template;
mod transfer;

use std::collections::BTreeMap;

use nocturne_app::util::{matches_query, new_id, now_ms};
use nocturne_app::{App, AppError, AppResult, NoParams, Router, profiles};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::platform;
use crate::session::TerminalState;
use template::{RenderError, Rendered, Template, Variable};

const COLLECTION: &str = "terminal_snippets";
/// Longest snippet kept. Anything larger is a file, not a snippet.
const MAX_TEXT: usize = 256 * 1024;
const MAX_NAME: usize = 200;
/// Most snippets one installation keeps, so the list stays a list.
const MAX_SNIPPETS: usize = 2000;
const MAX_TAGS: usize = 32;
/// Longest tag, in characters.
const MAX_TAG: usize = 64;

/// One writer at a time. A save reads the library, checks the new snippet
/// against it (no second one of a name for a device type, no more than the
/// most kept) and then writes; two saves that read before either wrote would
/// both pass a check only one of them should. The handlers do not wait on
/// anything while they hold it.
#[derive(Default)]
pub(crate) struct Library {
    writing: parking_lot::Mutex<()>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snippet {
    pub id: String,
    pub name: String,
    /// The text as it was written, placeholders and all.
    pub text: String,
    /// The device profile this is for (`cisco_ios`); `None` means any.
    pub profile: Option<String>,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Snippet {
    /// What makes two snippets the same one to a person: the name, whatever
    /// its case, and the device type. Two that differ in the second are
    /// variants of each other.
    fn key(&self) -> (String, Option<String>) {
        key_of(&self.name, self.profile.as_deref())
    }

    fn audience(&self) -> String {
        match &self.profile {
            Some(profile) => format!("for `{profile}` devices"),
            None => "for any device".to_string(),
        }
    }
}

fn key_of(name: &str, profile: Option<&str>) -> (String, Option<String>) {
    (
        name.trim().to_lowercase(),
        profile.map(str::to_ascii_lowercase),
    )
}

/// A snippet as it is listed: the stored record, and what its text says.
#[derive(Debug, Serialize)]
struct Row {
    #[serde(flatten)]
    snippet: Snippet,
    /// The variables in its text, in the order they first appear.
    variables: Vec<Variable>,
    /// Why this snippet cannot be rendered or sent as it is. Only on one
    /// saved before today's rules; saving it again, corrected, clears it.
    #[serde(skip_serializing_if = "Option::is_none")]
    problem: Option<String>,
    /// Whether this is the one to use in the session (or for the device
    /// type) the list was asked for. Absent when it was asked for neither.
    #[serde(skip_serializing_if = "Option::is_none")]
    chosen: Option<bool>,
    /// The variant that is used instead of this one.
    #[serde(skip_serializing_if = "Option::is_none")]
    chosen_instead: Option<String>,
}

impl Row {
    fn of(snippet: Snippet) -> Row {
        let (variables, problem) = match Template::parse(&snippet.text) {
            Ok(template) => (template.variables, None),
            Err(malformed) => (Vec::new(), Some(malformed.to_string())),
        };
        Row {
            snippet,
            variables,
            problem,
            chosen: None,
            chosen_instead: None,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ListParams {
    /// Only snippets for this profile (and those for any profile).
    profile: Option<String>,
    /// The snippets for this session: its device type decides.
    session_id: Option<String>,
    /// Words that must all appear in the name, tags or text.
    query: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SaveParams {
    /// Omitted: a new snippet. Given: the snippet to replace.
    id: Option<String>,
    name: String,
    text: String,
    profile: Option<String>,
    tags: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DeleteParams {
    id: Option<String>,
    ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RenderParams {
    id: String,
    /// A value for each variable that has no default, by name. JSON of any
    /// kind is taken in so that what is not a value can be refused in words
    /// of our own: the message serde writes about a parameter of the wrong
    /// type repeats it, and a value may be a password.
    #[serde(default)]
    values: Value,
}

/// The values of a render or a send, by variable name. Leaving them out is
/// giving none.
fn values_of(given: Value) -> AppResult<BTreeMap<String, Value>> {
    match given {
        Value::Null => Ok(BTreeMap::new()),
        // More names than a snippet can have variables: not looked at one by
        // one, or the refusal would be as long as what was sent.
        Value::Object(values) if values.len() > template::MAX_VARIABLES => {
            Err(AppError::invalid(format!(
                "`values` names {} variables; a snippet has at most {}",
                values.len(),
                template::MAX_VARIABLES
            )))
        }
        Value::Object(values) => Ok(values.into_iter().collect()),
        _ => Err(AppError::invalid(
            "`values` is an object: the text for each variable, by the variable's name",
        )),
    }
}

#[derive(Debug, Serialize)]
struct RenderResult {
    /// What will be typed, with every line break as `\n`.
    text: String,
    lines: usize,
    /// SHA-256 of `text`, in hex.
    sha256: String,
    variables: Vec<Variable>,
}

/// Who a list is for.
enum Audience {
    /// Nobody in particular: every snippet, and no choice among variants.
    Everyone,
    /// A session or a device type. `None`: a session whose device type is
    /// not known.
    DeviceType(Option<String>),
}

fn audience(app: &App, filter: &ListParams) -> AppResult<Audience> {
    let profile = filter
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|profile| !profile.is_empty());
    let session_id = filter
        .session_id
        .as_deref()
        .filter(|session_id| !session_id.is_empty());
    match (session_id, profile) {
        (Some(_), Some(_)) => Err(AppError::invalid(
            "terminal.snippets.list takes `session_id` or `profile`, not both: a session's device type is its own",
        )),
        (Some(session_id), None) => {
            let info = app.state::<TerminalState>().get(session_id)?.info();
            Ok(Audience::DeviceType(platform::device_type(
                app,
                info.device_id.as_deref(),
            )))
        }
        (None, Some(profile)) => Ok(Audience::DeviceType(Some(profile.to_string()))),
        (None, None) => Ok(Audience::Everyone),
    }
}

/// Decide, for a session of `device_type`, which of the snippets are offered
/// and which of two that share a name is used.
///
/// The one written for the session's device type wins over the one written
/// for any device. A snippet for another device type is not offered at all.
/// When the device type is not known nothing is left out — the person knows
/// what they are connected to better than Nocturne does — but only the
/// snippets for any device are marked as chosen: Nocturne cannot say that a
/// `cisco_ios` snippet is right here.
fn choose(mut rows: Vec<Row>, device_type: Option<&str>) -> Vec<Row> {
    let Some(device_type) = device_type else {
        for row in &mut rows {
            row.chosen = Some(row.snippet.profile.is_none());
        }
        return rows;
    };
    rows.retain(|row| {
        row.snippet
            .profile
            .as_deref()
            .is_none_or(|profile| profile.eq_ignore_ascii_case(device_type))
    });
    let own: BTreeMap<String, String> = rows
        .iter()
        .rev()
        .filter(|row| row.snippet.profile.is_some())
        .map(|row| (row.snippet.key().0, row.snippet.id.clone()))
        .collect();
    for row in &mut rows {
        let instead = match row.snippet.profile {
            Some(_) => None,
            None => own.get(&row.snippet.key().0).cloned(),
        };
        row.chosen = Some(instead.is_none());
        row.chosen_instead = instead;
    }
    rows
}

fn list(app: &App, filter: ListParams) -> AppResult<Vec<Row>> {
    let audience = audience(app, &filter)?;
    let mut snippets: Vec<Snippet> = app.store().list(COLLECTION)?;
    snippets.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.profile.cmp(&right.profile))
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut rows: Vec<Row> = snippets.into_iter().map(Row::of).collect();
    // The choice is made among all of them and the search narrows afterwards:
    // a variant that the words do not match still stands in for its twin.
    if let Audience::DeviceType(device_type) = audience {
        rows = choose(rows, device_type.as_deref());
    }
    if let Some(query) = filter.query.as_deref().map(str::trim)
        && !query.is_empty()
    {
        rows.retain(|row| {
            let snippet = &row.snippet;
            matches_query(&snippet.name, query)
                || matches_query(&snippet.text, query)
                || snippet.tags.iter().any(|tag| matches_query(tag, query))
        });
    }
    Ok(rows)
}

/// What a snippet is made of before it has an id: the part a person writes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Draft {
    name: String,
    text: String,
    profile: Option<String>,
    tags: Vec<String>,
}

/// The rules every snippet passes on its way in, whether a person typed it
/// or a document brought it. One place, so that an import can never store
/// what the form would have refused.
fn check(
    app: &App,
    name: &str,
    text: String,
    profile: Option<&str>,
    tags: Vec<String>,
) -> AppResult<(Draft, Template)> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::invalid("a snippet needs a name"));
    }
    if name.chars().count() > MAX_NAME {
        return Err(AppError::invalid(format!(
            "the name of a snippet is at most {MAX_NAME} characters"
        )));
    }
    if let Some((_, what)) = template::first_unshowable(name) {
        return Err(AppError::invalid(format!(
            "the name of a snippet cannot contain {what}"
        )));
    }
    if text.trim().is_empty() {
        return Err(AppError::invalid(format!(
            "the snippet `{name}` has no text; there would be nothing to type"
        )));
    }
    if text.len() > MAX_TEXT {
        return Err(AppError::invalid(format!(
            "`{name}` is {} bytes; a snippet holds at most {MAX_TEXT}. Keep larger text in a file and send it with SFTP.",
            text.len()
        )));
    }
    let template = Template::parse(&text).map_err(|malformed| {
        AppError::invalid(format!("the text of `{name}`, {malformed}")).with_details(json!({
            "line": malformed.line,
            "column": malformed.column,
        }))
    })?;
    let profile = match profile.map(str::trim) {
        None | Some("") => None,
        Some(profile) => {
            // Saying it is for a profile that does not exist would hide it
            // from every list that filters by one.
            profiles::platform(app, profile)?;
            Some(profile.to_string())
        }
    };
    let draft = Draft {
        name: name.to_string(),
        text,
        profile,
        tags: clean_tags(tags)?,
    };
    Ok((draft, template))
}

fn clean_tags(tags: Vec<String>) -> AppResult<Vec<String>> {
    let mut tags: Vec<String> = tags
        .into_iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect();
    tags.sort();
    tags.dedup();
    if tags.len() > MAX_TAGS {
        return Err(AppError::invalid(format!(
            "a snippet has at most {MAX_TAGS} tags; this one was given {}",
            tags.len()
        )));
    }
    for tag in &tags {
        if tag.chars().count() > MAX_TAG {
            return Err(AppError::invalid(format!(
                "a tag is at most {MAX_TAG} characters"
            )));
        }
        if let Some((_, what)) = template::first_unshowable(tag) {
            return Err(AppError::invalid(format!("a tag cannot contain {what}")));
        }
    }
    Ok(tags)
}

fn save(app: &App, params: SaveParams) -> AppResult<Row> {
    let (draft, _) = check(
        app,
        &params.name,
        params.text,
        params.profile.as_deref(),
        params.tags,
    )?;
    let library = app.state::<Library>();
    let _writing = library.writing.lock();
    let all: Vec<Snippet> = app.store().list(COLLECTION)?;
    let existing: Option<&Snippet> = match params.id.as_deref() {
        Some(id) => Some(
            all.iter()
                .find(|snippet| snippet.id == id)
                .ok_or_else(|| AppError::not_found(format!("snippet `{id}`")))?,
        ),
        None => None,
    };
    if existing.is_none() && all.len() >= MAX_SNIPPETS {
        return Err(AppError::conflict(format!(
            "there are already {MAX_SNIPPETS} snippets, which is the most Nocturne keeps; delete some first"
        )));
    }
    // Two snippets may share a name only as variants for different device
    // types. One that already shares both with another — saved before this
    // was a rule — may still be edited under the name it has; it is taking a
    // name, or moving to a device type, that is refused.
    let key = key_of(&draft.name, draft.profile.as_deref());
    let keeps_its_place = existing.is_some_and(|snippet| snippet.key() == key);
    if !keeps_its_place
        && let Some(taken) = all
            .iter()
            .find(|other| Some(other.id.as_str()) != params.id.as_deref() && other.key() == key)
    {
        return Err(AppError::conflict(format!(
            "there is already a snippet called `{}` {}; give this one another name, or make it for another device type",
            taken.name,
            taken.audience()
        ))
        .with_details(json!({"id": taken.id})));
    }

    let now = now_ms();
    let snippet = Snippet {
        id: existing.map_or_else(new_id, |snippet| snippet.id.clone()),
        name: draft.name,
        text: draft.text,
        profile: draft.profile,
        tags: draft.tags,
        created_at: existing.map_or(now, |snippet| snippet.created_at),
        updated_at: now,
    };
    app.store().put(COLLECTION, &snippet.id, &snippet)?;
    app.audit(
        "terminal.snippet.save",
        None,
        format!("Saved the terminal snippet `{}`", snippet.name),
        json!({"id": snippet.id, "profile": snippet.profile, "tags": snippet.tags}),
    );
    app.publish(
        "terminal.snippets.changed",
        &json!({"saved": [&snippet.id]}),
    );
    Ok(Row::of(snippet))
}

#[derive(Debug, Serialize)]
struct DeleteResult {
    deleted: usize,
}

fn delete(app: &App, params: DeleteParams) -> AppResult<DeleteResult> {
    let mut ids = params.ids;
    ids.extend(params.id);
    ids.retain(|id| !id.trim().is_empty());
    ids.dedup();
    if ids.is_empty() {
        return Err(AppError::invalid("no snippet named to delete"));
    }
    let mut deleted = Vec::new();
    for id in &ids {
        let snippet: Option<Snippet> = app.store().get(COLLECTION, id)?;
        let Some(snippet) = snippet else {
            return Err(AppError::not_found(format!("snippet `{id}`")));
        };
        app.store().delete(COLLECTION, id)?;
        app.audit(
            "terminal.snippet.delete",
            None,
            format!("Deleted the terminal snippet `{}`", snippet.name),
            json!({"id": id}),
        );
        deleted.push(id.clone());
    }
    app.publish("terminal.snippets.changed", &json!({"deleted": deleted}));
    Ok(DeleteResult {
        deleted: deleted.len(),
    })
}

/// A stored snippet with its values put in: what both `render` and `send`
/// work from, so that what is sent is rendered by the very code that
/// rendered what was shown.
struct Filled {
    snippet: Snippet,
    variables: Vec<Variable>,
    rendered: Rendered,
}

fn fill(app: &App, id: &str, values: Value) -> AppResult<Filled> {
    let values = &values_of(values)?;
    let snippet: Snippet = app.store().require(COLLECTION, id, "snippet")?;
    let name = &snippet.name;
    let template = Template::parse(&snippet.text).map_err(|malformed| {
        AppError::invalid(format!(
            "the snippet `{name}` cannot be used as it is ({malformed}); it was saved before placeholders were checked. Open it, correct it and save it again."
        ))
        .with_details(json!({"line": malformed.line, "column": malformed.column}))
    })?;
    let rendered = template.render(values).map_err(|err| match err {
        RenderError::Values(problems) => {
            let says: Vec<&str> = problems
                .iter()
                .map(|problem| problem.says.as_str())
                .collect();
            AppError::invalid(format!("`{name}` cannot be filled in: {}", says.join("; ")))
                .with_details(json!({"variables": problems}))
        }
        RenderError::TooLong(bytes) => AppError::invalid(format!(
            "`{name}` would be {bytes} bytes with these values; what is typed is at most {} bytes",
            template::MAX_RENDERED
        )),
    })?;
    Ok(Filled {
        snippet,
        variables: template.variables,
        rendered,
    })
}

fn render(app: &App, params: RenderParams) -> AppResult<RenderResult> {
    let filled = fill(app, &params.id, params.values)?;
    Ok(RenderResult {
        sha256: filled.rendered.sha256(),
        lines: filled.rendered.lines,
        text: filled.rendered.text,
        variables: filled.variables,
    })
}

pub(crate) fn register(router: &mut Router) {
    router.add(
        "terminal.snippets.list",
        |app: App, params: ListParams| async move { list(&app, params) },
    );
    router.add(
        "terminal.snippets.save",
        |app: App, params: SaveParams| async move { save(&app, params) },
    );
    router.add(
        "terminal.snippets.delete",
        |app: App, params: DeleteParams| async move { delete(&app, params) },
    );
    router.add(
        "terminal.snippets.render",
        |app: App, params: RenderParams| async move { render(&app, params) },
    );
    router.add(
        "terminal.snippets.send",
        |app: App, params: send::SendParams| async move { send::send(&app, params).await },
    );
    router.add(
        "terminal.snippets.export",
        |app: App, _params: NoParams| async move { transfer::export(&app) },
    );
    router.add(
        "terminal.snippets.import",
        |app: App, params: transfer::ImportParams| async move { transfer::import(&app, params) },
    );
}
