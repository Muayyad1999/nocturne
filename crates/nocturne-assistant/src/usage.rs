//! What the assistant has cost, per provider and model.
//!
//! Tokens are counted into hourly buckets: enough to answer "what did today
//! cost" and to draw a graph, small enough that a year of heavy use is a few
//! thousand rows.

use nocturne_ai::Usage;
use nocturne_app::error::AppResult;
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const COLLECTION: &str = "ai_usage";

const HOUR_MS: i64 = 3_600_000;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageRow {
    pub hour: i64,
    pub provider_id: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub turns: u64,
}

fn row_id(hour: i64, provider_id: &str, model: &str) -> String {
    format!("{hour}:{provider_id}:{model}")
}

/// Add one turn's tokens to the ledger.
pub fn record(app: &App, provider_id: &str, model: &str, usage: &Usage) -> AppResult<()> {
    let hour = now_ms() / HOUR_MS * HOUR_MS;
    let id = row_id(hour, provider_id, model);
    let mut row: UsageRow = app.store().get(COLLECTION, &id)?.unwrap_or(UsageRow {
        hour,
        provider_id: provider_id.to_string(),
        model: model.to_string(),
        ..UsageRow::default()
    });
    row.input_tokens += usage.input_tokens;
    row.output_tokens += usage.output_tokens;
    row.cache_read_tokens += usage.cache_read_tokens;
    row.cache_write_tokens += usage.cache_write_tokens;
    row.turns += 1;
    app.store().put(COLLECTION, &id, &row)
}

#[derive(Deserialize)]
struct UsageParams {
    /// Milliseconds since the epoch; everything from then on.
    #[serde(default)]
    since: Option<i64>,
}

pub fn register(builder: &mut AppBuilder) {
    builder
        .router
        .add("ai.usage", |app: App, params: UsageParams| async move {
            let since = params.since.unwrap_or(0);
            let rows: Vec<UsageRow> = app
                .store()
                .list::<UsageRow>(COLLECTION)?
                .into_iter()
                .filter(|row| row.hour >= since)
                .collect();
            let mut by: Vec<Value> = Vec::new();
            let mut totals = UsageRow::default();
            for row in &rows {
                totals.input_tokens += row.input_tokens;
                totals.output_tokens += row.output_tokens;
                totals.cache_read_tokens += row.cache_read_tokens;
                totals.cache_write_tokens += row.cache_write_tokens;
                totals.turns += row.turns;
                match by.iter_mut().find(|entry| {
                    entry["provider_id"] == json!(row.provider_id)
                        && entry["model"] == json!(row.model)
                }) {
                    Some(entry) => {
                        for field in [
                            "input_tokens",
                            "output_tokens",
                            "cache_read_tokens",
                            "cache_write_tokens",
                            "turns",
                        ] {
                            let add = match field {
                                "input_tokens" => row.input_tokens,
                                "output_tokens" => row.output_tokens,
                                "cache_read_tokens" => row.cache_read_tokens,
                                "cache_write_tokens" => row.cache_write_tokens,
                                _ => row.turns,
                            };
                            let current = entry[field].as_u64().unwrap_or_default();
                            entry[field] = json!(current + add);
                        }
                    }
                    None => by.push(json!({
                        "provider_id": row.provider_id, "model": row.model,
                        "input_tokens": row.input_tokens, "output_tokens": row.output_tokens,
                        "cache_read_tokens": row.cache_read_tokens,
                        "cache_write_tokens": row.cache_write_tokens, "turns": row.turns,
                    })),
                }
            }
            Ok(json!({
                "since": since,
                "totals": {
                    "input_tokens": totals.input_tokens, "output_tokens": totals.output_tokens,
                    "cache_read_tokens": totals.cache_read_tokens,
                    "cache_write_tokens": totals.cache_write_tokens, "turns": totals.turns,
                },
                "by": by,
                "hours": rows,
            }))
        });
}
