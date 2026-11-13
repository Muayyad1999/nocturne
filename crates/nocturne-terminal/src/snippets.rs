//! Saved commands.
//!
//! A snippet is a piece of text an engineer keeps around to paste into a
//! terminal — a block of configuration, a troubleshooting sequence, the
//! incantation nobody remembers. They are small and few, so they live as
//! ordinary documents.

use nocturne_app::util::{matches_query, new_id, now_ms};
use nocturne_app::{App, AppError, AppResult, Router, profiles};
use serde::{Deserialize, Serialize};
use serde_json::json;

const COLLECTION: &str = "terminal_snippets";
/// Longest snippet kept. Anything larger is a file, not a snippet.
const MAX_TEXT: usize = 256 * 1024;
const MAX_NAME: usize = 200;
/// Most snippets one installation keeps, so the list stays a list.
const MAX_SNIPPETS: usize = 2000;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Snippet {
    pub id: String,
    pub name: String,
    /// The text, exactly as it will be typed.
    pub text: String,
    /// The device profile this is for (`cisco_ios`); `None` means any.
    pub profile: Option<String>,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ListParams {
    /// Only snippets for this profile (and those for any profile).
    profile: Option<String>,
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

fn clean_tags(tags: Vec<String>) -> Vec<String> {
    let mut tags: Vec<String> = tags
        .into_iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect();
    tags.sort();
    tags.dedup();
    tags.truncate(32);
    tags
}

fn list(app: &App, filter: ListParams) -> AppResult<Vec<Snippet>> {
    let mut snippets: Vec<Snippet> = app.store().list(COLLECTION)?;
    if let Some(profile) = filter.profile.as_deref().map(str::trim)
        && !profile.is_empty()
    {
        snippets.retain(|snippet| {
            snippet
                .profile
                .as_deref()
                .is_none_or(|wanted| wanted.eq_ignore_ascii_case(profile))
        });
    }
    if let Some(query) = filter.query.as_deref().map(str::trim)
        && !query.is_empty()
    {
        snippets.retain(|snippet| {
            matches_query(&snippet.name, query)
                || matches_query(&snippet.text, query)
                || snippet.tags.iter().any(|tag| matches_query(tag, query))
        });
    }
    snippets.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(snippets)
}

fn save(app: &App, params: SaveParams) -> AppResult<Snippet> {
    let name = params.name.trim();
    if name.is_empty() {
        return Err(AppError::invalid("a snippet needs a name"));
    }
    if name.chars().count() > MAX_NAME {
        return Err(AppError::invalid(format!(
            "the name of a snippet is at most {MAX_NAME} characters"
        )));
    }
    if params.text.is_empty() {
        return Err(AppError::invalid(format!(
            "the snippet `{name}` has no text; there would be nothing to paste"
        )));
    }
    if params.text.len() > MAX_TEXT {
        return Err(AppError::invalid(format!(
            "`{name}` is {} bytes; a snippet holds at most {MAX_TEXT}. Keep larger text in a file and send it with SFTP.",
            params.text.len()
        )));
    }
    let profile = match params.profile.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(profile) => {
            // Saying it is for a profile that does not exist would hide it
            // from every list that filters by one.
            profiles::platform(app, profile)?;
            Some(profile.to_string())
        }
    };

    let existing: Option<Snippet> = match params.id.as_deref() {
        Some(id) => Some(
            app.store()
                .get(COLLECTION, id)?
                .ok_or_else(|| AppError::not_found(format!("snippet `{id}`")))?,
        ),
        None => None,
    };
    if existing.is_none() && app.store().count(COLLECTION)? as usize >= MAX_SNIPPETS {
        return Err(AppError::conflict(format!(
            "there are already {MAX_SNIPPETS} snippets, which is the most Nocturne keeps; delete some first"
        )));
    }
    let now = now_ms();
    let snippet = Snippet {
        id: existing
            .as_ref()
            .map(|snippet| snippet.id.clone())
            .unwrap_or_else(new_id),
        name: name.to_string(),
        text: params.text,
        profile,
        tags: clean_tags(params.tags),
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
    Ok(snippet)
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
}
