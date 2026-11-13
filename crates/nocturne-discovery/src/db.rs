//! Where scans live.
//!
//! Scans are documents, as the specification asks: one record per scan and
//! one document holding its finds. A running scan keeps its finds in memory
//! and flushes them in batches, so that watching a /22 come in does not
//! rewrite the same document once per answering host; `hosts` serves the
//! live copy while the scan runs and the stored one afterwards, so a reader
//! cannot tell the difference.

use std::collections::HashMap;

use nocturne_app::{App, AppError, AppResult, util};
use parking_lot::Mutex;

use crate::model::{Found, Ignored, Scan};
use crate::probe::ProbeLayer;

pub const SCANS: &str = "discovery_scans";
pub const HOSTS: &str = "discovery_hosts";
pub const IGNORED: &str = "discovery_ignored";

/// A scan's finds as one document.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct HostDoc {
    pub scan_id: String,
    pub hosts: Vec<Found>,
}

/// This feature's runtime state.
#[derive(Default)]
pub struct DiscoveryState {
    pub probes: ProbeLayer,
    /// Finds of the scans that are running, newest write wins.
    live: Mutex<HashMap<String, Vec<Found>>>,
}

impl DiscoveryState {
    pub fn start_live(&self, scan_id: &str) {
        self.live.lock().insert(scan_id.to_string(), Vec::new());
    }

    pub fn push_live(&self, scan_id: &str, found: Found) {
        if let Some(hosts) = self.live.lock().get_mut(scan_id) {
            hosts.push(found);
        }
    }

    pub fn live(&self, scan_id: &str) -> Option<Vec<Found>> {
        self.live.lock().get(scan_id).cloned()
    }

    pub fn end_live(&self, scan_id: &str) -> Vec<Found> {
        self.live.lock().remove(scan_id).unwrap_or_default()
    }
}

pub fn state(app: &App) -> std::sync::Arc<DiscoveryState> {
    app.state::<DiscoveryState>()
}

pub fn put_scan(app: &App, scan: &Scan) -> AppResult<()> {
    app.store().put(SCANS, &scan.id, scan)
}

pub fn get_scan(app: &App, scan_id: &str) -> AppResult<Scan> {
    app.store().require(SCANS, scan_id, "scan")
}

/// Newest first.
pub fn list_scans(app: &App, limit: usize) -> AppResult<Vec<Scan>> {
    let mut scans: Vec<Scan> = app.store().list(SCANS)?;
    scans.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.id.cmp(&a.id)));
    scans.truncate(limit);
    Ok(scans)
}

/// Change a scan's record, if it is still there.
///
/// A scan can be deleted while its job runs, and the job must not write the
/// record back afterwards: `None` means "it was deleted, let it go".
pub fn update_scan(
    app: &App,
    scan_id: &str,
    change: impl FnOnce(&mut Scan),
) -> AppResult<Option<Scan>> {
    let Some(mut scan) = app.store().get::<Scan>(SCANS, scan_id)? else {
        return Ok(None);
    };
    change(&mut scan);
    put_scan(app, &scan)?;
    Ok(Some(scan))
}

/// Write a running scan's finds so far, or its final set. Nothing is written
/// for a scan that has been deleted.
pub fn put_hosts(app: &App, scan_id: &str, hosts: &[Found]) -> AppResult<()> {
    if app.store().get::<Scan>(SCANS, scan_id)?.is_none() {
        return Ok(());
    }
    app.store().put(
        HOSTS,
        scan_id,
        &HostDoc {
            scan_id: scan_id.to_string(),
            hosts: hosts.to_vec(),
        },
    )
}

/// The finds of a scan: the live set while it runs, the stored one after.
pub fn hosts(app: &App, scan_id: &str) -> AppResult<Vec<Found>> {
    if let Some(live) = state(app).live(scan_id) {
        return Ok(live);
    }
    Ok(app
        .store()
        .get::<HostDoc>(HOSTS, scan_id)?
        .map(|doc| doc.hosts)
        .unwrap_or_default())
}

