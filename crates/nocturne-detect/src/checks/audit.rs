//! The detections decided from the audit trail: `unredacted_config_views`,
//! `secrets_changed` and `failed_unlocks`.
//!
//! The trail has no actor. Nocturne has one operator identity per instance,
//! so none of these can say who — only what was done, how often, and which
//! audit entries say so.

use serde_json::{Value, json};

use nocturne_app::App;
use nocturne_app::store::{LogEntry, LogQuery};

use super::compact;
use crate::catalogue::{FAILED_UNLOCKS, SECRETS_CHANGED, UNREDACTED_CONFIG_VIEWS};
use crate::hits::{self, Wanted};
use crate::model::{Finding, Severity};
use crate::settings::{Settings, Threshold};
use crate::sources::Gap;

pub const VIEW_UNREDACTED: &str = "config.view_unredacted";
pub const CREDENTIAL_SAVE: &str = "credential.save";
pub const CREDENTIAL_DELETE: &str = "credential.delete";
pub const VAULT_SET_PASSWORD: &str = "vault.set_password";
pub const VAULT_WRONG_PASSWORD: &str = "vault.wrong_password";
pub const SERVER_TOKEN_REFUSED: &str = "server.token_refused";

/// The audit actions any of these detections reads.
pub fn is_watched(action: &str) -> bool {
    matches!(
        action,
        VIEW_UNREDACTED
            | CREDENTIAL_SAVE
            | CREDENTIAL_DELETE
            | VAULT_SET_PASSWORD
            | VAULT_WRONG_PASSWORD
            | SERVER_TOKEN_REFUSED
    )
}

const TRAIL: &str = "the audit trail";
const NO_USERS: &str = "Nocturne has one operator identity, not users, so this cannot say who.";
/// The most audit entries one window is read as. A window with more of one
/// action than this is over any threshold many times already.
const WINDOW_CAP: u32 = 5_000;
/// How many entry ids a hit names. The count is exact either way.
const NAMED: usize = 50;

/// The entries of one action in a window, oldest first.
fn entries(app: &App, action: &str, since: i64, until: i64) -> Result<Vec<LogEntry>, Gap> {
    let mut entries: Vec<LogEntry> = app
        .store()
        .log_query(&LogQuery {
            stream: Some("audit".into()),
            since: Some(since),
            until: Some(until),
            // The store matches a substring of the action; the exact action
            // is checked below.
            text: Some(action.to_string()),
            limit: Some(WINDOW_CAP),
            ..LogQuery::default()
        })
        .map_err(|err| Gap::failed(TRAIL, err.message))?
        .into_iter()
        .filter(|entry| entry.source == action)
        .collect();
    entries.reverse();
    Ok(entries)
}

/// The newest audit entry an earlier hit already counted.
///
/// A threshold that has been reported starts again from nothing: the sixth
/// view after a hit on the fifth is the first of the next five, not another
/// hit. This is read from the stored hits rather than remembered, so a
/// restart does not report the same entries twice.
fn already_counted(app: &App, detection: &str, what: Option<&str>, since: i64) -> Result<i64, Gap> {
    let earlier = hits::recent(
        app,
        &Wanted {
            detection: Some(detection),
            since: Some(since),
            limit: 100,
            ..Wanted::default()
        },
    )
    .map_err(|err| Gap::failed("the stored hits", err.message))?;
    Ok(earlier
        .iter()
        .filter(|hit| what.is_none_or(|what| hit.evidence["what"] == what))
        .filter_map(|hit| hit.evidence["through_audit_id"].as_i64())
        .max()
        .unwrap_or(0))
}

fn ids(entries: &[&LogEntry]) -> Vec<i64> {
    // The newest are the ones that tipped it over.
    entries
        .iter()
        .rev()
        .take(NAMED)
        .map(|entry| entry.id)
        .collect()
}

/// `unredacted_config_views`: `<n> unredacted configuration views in <m>
/// minutes`.
pub fn unredacted_views(
    app: &App,
    settings: &Settings,
    entry: &LogEntry,
) -> Result<Option<Finding>, Gap> {
    let threshold = settings.unredacted_views;
    let since = entry.ts - threshold.window_ms();
    let counted = already_counted(app, UNREDACTED_CONFIG_VIEWS, None, since)?;
    let views = entries(app, VIEW_UNREDACTED, since, entry.ts)?;
    let fresh: Vec<&LogEntry> = views.iter().filter(|view| view.id > counted).collect();
    if (fresh.len() as u64) < u64::from(threshold.count) {
        return Ok(None);
    }
    let mut devices: Vec<&str> = fresh
        .iter()
        .filter_map(|view| view.device_id.as_deref())
        .collect();
    devices.sort_unstable();
    devices.dedup();
    Ok(Some(Finding {
        detection: UNREDACTED_CONFIG_VIEWS,
        at: entry.ts,
        // It is about the reader, not about any one device.
        device_id: None,
        severity: Severity::Medium,
        title: format!(
            "{} unredacted configuration views in {} minutes",
            fresh.len(),
            threshold.window_min
        ),
        evidence: json!({
            "decided_from": "audit",
            "action": VIEW_UNREDACTED,
            "count": fresh.len(),
            "window_min": threshold.window_min,
            "audit_entry_ids": ids(&fresh),
            "through_audit_id": fresh.last().map(|view| view.id),
            "device_ids": devices,
            "who": NO_USERS,
        }),
        key: "views".into(),
        detail: format!(
            "Decided from the audit trail: every view of a configuration with its secrets showing is recorded. {NO_USERS}"
        ),
    }))
}

