//! Taking the snippets out as one document, and taking such a document in.
//!
//! An export is for a colleague or a backup, so it holds what a person
//! wrote — name, text, device type, tags — and nothing that belongs to this
//! installation (ids, timestamps).
//!
//! An import is the opposite case: a file from somewhere else, which is to
//! say hostile input. It is judged whole before anything is written, by the
//! same rules a snippet typed into the form passes, and it is taken whole or
//! not at all: one entry that fails means nothing is imported, and the
//! answer says which entry and why. What does get written is written in one
//! transaction.

use std::collections::{HashMap, HashSet};

use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppError, AppResult, profiles};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::template::unshowable;
use super::{COLLECTION, Draft, Library, MAX_NAME, MAX_SNIPPETS, Snippet, check, key_of};

/// What an export says it is, and the only thing an import takes.
pub(super) const FORMAT: &str = "nocturne.snippets/1";
/// Largest document an import reads, as JSON text. Two thousand snippets of
/// two kilobytes each; a library is far smaller.
const MAX_DOCUMENT: usize = 4 * 1024 * 1024;
/// How much of an entry's name a problem repeats.
const SHOWN_NAME: usize = 80;

#[derive(Debug, Serialize)]
pub(super) struct Exported {
    format: &'static str,
    snippets: Vec<Entry>,
}

/// A snippet as it travels: what a person wrote, and nothing else.
#[derive(Debug, Serialize)]
struct Entry {
    name: String,
    text: String,
    /// The device type it is for; `null` means any.
    profile: Option<String>,
    tags: Vec<String>,
}

pub(super) fn export(app: &App) -> AppResult<Exported> {
    let mut snippets: Vec<Snippet> = app.store().list(COLLECTION)?;
    snippets.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.profile.cmp(&right.profile))
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(Exported {
        format: FORMAT,
        snippets: snippets
            .into_iter()
            .map(|snippet| Entry {
                name: snippet.name,
                text: snippet.text,
                profile: snippet.profile,
                tags: snippet.tags,
            })
            .collect(),
    })
}

/// What to do with an entry when a snippet of its name, for its device
/// type, is already here and says something else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum OnConflict {
    /// Keep the one that is here. The default: it loses nothing.
    #[default]
    Skip,
    /// Put the entry's text and tags in its place.
    Replace,
    /// Keep both: the entry comes in as `name (2)`.
    Rename,
}

#[derive(Debug, Deserialize)]
pub(super) struct ImportParams {
    /// What `terminal.snippets.export` returned: the object, or its JSON text
    /// as read from a file.
    document: Value,
    #[serde(default)]
    on_conflict: OnConflict,
    /// Say what would happen and change nothing.
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Default, Serialize)]
pub(super) struct ImportResult {
    /// Snippets added, those added under a new name included.
    created: usize,
    replaced: usize,
    /// Entries left out because they are here already: a snippet of that
    /// name says the same (whatever the mode), an earlier `rename` brought
    /// the entry in as `name (n)`, or — with `skip` — one of that name
    /// exists at all.
    skipped: usize,
    /// How many of `created` came in under a new name (`rename`).
    renamed: usize,
    /// What is wrong with the document, entry by entry. While this is not
    /// empty nothing is imported and the counts are all zero.
    problems: Vec<Problem>,
    /// The snippets are now as this says: not a dry run, and no problems.
    applied: bool,
    dry_run: bool,
}

#[derive(Debug, Serialize)]
struct Problem {
    /// The entry's position in `snippets`, from 0.
    index: usize,
    /// The entry's name, when it has one that can be shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    problem: String,
}

/// Text from the document in a form that can be repeated back: short, and
/// with nothing in it that would not show.
fn shown(text: &str) -> String {
    text.trim()
        .chars()
        .take(SHOWN_NAME)
        .map(|ch| {
            if unshowable(ch).is_some() {
                '\u{fffd}'
            } else {
                ch
            }
        })
        .collect()
}

/// The document as a JSON value, however it was handed over.
fn document(given: Value) -> AppResult<Value> {
    let too_large = |bytes: usize| {
        AppError::invalid(format!(
            "the document is {bytes} bytes; an import reads at most {MAX_DOCUMENT} ({} MiB)",
            MAX_DOCUMENT / (1024 * 1024)
        ))
    };
    match given {
        Value::String(text) => {
            if text.len() > MAX_DOCUMENT {
                return Err(too_large(text.len()));
            }
            // A file saved by an editor on Windows starts with a byte-order
            // mark, which is not JSON and not the document either.
            let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
            serde_json::from_str(text).map_err(|err| {
                AppError::invalid(format!(
                    "the document is not JSON ({err}); an import takes what `terminal.snippets.export` returned"
                ))
            })
        }
        Value::Object(_) => {
            let bytes = serde_json::to_vec(&given)?.len();
            if bytes > MAX_DOCUMENT {
                return Err(too_large(bytes));
            }
            Ok(given)
        }
        _ => Err(AppError::invalid(
            "`document` is what `terminal.snippets.export` returned: that object, or its JSON text",
        )),
    }
}

