//! One redactor per chat.
//!
//! Everything that comes back from an RPC call passes through here before any
//! tool handler sees it, so nothing a handler forgets — and no cap that cuts a
//! line in half — can leak a secret. Three layers:
//!
//! 1. `nocturne_ai::Redactor`'s built-in rules, plus the `config.secrets`
//!    patterns of every device type that is loaded;
//! 2. the values Nocturne itself knows: device passwords, enable secrets and
//!    SNMP secrets from the vault, and the model-provider API keys;
//! 3. every value that has ever been given a placeholder in this chat —
//!    which is what stops [`ChatRedactor::restore`] from becoming a way out:
//!    a secret the model puts back into a configuration line is replaced by
//!    its placeholder again the moment it comes back from any tool.
//!
//! Placeholders are stable (`<secret:3>` is always the same value in one
//! chat), so the model can still reason about two devices sharing a key. The
//! table holds the secrets themselves and lives only in the vault.

use nocturne_ai::Redactor;
use nocturne_ai::redact::{RedactionTable, Restored};
use nocturne_app::error::AppResult;
use nocturne_app::inventory::DeviceFilter;
use nocturne_app::{App, credentials, inventory, profiles};
use parking_lot::Mutex;
use serde_json::Value;

/// Shortest value from the vault that is scrubbed wherever it appears.
/// Shorter ones are ordinary words far more often than they are secrets, and
/// hiding every occurrence of "pw" would make device output unreadable; the
/// pattern rules still catch them where they appear as a credential.
const MIN_KNOWN_LEN: usize = 6;

pub struct ChatRedactor {
    redactor: Redactor,
    known: Mutex<Vec<String>>,
}

impl ChatRedactor {
    /// A redactor for `chat_id`, with the device types' own secret patterns,
    /// the vault's known values and whatever placeholders this chat already
    /// handed out.
    pub fn open(app: &App, chat_id: &str) -> ChatRedactor {
        let mut patterns: Vec<String> = Vec::new();
        for platform in profiles::registry(app).platforms() {
            for pattern in &platform.profile.config.secrets {
                if !patterns.contains(pattern) {
                    patterns.push(pattern.clone());
                }
            }
        }
        // A profile with a broken pattern must not disable redaction: fall
        // back to the built-in rules alone.
        let redactor = Redactor::with_rules(&patterns).unwrap_or_else(|err| {
            tracing::warn!(%err, "a device type's secret pattern does not compile; using the built-in rules only");
            Redactor::new()
        });
        let this = ChatRedactor {
            redactor,
            known: Mutex::new(Vec::new()),
        };
        match load_table(app, chat_id) {
            Some(table) => {
                for (_, value) in &table.entries {
                    this.remember(value);
                }
                this.redactor.import(table);
            }
            // A chat that has not run yet learns the secrets of the network
            // from the configurations Nocturne has already backed up (see
            // `prime_from_backups`).
            None => this.prime_from_backups(app),
        }
        this.load_vault_values(app);
        this
    }

    /// Learn the network's secrets from the stored configurations.
    ///
    /// A secret only has to look like a secret *once*. `snmp-server community
    /// X RO` in a backup is unmistakable; the same value in a log line
    /// ("Community Index: X"), in an error message or in a `show` output that
    /// no pattern covers is not. Reading the backups first means a value that
    /// was ever recognised anywhere is hidden everywhere afterwards, whatever
    /// order the tools are called in.
    ///
    /// It costs one pass over the stored configurations, once per chat (the
    /// table is then kept in the vault), bounded by the two limits below.
    fn prime_from_backups(&self, app: &App) {
        const MAX_DEVICES: usize = 250;
        const MAX_BYTES: usize = 8 * 1024 * 1024;
        let Ok(devices) = inventory::list(app, &DeviceFilter::default()) else {
            return;
        };
        let mut budget = MAX_BYTES;
        for row in devices.iter().take(MAX_DEVICES) {
            for kind in ["running", "startup"] {
                let Ok(Some(version)) = app.store().config_latest(&row.device.id, kind) else {
                    continue;
                };
                let Ok((_, text)) = app.store().config_get(version.id) else {
                    continue;
                };
                if text.len() > budget {
                    return;
                }
                budget -= text.len();
                // The result is thrown away; what matters is the table.
                let _ = self.redactor.redact(&text);
            }
        }
    }

    /// A redactor with no app behind it (used where only the rules matter).
    pub fn bare() -> ChatRedactor {
        ChatRedactor {
            redactor: Redactor::new(),
            known: Mutex::new(Vec::new()),
        }
    }