/// `secrets_changed`: a credential was saved or deleted, or the master
/// password was set, changed or removed.
pub fn secrets_changed(entry: &LogEntry) -> Option<Finding> {
    let (title, severity, key) = match entry.source.as_str() {
        // Which credential is in the audit entry the evidence names.
        CREDENTIAL_SAVE => {
            let body = entry.body.as_ref();
            let wrote = |part: &str| {
                body.and_then(|body| body[part].as_array())
                    .is_some_and(|names| !names.is_empty())
            };
            match body.and_then(|body| body["change"].as_str()) {
                // Somebody setting the application up. It is in the audit
                // trail; it is not something to be told about. Before this
                // was told apart every first device anybody added came with
                // an alert.
                Some("created") => return None,
                Some("updated")
                    if wrote("secrets_replaced")
                        || wrote("secrets_removed")
                        || wrote("secrets_added") =>
                {
                    (
                        "The secrets of a stored credential were changed".to_string(),
                        Severity::Info,
                        "credential_saved",
                    )
                }
                // A name, a user or a note was edited: no secret moved.
                Some("updated") => return None,
                // An entry that does not say what the save was (written by
                // a version before it did) is not guessed at.
                _ => return None,
            }
        }
        CREDENTIAL_DELETE => (
            "A credential was deleted".to_string(),
            Severity::Info,
            "credential_deleted",
        ),
        VAULT_SET_PASSWORD => {
            let change = entry
                .body
                .as_ref()
                .and_then(|body| body["change"].as_str())
                .unwrap_or_default();
            match change {
                // Without it the key file holds the vault key in the clear:
                // the one change here that weakens something.
                "removed" => (
                    "The vault's master password was removed".to_string(),
                    Severity::High,
                    "master_password",
                ),
                "changed" => (
                    "The vault's master password was changed".to_string(),
                    Severity::Info,
                    "master_password",
                ),
                "set" => (
                    "The vault's master password was set".to_string(),
                    Severity::Info,
                    "master_password",
                ),
                // Asked to remove a password that was not there: nothing
                // about the vault's secrets changed.
                "unchanged" => return None,
                // An entry that does not say which is not turned into one
                // of the three.
                _ => (
                    "The vault's master password was set, changed or removed".to_string(),
                    Severity::Info,
                    "master_password",
                ),
            }
        }
        _ => return None,
    };
    Some(Finding {
        detection: SECRETS_CHANGED,
        at: entry.ts,
        device_id: None,
        severity,
        title,
        evidence: json!({
            "decided_from": "audit",
            "action": entry.source,
            "audit_entry_id": entry.id,
            "who": NO_USERS,
        }),
        key: key.into(),
        detail: format!("Decided from the audit trail. {NO_USERS}"),
    })
}

/// How many attempts one audit entry stands for.
fn attempts(entry: &LogEntry) -> u64 {
    match entry.source.as_str() {
        VAULT_WRONG_PASSWORD => 1,
        // The server folds a minute of refused requests into one entry.
        // Only the ones that tried a token are guesses.
        SERVER_TOKEN_REFUSED => entry
            .body
            .as_ref()
            .and_then(|body| body["bad_token"].as_u64())
            .unwrap_or(0),
        _ => 0,
    }
}