/// One entry of the document, read without trusting its shape.
fn read_entry(value: &Value) -> Result<(String, String, Option<String>, Vec<String>), String> {
    let Some(entry) = value.as_object() else {
        return Err("is not an object with `name` and `text`".to_string());
    };
    let text_of = |field: &str| match entry.get(field) {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(format!("`{field}` is not text")),
        None => Err(format!("has no `{field}`")),
    };
    let name = text_of("name")?;
    let text = text_of("text")?;
    let profile = match entry.get("profile") {
        None | Some(Value::Null) => None,
        Some(Value::String(profile)) => Some(profile.clone()),
        Some(_) => return Err("`profile` is neither a device type nor null".to_string()),
    };
    let tags = match entry.get("tags") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(tags)) => tags
            .iter()
            .map(|tag| tag.as_str().map(str::to_string))
            .collect::<Option<Vec<String>>>()
            .ok_or_else(|| "`tags` is not a list of words".to_string())?,
        Some(_) => return Err("`tags` is not a list of words".to_string()),
    };
    Ok((name, text, profile, tags))
}

/// What becomes of one entry.
enum Step {
    Create { draft: Draft, renamed: bool },
    Replace { id: String, draft: Draft },
    Skip,
}

/// `name (n)`, with the name cut short when the two would not fit.
fn numbered(name: &str, n: usize) -> String {
    let suffix = format!(" ({n})");
    let room = MAX_NAME.saturating_sub(suffix.chars().count());
    let base: String = name.chars().take(room).collect();
    format!("{}{suffix}", base.trim_end())
}

/// The first free `name (n)` for a device type.
fn free_name(
    name: &str,
    profile: Option<&str>,
    taken: &HashSet<(String, Option<String>)>,
) -> String {
    (2usize..)
        .map(|n| numbered(name, n))
        .find(|candidate| !taken.contains(&key_of(candidate, profile)))
        .unwrap_or_else(|| name.to_string())
}

/// Whether an earlier import with `rename` already brought this entry in:
/// one of the `name (n)` it would have been given is here and says the same,
/// word for word. Without this look a document imported twice that way would
/// leave `name (2)` and `name (3)` behind, alike in everything but the number.
fn already_renamed(draft: &Draft, here: &HashMap<(String, Option<String>), Vec<&Snippet>>) -> bool {
    // Each step needs a snippet of that name to be here, so this ends.
    for n in 2usize.. {
        let key = key_of(&numbered(&draft.name, n), draft.profile.as_deref());
        match here.get(&key) {
            Some(twins) if twins.iter().any(|twin| says_the_same(twin, draft)) => return true,
            Some(_) => {}
            None => return false,
        }
    }
    false
}

fn says_the_same(snippet: &Snippet, draft: &Draft) -> bool {
    snippet.text == draft.text && snippet.tags == draft.tags
}

