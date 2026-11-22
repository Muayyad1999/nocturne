//! Reading another feature's list to its end.
//!
//! The features this one asks (`monitor.interfaces`, `alerts.list`) answer
//! with at most a few thousand rows a call and page by offset: `cursor` is
//! the number of rows already received, and a page shorter than `limit` is
//! the last one. One call is therefore only the beginning of the table, and
//! what is in the rest of it looks exactly like what does not exist — a port
//! with no rate, a device with no alerts. Everything here reads to the end.

use nocturne_app::App;
use nocturne_app::error::{AppError, AppResult};
use serde_json::{Value, json};

/// More pages than any table this feature reads has rows for. An answer that
/// is still full after this many is not paging at all, and is refused rather
/// than followed for ever.
const MAX_PAGES: usize = 200;

/// The rows of an answer, whether it is a bare array or carries them in
/// `rows` — the two shapes list methods in this codebase answer with.
fn rows_of(answer: Value) -> Vec<Value> {
    match answer {
        Value::Array(rows) => rows,
        Value::Object(mut fields) => match fields.remove("rows") {
            Some(Value::Array(rows)) => rows,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// One call of `method`, or `None` when no feature in this build has it. Any
/// other refusal is the caller's to deal with.
pub async fn page(app: &App, method: &str, params: Value) -> AppResult<Option<Vec<Value>>> {
    match app.rpc(method, params).await {
        Ok(answer) => Ok(Some(rows_of(answer))),
        Err(err) if err.code == "unknown_method" => Ok(None),
        Err(err) => Err(err),
    }
}

/// Every row `method` has for `params`, `size` at a time, or `None` when no
/// feature in this build has the method.
///
/// `size` has to be within what the method allows: it clamps a larger limit
/// down, and a clamped page would then be shorter than what was asked for and
/// be taken for the last one.
pub async fn rows(
    app: &App,
    method: &str,
    params: &Value,
    size: usize,
) -> AppResult<Option<Vec<Value>>> {
    let mut rows: Vec<Value> = Vec::new();
    for _ in 0..MAX_PAGES {
        let mut params = params.clone();
        params["limit"] = json!(size);
        if !rows.is_empty() {
            params["cursor"] = json!(rows.len().to_string());
        }
        let Some(page) = page(app, method, params).await? else {
            return Ok(None);
        };
        let last = page.len() < size;
        rows.extend(page);
        if last {
            return Ok(Some(rows));
        }
    }
    Err(AppError::internal(format!(
        "{method} kept answering with full pages ({} rows read), so it was not read to the end",
        rows.len()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rows_are_found_in_either_shape_an_answer_comes_in() {
        assert_eq!(rows_of(json!([{"name": "Gi0/1"}])).len(), 1);
        assert_eq!(rows_of(json!({"rows": [{"name": "Gi0/1"}]})).len(), 1);
        assert!(rows_of(json!({"total": 3})).is_empty());
        assert!(rows_of(Value::Null).is_empty());
    }
}