    /// Learn the secrets Nocturne stores itself, so they are hidden wherever
    /// they turn up — in a configuration, in a log line, in an error message.
    fn load_vault_values(&self, app: &App) {
        let Ok(credentials) = credentials::list(app) else {
            return;
        };
        for credential in credentials {
            // A locked vault is not an error here: there is simply nothing to learn.
            let Ok(resolved) = credentials::resolve(app, &credential.id) else {
                continue;
            };
            for secret in [
                &resolved.password,
                &resolved.enable_secret,
                &resolved.passphrase,
                &resolved.snmp_community,
                &resolved.snmp_auth_password,
                &resolved.snmp_priv_password,
            ] {
                if let Some(value) = secret.as_ref() {
                    self.remember_if_long_enough(value);
                }
            }
            // The private key is matched by the built-in PEM rule as well, but
            // a fragment of it quoted in an error message would not be.
            if let Some(key) = resolved.private_key.as_ref() {
                for line in key.lines().filter(|line| line.len() >= 20) {
                    self.remember(line);
                }
            }
        }
    }

    /// Hide `value` wherever it appears from now on.
    pub fn remember(&self, value: &str) {
        let value = value.trim();
        if value.len() < 4 {
            return;
        }
        let mut known = self.known.lock();
        if !known.iter().any(|existing| existing == value) {
            known.push(value.to_string());
        }
    }

    fn remember_if_long_enough(&self, value: &str) {
        if value.trim().len() >= MIN_KNOWN_LEN {
            self.remember(value);
        }
    }

    /// Replace every secret in `text` with its placeholder.
    ///
    /// Three passes: the patterns, then every value Nocturne itself holds,
    /// then every value this chat has ever given a placeholder to. The last
    /// one is what makes the guarantee hold for text no pattern covers — a
    /// password quoted in a log line, a community string in a `show` output
    /// with an unfamiliar shape — and what stops a restored secret from
    /// coming back (see [`ChatRedactor::restore`]).
    ///
    /// This is for text that came from outside: a device, a stored record, an
    /// error message. Text Nocturne composed itself gets
    /// [`ChatRedactor::scrub_values`] instead, because the patterns are
    /// written for configuration and give odd results on prose that already
    /// holds placeholders.
    pub fn scrub(&self, text: &str) -> String {
        self.values(&self.redactor.redact(text))
    }

    /// The value passes only: for text Nocturne wrote itself (a tool's own
    /// wording, a summary, a message about an error) which may still quote
    /// something it read.
    pub fn scrub_values(&self, text: &str) -> String {
        self.values(text)
    }

    fn values(&self, text: &str) -> String {
        let mut out = text.to_string();
        // Cloned so that a `remember` from another turn cannot deadlock here.
        let known = self.known.lock().clone();
        for value in known {
            out = self.redactor.redact_value(&out, &value);
        }
        for (_, value) in self.redactor.export().entries {
            if scrubbable_anywhere(&value) {
                out = self.redactor.redact_value(&out, &value);
            }
        }
        out
    }