/// `failed_unlocks`: repeated wrong vault passwords, or repeated requests to
/// the server with a wrong token. The two are counted apart: five of one and
/// none of the other is a finding, two of each is not.
pub fn failed_unlocks(
    app: &App,
    settings: &Settings,
    entry: &LogEntry,
) -> Result<Option<Finding>, Gap> {
    let (what, action) = match entry.source.as_str() {
        VAULT_WRONG_PASSWORD => ("vault", VAULT_WRONG_PASSWORD),
        SERVER_TOKEN_REFUSED => ("server", SERVER_TOKEN_REFUSED),
        _ => return Ok(None),
    };
    let threshold: Threshold = settings.failed_unlocks;
    let since = entry.ts - threshold.window_ms();
    let counted = already_counted(app, FAILED_UNLOCKS, Some(what), since)?;
    let refused = entries(app, action, since, entry.ts)?;
    let fresh: Vec<&LogEntry> = refused
        .iter()
        .filter(|refusal| refusal.id > counted && attempts(refusal) > 0)
        .collect();
    let count: u64 = fresh.iter().map(|refusal| attempts(refusal)).sum();
    if count < u64::from(threshold.count) {
        return Ok(None);
    }
    let (title, detail) = match what {
        "vault" => (
            format!(
                "{count} wrong vault passwords in {} minutes",
                threshold.window_min
            ),
            "Decided from the audit trail: every wrong master password is recorded, never the password that was tried.",
        ),
        _ => (
            format!(
                "{count} requests to the server with a wrong access token in {} minutes",
                threshold.window_min
            ),
            "Decided from the audit trail: the server records the requests it refused, a minute at a time, never the token that was tried.",
        ),
    };
    // Where the requests came from, when the server said so. Behind a
    // reverse proxy this is the proxy.
    let mut from: Vec<&str> = fresh
        .iter()
        .filter_map(|refusal| refusal.body.as_ref())
        .filter_map(|body| body["from"].as_array())
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    from.sort_unstable();
    from.dedup();
    Ok(Some(Finding {
        detection: FAILED_UNLOCKS,
        at: entry.ts,
        device_id: None,
        severity: Severity::High,
        title,
        evidence: compact(json!({
            "decided_from": "audit",
            "what": what,
            "action": action,
            "count": count,
            "window_min": threshold.window_min,
            "audit_entry_ids": ids(&fresh),
            "through_audit_id": fresh.last().map(|refusal| refusal.id),
            "from": (!from.is_empty()).then_some(from),
        })),
        key: what.into(),
        detail: detail.into(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(action: &str, body: Option<Value>) -> LogEntry {
        LogEntry {
            id: 7,
            ts: 1_763_000_000_000,
            stream: "audit".into(),
            device_id: None,
            severity: Some(6),
            source: action.into(),
            message: "…".into(),
            body,
        }
    }

    #[test]
    fn a_save_is_a_hit_only_when_it_says_a_stored_credentials_secret_moved() {
        let save = |body: Option<Value>| secrets_changed(&entry(CREDENTIAL_SAVE, body));
        let said = |change: &str, added: &[&str], replaced: &[&str], removed: &[&str]| {
            Some(json!({
                "change": change, "secrets_added": added,
                "secrets_replaced": replaced, "secrets_removed": removed,
            }))
        };
        for moved in [
            said("updated", &[], &["password"], &[]),
            said("updated", &["enable"], &[], &[]),
            said("updated", &[], &[], &["snmp_community"]),
        ] {
            let finding = save(moved.clone()).unwrap_or_else(|| panic!("{moved:?} is a hit"));
            assert_eq!(
                finding.title,
                "The secrets of a stored credential were changed"
            );
            assert_eq!(finding.severity, Severity::Info);
        }
        // A new credential, an edit that moved no secret, and an entry from
        // before the trail said which it was: none is guessed into a hit.
        for quiet in [
            said("created", &["password"], &[], &[]),
            said("updated", &[], &[], &[]),
            Some(json!({"change": "updated"})),
            Some(json!({})),
            None,
        ] {
            assert!(save(quiet.clone()).is_none(), "{quiet:?} is not a hit");
        }
    }

    #[test]
    fn only_a_removed_master_password_is_more_than_information() {
        for (change, severity, word) in [
            ("set", Severity::Info, "was set"),
            ("changed", Severity::Info, "was changed"),
            ("removed", Severity::High, "was removed"),
        ] {
            let finding =
                secrets_changed(&entry(VAULT_SET_PASSWORD, Some(json!({"change": change}))))
                    .unwrap();
            assert_eq!(finding.severity, severity, "{change}");
            assert!(finding.title.ends_with(word), "{}", finding.title);
            assert_eq!(finding.key, "master_password");
        }
    }

    #[test]
    fn removing_a_master_password_that_was_not_there_changed_no_secret() {
        assert_eq!(
            secrets_changed(&entry(
                VAULT_SET_PASSWORD,
                Some(json!({"change": "unchanged"}))
            )),
            None
        );
    }

    #[test]
    fn an_entry_that_does_not_say_what_happened_to_the_password_is_not_guessed_at() {
        let finding = secrets_changed(&entry(VAULT_SET_PASSWORD, None)).unwrap();
        assert_eq!(finding.severity, Severity::Info);
        assert_eq!(
            finding.title,
            "The vault's master password was set, changed or removed"
        );
    }

    #[test]
    fn a_request_with_no_token_is_not_a_guess() {
        let refused = entry(
            SERVER_TOKEN_REFUSED,
            Some(json!({"count": 9, "bad_token": 2, "missing_token": 7})),
        );
        assert_eq!(attempts(&refused), 2);
        assert_eq!(attempts(&entry(VAULT_WRONG_PASSWORD, None)), 1);
        assert_eq!(attempts(&entry(SERVER_TOKEN_REFUSED, None)), 0);
        assert_eq!(attempts(&entry("credential.save", None)), 0);
    }
}