/// Mark a scan whose job failed, with the reason.
pub fn fail_scan(app: &App, scan_id: &str, reason: &str) {
    let Ok(mut scan) = get_scan(app, scan_id) else {
        return;
    };
    let hosts = state(app).end_live(scan_id);
    let _ = put_hosts(app, scan_id, &hosts);
    scan.found = hosts.len() as u32;
    scan.state = "failed".into();
    scan.error = Some(reason.to_string());
    scan.finished_at = Some(util::now_ms());
    if let Err(error) = put_scan(app, &scan) {
        tracing::error!(%error, scan = scan_id, "a failed scan could not be recorded");
    }
    app.publish(
        "discovery.finished",
        &serde_json::json!({
            "scan_id": scan_id,
            "summary": {"scan_id": scan_id, "state": "failed", "error": reason,
                        "probed": scan.probed, "found": scan.found,
                        "identified": scan.identified, "new": scan.new,
                        "probes_sent": scan.probes_sent},
        }),
    );
}

/// Scans that were running when the previous process ended did not finish.
pub fn close_interrupted(store: &nocturne_app::store::Store) {
    let Ok(scans) = store.list::<Scan>(SCANS) else {
        return;
    };
    for mut scan in scans.into_iter().filter(|scan| scan.state == "running") {
        scan.state = "failed".into();
        scan.error = Some("Nocturne was closed while this scan was running".into());
        scan.finished_at = Some(util::now_ms());
        let _ = store.put(SCANS, &scan.id, &scan);
    }
}

pub fn delete_scan(app: &App, scan_id: &str) -> AppResult<bool> {
    let existed = app.store().delete(SCANS, scan_id)?;
    app.store().delete(HOSTS, scan_id)?;
    state(app).end_live(scan_id);
    Ok(existed)
}

/// Keep the newest `keep` finished scans; a running one is never pruned.
pub fn prune(app: &App, keep: usize) -> AppResult<usize> {
    let mut finished: Vec<Scan> = app
        .store()
        .list::<Scan>(SCANS)?
        .into_iter()
        .filter(|scan| scan.state != "running")
        .collect();
    if finished.len() <= keep {
        return Ok(0);
    }
    finished.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.id.cmp(&a.id)));
    let mut removed = 0;
    for scan in &finished[keep..] {
        delete_scan(app, &scan.id)?;
        removed += 1;
    }
    Ok(removed)
}

// ------------------------------------------------------------------ ignore

fn ignore_key(address: &str) -> AppResult<String> {
    address
        .trim()
        .parse::<std::net::IpAddr>()
        .map(|address| address.to_string())
        .map_err(|_| {
            AppError::invalid(format!(
                "`{address}` is not an IP address; discovery ignores addresses, not names"
            ))
        })
}

pub fn ignore(app: &App, addresses: &[String], note: &str) -> AppResult<Vec<String>> {
    let mut stored = Vec::new();
    for address in addresses {
        let key = ignore_key(address)?;
        app.store().put(
            IGNORED,
            &key,
            &Ignored {
                address: key.clone(),
                note: note.to_string(),
                at: util::now_ms(),
            },
        )?;
        stored.push(key);
    }
    Ok(stored)
}

pub fn unignore(app: &App, addresses: &[String]) -> AppResult<Vec<String>> {
    let mut removed = Vec::new();
    for address in addresses {
        let key = ignore_key(address)?;
        if app.store().delete(IGNORED, &key)? {
            removed.push(key);
        }
    }
    Ok(removed)
}

pub fn ignored(app: &App) -> AppResult<Vec<Ignored>> {
    let mut all: Vec<Ignored> = app.store().list(IGNORED)?;
    all.sort_by(|a, b| a.address.cmp(&b.address));
    Ok(all)
}

pub fn ignored_set(app: &App) -> AppResult<std::collections::HashSet<String>> {
    Ok(ignored(app)?
        .into_iter()
        .map(|entry| entry.address)
        .collect())
}