    /// Scrub every string inside a JSON value, keys included.
    pub fn scrub_json(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.scrub(text)),
            Value::Array(items) => Value::Array(items.iter().map(|i| self.scrub_json(i)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, item)| (self.scrub(key), self.scrub_json(item)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// Put the real values back in place of placeholders, and keep hiding
    /// them: a restored secret that comes back through any later tool result
    /// is turned into its placeholder again.
    pub fn restore(&self, text: &str) -> Restored {
        let restored = self.redactor.restore(text);
        for (_, value) in self.redactor.export().entries {
            if restored.text.contains(&value) {
                self.remember(&value);
            }
        }
        restored
    }

    pub fn has_placeholders(text: &str) -> bool {
        Redactor::has_placeholders(text)
    }

    /// Store the placeholder table so the chat can be continued after a
    /// restart. It contains the secrets themselves, so it goes to the vault
    /// and nowhere else.
    pub fn save(&self, app: &App, chat_id: &str) {
        let table = self.redactor.export();
        if table.entries.is_empty() {
            return;
        }
        if let Err(err) = store_table(app, chat_id, &table) {
            tracing::debug!(%err, "the chat's redaction table could not be stored");
        }
    }

    pub fn forget(app: &App, chat_id: &str) {
        let _ = app.vault().delete(&vault_id(chat_id));
    }
}

/// Is this value distinctive enough to hide wherever it appears, out of any
/// context?
///
/// The patterns are generous on purpose — they would rather mask a word than
/// miss a key — so the table also collects the odd ordinary word out of a
/// banner or a comment. Replacing *those* everywhere would shred the text the
/// model has to read, so the blanket pass is limited to values that look like
/// credentials: at least eight characters and at least two different kinds of
/// character. Values Nocturne knows are secrets (the vault, and anything it
/// has put back into a configuration) are hidden whatever they look like.
fn scrubbable_anywhere(value: &str) -> bool {
    if value.chars().count() < 8 {
        return false;
    }
    let kinds = [
        value.chars().any(|c| c.is_ascii_lowercase()),
        value.chars().any(|c| c.is_ascii_uppercase()),
        value.chars().any(|c| c.is_ascii_digit()),
        value.chars().any(|c| !c.is_ascii_alphanumeric()),
    ];
    kinds.into_iter().filter(|present| *present).count() >= 2
}

fn vault_id(chat_id: &str) -> String {
    format!("ai:chat:{chat_id}:redaction")
}

fn store_table(app: &App, chat_id: &str, table: &RedactionTable) -> AppResult<()> {
    app.vault()
        .put(&vault_id(chat_id), &serde_json::to_string(table)?)
}

fn load_table(app: &App, chat_id: &str) -> Option<RedactionTable> {
    let stored = app.vault().get(&vault_id(chat_id)).ok()??;
    serde_json::from_str(&stored).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restored_secret_is_hidden_again_wherever_it_comes_back() {
        let redactor = ChatRedactor::bare();
        let hidden = redactor.scrub("snmp-server community Sup3rS3cretRO RO");
        assert!(!hidden.contains("Sup3rS3cretRO"), "{hidden}");
        let token = hidden
            .split_whitespace()
            .nth(2)
            .expect("the placeholder")
            .to_string();

        // The model moves the secret into a line no rule would ever match.
        let restored = redactor.restore(&format!("description uplink {token}"));
        assert!(restored.text.contains("Sup3rS3cretRO"));
        assert!(restored.unresolved.is_empty());

        // Reading that line back gives the placeholder, not the secret.
        let read_back = redactor.scrub("interface Gi0/1\n description uplink Sup3rS3cretRO\n");
        assert!(!read_back.contains("Sup3rS3cretRO"), "{read_back}");
        assert!(read_back.contains(&token), "{read_back}");
    }

    #[test]
    fn placeholders_are_stable_and_json_is_scrubbed_everywhere() {
        let redactor = ChatRedactor::bare();
        redactor.remember("Vault0nlyValue");
        let scrubbed = redactor.scrub_json(&serde_json::json!({
            "config": "enable secret 9 $9$abcdefghijklmnop",
            "rows": [{"note": "the key is Vault0nlyValue", "Vault0nlyValue": 1}],
        }));
        let text = scrubbed.to_string();
        assert!(!text.contains("Vault0nlyValue"), "{text}");
        assert!(!text.contains("$9$abcdefghijklmnop"), "{text}");
        assert!(text.contains("<secret:"), "{text}");
        // The same value keeps the same placeholder.
        assert_eq!(
            redactor.scrub("Vault0nlyValue"),
            redactor.scrub("Vault0nlyValue")
        );
    }

    #[test]
    fn only_credential_shaped_values_are_hidden_out_of_context() {
        assert!(scrubbable_anywhere("Sup3rS3cretCommunity"));
        assert!(scrubbable_anywhere("0822455D0A16"));
        assert!(scrubbable_anywhere("sk-live-9f8e7d6c5b4a3210"));
        assert!(scrubbable_anywhere("$9$Hb4aLsj9k2nMxQw0pLm"));
        // Words a generous pattern picked out of a banner or a comment.
        assert!(!scrubbable_anywhere("community"));
        assert!(!scrubbable_anywhere("from"));
        assert!(!scrubbable_anywhere("4096>"));
        assert!(!scrubbable_anywhere("INSTRUCTIONS"));
    }

    #[test]
    fn short_known_values_are_not_scrubbed_but_long_ones_are() {
        let redactor = ChatRedactor::bare();
        redactor.remember_if_long_enough("pw");
        redactor.remember_if_long_enough("L0ngEnoughSecret");
        let text = redactor.scrub("login pw then L0ngEnoughSecret");
        assert!(text.contains("login pw then"), "{text}");
        assert!(!text.contains("L0ngEnoughSecret"), "{text}");
    }
}