pub(super) fn import(app: &App, params: ImportParams) -> AppResult<ImportResult> {
    let document = document(params.document)?;
    if document.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(AppError::invalid(format!(
            "this is not a Nocturne snippets document: its `format` is not `{FORMAT}`"
        )));
    }
    let Some(entries) = document.get("snippets").and_then(Value::as_array) else {
        return Err(AppError::invalid(
            "the document has no `snippets` list; an import takes what `terminal.snippets.export` returned",
        ));
    };
    if entries.len() > MAX_SNIPPETS {
        return Err(AppError::invalid(format!(
            "the document holds {} snippets; Nocturne keeps at most {MAX_SNIPPETS}",
            entries.len()
        )));
    }

    // Every entry by the rules of the form, and no two of them the same.
    let mut problems = Vec::new();
    let mut drafts: Vec<(usize, Draft)> = Vec::new();
    let mut seen: HashSet<(String, Option<String>)> = HashSet::new();
    for (index, value) in entries.iter().enumerate() {
        let name = value.get("name").and_then(Value::as_str).map(shown);
        let checked = read_entry(value).and_then(|(name, text, profile, tags)| {
            if let Some(profile) = profile.as_deref().map(str::trim)
                && !profile.is_empty()
                && profiles::platform(app, profile).is_err()
            {
                return Err(format!(
                    "is for the device type `{}`, which this installation does not have; add that device type first, or make the snippet one for any device",
                    shown(profile)
                ));
            }
            check(app, &name, text, profile.as_deref(), tags)
                .map(|(draft, _)| draft)
                .map_err(|err| err.message)
        });
        match checked {
            Ok(draft) => {
                if seen.insert(key_of(&draft.name, draft.profile.as_deref())) {
                    drafts.push((index, draft));
                } else {
                    problems.push(Problem {
                        index,
                        name,
                        problem:
                            "has the same name and device type as an earlier entry of this document"
                                .to_string(),
                    });
                }
            }
            Err(problem) => problems.push(Problem {
                index,
                name,
                problem,
            }),
        }
    }

    // Against what is here already — which stays as it is until this import
    // has been written: no save may slip in between the look and the writing.
    let library = app.state::<Library>();
    let _writing = library.writing.lock();
    let existing: Vec<Snippet> = app.store().list(COLLECTION)?;
    let mut here: HashMap<(String, Option<String>), Vec<&Snippet>> = HashMap::new();
    for snippet in &existing {
        here.entry(snippet.key()).or_default().push(snippet);
    }
    // Names nothing new may take: those here, and those the document brings.
    let mut taken: HashSet<(String, Option<String>)> = here.keys().cloned().collect();
    taken.extend(seen);

    let mut steps = Vec::new();
    for (index, draft) in drafts {
        let key = key_of(&draft.name, draft.profile.as_deref());
        let twins = here.get(&key).map(Vec::as_slice).unwrap_or_default();
        let step = if twins.is_empty() {
            Step::Create {
                draft,
                renamed: false,
            }
        } else if twins.iter().any(|twin| says_the_same(twin, &draft)) {
            // Already here, word for word: importing a document twice adds
            // nothing, whatever the mode.
            Step::Skip
        } else {
            match params.on_conflict {
                OnConflict::Skip => Step::Skip,
                OnConflict::Replace if twins.len() == 1 => Step::Replace {
                    id: twins[0].id.clone(),
                    draft,
                },
                OnConflict::Replace => {
                    problems.push(Problem {
                        index,
                        name: Some(shown(&draft.name)),
                        problem: format!(
                            "{} snippets here already have this name and device type, so which one to replace is not clear; rename or delete one of them first",
                            twins.len()
                        ),
                    });
                    continue;
                }
                OnConflict::Rename if already_renamed(&draft, &here) => Step::Skip,
                OnConflict::Rename => {
                    let name = free_name(&draft.name, draft.profile.as_deref(), &taken);
                    taken.insert(key_of(&name, draft.profile.as_deref()));
                    Step::Create {
                        draft: Draft { name, ..draft },
                        renamed: true,
                    }
                }
            }
        };
        steps.push(step);
    }

    let mut result = ImportResult {
        dry_run: params.dry_run,
        ..ImportResult::default()
    };
    if !problems.is_empty() {
        problems.sort_by_key(|problem| problem.index);
        result.problems = problems;
        return Ok(result);
    }
    for step in &steps {
        match step {
            Step::Create { renamed, .. } => {
                result.created += 1;
                result.renamed += usize::from(*renamed);
            }
            Step::Replace { .. } => result.replaced += 1,
            Step::Skip => result.skipped += 1,
        }
    }
    if existing.len() + result.created > MAX_SNIPPETS {
        return Err(AppError::conflict(format!(
            "there are {} snippets and this would add {}; Nocturne keeps at most {MAX_SNIPPETS}. Delete some first, or import fewer.",
            existing.len(),
            result.created
        )));
    }
    if params.dry_run {
        return Ok(result);
    }

    let now = now_ms();
    let mut written: Vec<(String, String)> = Vec::new();
    for step in steps {
        let snippet = match step {
            Step::Skip => continue,
            Step::Create { draft, .. } => Snippet {
                id: new_id(),
                name: draft.name,
                text: draft.text,
                profile: draft.profile,
                tags: draft.tags,
                created_at: now,
                updated_at: now,
            },
            Step::Replace { id, draft } => {
                let Some(old) = existing.iter().find(|snippet| snippet.id == id) else {
                    continue;
                };
                Snippet {
                    id,
                    name: draft.name,
                    text: draft.text,
                    // The device type as it is written here already: the two
                    // differ at most in case.
                    profile: old.profile.clone(),
                    tags: draft.tags,
                    created_at: old.created_at,
                    updated_at: now,
                }
            }
        };
        written.push((snippet.id.clone(), serde_json::to_string(&snippet)?));
    }
    if !written.is_empty() {
        // All of them or none: the same statement the store's own `put`
        // runs, inside one transaction, which `put` cannot be.
        app.store().transaction(|tx| {
            let mut statement = tx.prepare(
                "INSERT INTO docs (collection, id, body, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT (collection, id) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
            )?;
            for (id, body) in &written {
                statement.execute(params![COLLECTION, id, body, now])?;
            }
            Ok(())
        })?;
    }
    result.applied = true;

    if !written.is_empty() {
        app.audit(
            "terminal.snippet.import",
            None,
            format!(
                "Imported terminal snippets: {} added, {} replaced, {} left as they were",
                result.created, result.replaced, result.skipped
            ),
            json!({
                "created": result.created,
                "replaced": result.replaced,
                "skipped": result.skipped,
                "renamed": result.renamed,
                "on_conflict": params.on_conflict,
            }),
        );
        let ids: Vec<&str> = written.iter().map(|(id, _)| id.as_str()).collect();
        app.publish("terminal.snippets.changed", &json!({"saved": ids}));
    }
    Ok(result)
}
