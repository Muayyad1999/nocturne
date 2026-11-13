//! The live tail.
//!
//! The event bus is one broadcast channel shared by every feature and every
//! connected UI, so a tail that forwarded each line as it arrived would, at
//! twenty thousand lines a minute, push everything else off the bus and melt
//! whatever is drawing it. This one batches: at most four events a second,
//! at most [`TAIL_MAX_ENTRIES`] lines in each.
//!
//! When more arrive than that, the tail **samples** rather than truncates —
//! evenly spaced lines plus the newest — and says how many it left out. A
//! tail that silently showed the first two hundred of twenty thousand would
//! be a lie about what the network is doing; one that says "200 of 18,431"
//! is a reading.

use nocturne_app::store::LogEntry;
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppError, AppResult};
use serde_json::json;

use crate::db::Prepared;
use crate::query::Filter;
use crate::state::{Follow, LogsState, TAIL_INTERVAL_MS, TAIL_MAX_ENTRIES};

/// How many entries may wait for the next event before the oldest waiting
/// ones are counted rather than kept. Four events' worth: enough to sample
/// from, bounded so a storm cannot grow this buffer.
const PENDING_LIMIT: usize = TAIL_MAX_ENTRIES * 20;
/// A follow that nobody renews stops costing anything after this.
const FOLLOW_TTL_MS: i64 = 120_000;
/// How many bytes of log text one event may carry.
///
/// Two hundred entries is the contract, but an entry is up to 64 KiB — the
/// event bus keeps its whole ring resident, so two hundred of those would be
/// megabytes in one event and gigabytes in the ring. The newest lines that
/// fit are sent and the rest are counted; the full lines are in the store,
/// which is where a screen reads them from anyway.
const TAIL_MAX_BYTES: usize = 64 * 1024;

/// Hand a written batch to the tail and to every open follow.
pub fn offer(app: &App, batch: &[Prepared]) {
    let state = app.state::<LogsState>();
    {
        let (pending, dropped) = &mut *state.tail.lock();
        for item in batch {
            if pending.len() >= PENDING_LIMIT {
                *dropped += 1;
            } else {
                pending.push(item.entry.clone());
            }
        }
    }
    let mut follows = state.follows.lock();
    if follows.is_empty() {
        return;
    }
    for follow in follows.values_mut() {
        for item in batch {
            if !follow.filter.matches(item) {
                continue;
            }
            if follow.pending.len() >= PENDING_LIMIT {
                follow.dropped += 1;
            } else {
                follow.pending.push(item.entry.clone());
            }
        }
    }
}

/// Roughly what one entry costs in an event.
fn weight(entry: &LogEntry) -> usize {
    let raw = entry
        .body
        .as_ref()
        .and_then(|body| body.get("raw"))
        .and_then(serde_json::Value::as_str)
        .map(str::len)
        .unwrap_or_default();
    entry.message.len() + raw + entry.source.len() + 128
}

/// The newest entries that fit in `budget`, in order, and how many were left
/// out for want of room.
fn fit(mut entries: Vec<LogEntry>, budget: usize) -> (Vec<LogEntry>, u64) {
    let mut spent = 0;
    let mut keep = 0;
    for entry in entries.iter().rev() {
        let cost = weight(entry);
        if keep > 0 && spent + cost > budget {
            break;
        }
        spent += cost;
        keep += 1;
    }
    let dropped = (entries.len() - keep) as u64;
    entries.drain(..entries.len() - keep);
    (entries, dropped)
}

/// Evenly spaced entries, the newest always included.
fn sample(entries: Vec<LogEntry>, max: usize) -> (Vec<LogEntry>, bool) {
    if entries.len() <= max {
        return (entries, false);
    }
    let step = entries.len() as f64 / max as f64;
    let mut kept: Vec<LogEntry> = (0..max)
        .map(|index| entries[((index as f64) * step) as usize].clone())
        .collect();
    if let (Some(last), Some(newest)) = (kept.last_mut(), entries.last()) {
        *last = newest.clone();
    }
    (kept, true)
}

/// Publish whatever has piled up. Called by the tail task every quarter of a
/// second; returns the number of events published (for tests).
pub fn flush(app: &App, now: i64) -> usize {
    let state = app.state::<LogsState>();
    let mut published = 0;
    let (entries, dropped) = {
        let (pending, dropped) = &mut *state.tail.lock();
        (std::mem::take(pending), std::mem::replace(dropped, 0))
    };
    if !entries.is_empty() {
        let total = entries.len() as u64 + dropped;
        let (entries, sampled) = sample(entries, TAIL_MAX_ENTRIES);
        let (entries, _) = fit(entries, TAIL_MAX_BYTES);
        let left_out = total - entries.len() as u64;
        app.publish(
            "logs.appended",
            &json!({"entries": entries, "dropped": left_out, "sampled": sampled, "received": total}),
        );
        published += 1;
    }
    let ready: Vec<(String, Vec<LogEntry>, u64)> = {
        let mut follows = state.follows.lock();
        follows.retain(|_, follow| follow.expires_at > now);
        follows
            .iter_mut()
            .filter(|(_, follow)| !follow.pending.is_empty())
            .map(|(id, follow)| {
                (
                    id.clone(),
                    std::mem::take(&mut follow.pending),
                    std::mem::replace(&mut follow.dropped, 0),
                )
            })
            .collect()
    };
    for (id, entries, dropped) in ready {
        let total = entries.len() as u64 + dropped;
        let (entries, sampled) = sample(entries, TAIL_MAX_ENTRIES);
        let (entries, _) = fit(entries, TAIL_MAX_BYTES);
        let left_out = total - entries.len() as u64;
        app.publish(
            "logs.tail",
            &json!({"follow_id": id, "entries": entries, "dropped": left_out,
                    "sampled": sampled, "received": total}),
        );
        published += 1;
    }
    published
}

/// The tail task.
pub async fn publisher(app: App) {
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(TAIL_INTERVAL_MS)).await;
        flush(&app, now_ms());
    }
}

/// Open or renew a follow. The same filter the search uses, so that
/// "follow what I am looking at" needs no second filter language.
pub fn follow(app: &App, filter: Filter, existing: Option<String>) -> AppResult<serde_json::Value> {
    let state = app.state::<LogsState>();
    let now = now_ms();
    let mut follows = state.follows.lock();
    let expires_at = now + FOLLOW_TTL_MS;
    let id = match existing.filter(|id| follows.contains_key(id)) {
        Some(id) => {
            if let Some(follow) = follows.get_mut(&id) {
                follow.filter = filter;
                follow.expires_at = expires_at;
            }
            id
        }
        None => {
            // A hard ceiling: each follow costs a filter pass over every line.
            if follows.len() >= 32 {
                follows.retain(|_, follow| follow.expires_at > now);
            }
            if follows.len() >= 32 {
                return Err(AppError::conflict(
                    "too many live tails are open; close one before opening another",
                ));
            }
            let id = new_id();
            follows.insert(
                id.clone(),
                Follow {
                    filter,
                    pending: Vec::new(),
                    dropped: 0,
                    expires_at,
                },
            );
            id
        }
    };
    Ok(json!({"follow_id": id, "expires_at": expires_at, "event": "logs.tail"}))
}

pub fn unfollow(app: &App, id: &str) -> bool {
    app.state::<LogsState>().follows.lock().remove(id).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: i64) -> LogEntry {
        LogEntry {
            id,
            ts: id,
            stream: "syslog".into(),
            device_id: None,
            severity: Some(5),
            source: "10.0.0.1".into(),
            message: format!("line {id}"),
            body: None,
        }
    }

    #[test]
    fn a_tail_that_cannot_keep_up_samples_and_keeps_the_newest() {
        let entries: Vec<LogEntry> = (1..=1_000).map(entry).collect();
        let (kept, sampled) = sample(entries, 200);
        assert!(sampled);
        assert_eq!(kept.len(), 200);
        assert_eq!(kept[0].id, 1, "the window starts where it started");
        assert_eq!(kept[199].id, 1_000, "and ends at the newest line");
        let spread: Vec<i64> = kept
            .windows(2)
            .map(|pair| pair[1].id - pair[0].id)
            .collect();
        assert!(
            spread.iter().take(190).all(|gap| *gap == 5),
            "evenly spaced, not the first two hundred: {spread:?}"
        );
    }

    #[test]
    fn one_event_cannot_carry_megabytes_of_log_text() {
        let huge: Vec<LogEntry> = (1..=50)
            .map(|id| LogEntry {
                message: "x".repeat(20_000),
                ..entry(id)
            })
            .collect();
        let (kept, dropped) = fit(huge, TAIL_MAX_BYTES);
        assert_eq!((kept.len(), dropped), (3, 47));
        assert_eq!(
            kept.last().unwrap().id,
            50,
            "the newest lines are the ones that fit"
        );

        // A single line longer than the whole budget is still delivered: one
        // entry that cannot be shown is worse than a tail that stops.
        let (kept, dropped) = fit(
            vec![LogEntry {
                message: "x".repeat(200_000),
                ..entry(1)
            }],
            TAIL_MAX_BYTES,
        );
        assert_eq!((kept.len(), dropped), (1, 0));
    }

    #[test]
    fn a_tail_that_keeps_up_passes_everything_through_untouched() {
        let entries: Vec<LogEntry> = (1..=7).map(entry).collect();
        let (kept, sampled) = sample(entries.clone(), 200);
        assert!(!sampled);
        assert_eq!(kept, entries);
    }
}
