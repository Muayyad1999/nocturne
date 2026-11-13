//! Sweeping a range: the job, its events, and the record it leaves behind.
//!
//! A scan is a job, so it appears beside everything else a user has running,
//! reports progress, writes a log that says what it did and to how many
//! addresses — and can be stopped from the same button as everything else.
//! Cancelling is checked between addresses and inside every probe, so a
//! stopped scan stops sending immediately rather than draining its queue.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use nocturne_app::jobs::{self, JobCtx, JobSpec};
use nocturne_app::{App, AppError, AppResult, Router, util};
use nocturne_netprobe::tcp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::db;
use crate::identify::{self, Context, Known};
use crate::model::{Found, Scan, ScanRequest};
use crate::probe::Rate;
use crate::settings::DiscoverySettings;
use crate::targets;

/// How many `discovery.found` events one scan streams before it asks the UI
/// to read the results instead. A /16 of answering hosts must not push tens
/// of thousands of payloads through a bus every window shares.
const FOUND_EVENTS: u64 = 200;
/// How often a scan writes what it has found so far, and how often it
/// publishes progress once it has stopped streaming finds.
const FLUSH_EVERY: Duration = Duration::from_millis(750);
/// The port SNMP is asked on.
const SNMP_PORT: u16 = 161;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ScansParams {
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ScanGetParams {
    pub scan_id: String,
    /// Leave out finds that are already in the inventory.
    pub only_new: bool,
    /// Free text matched against address, names, vendor, profile and banners.
    pub query: Option<String>,
    /// Finds per page. A sweep of a /16 can answer tens of thousands of
    /// times, which is not a payload to hand a browser in one piece.
    pub limit: Option<u32>,
    /// Rows already received, as a decimal string. A page shorter than
    /// `limit` is the last one.
    pub cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ScanDeleteParams {
    pub scan_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ScanGet {
    pub scan: Scan,
    pub hosts: Vec<Found>,
    /// Finds matching the filters, before paging.
    pub total: usize,
}

/// Start a sweep. Everything that can be refused is refused here, before a
/// single packet is sent, so the user gets a reason and not a failed job.
pub async fn start(app: &App, request: ScanRequest) -> AppResult<Value> {
    let settings = DiscoverySettings::load(app);
    let limits = settings.limits(&request);

    if request.allow_public && !settings.allow_public {
        return Err(AppError::forbidden(
            "this installation does not allow sweeping public address space. Turn on `allow_public` under Settings → Discovery first",
        ));
    }
    let plan = targets::plan(
        &request.targets,
        limits.max_hosts,
        request.allow_public,
        &settings.never_scan,
    )
    .await?;

    let ports = parse_ports(&request, &settings)?;
    if !request.icmp && ports.is_empty() {
        return Err(AppError::invalid(
            "a scan with neither echo requests nor ports has no way to find anything",
        ));
    }
    // Resolving now means a locked vault or a credential without a community
    // is an error on the button, not a surprise half way through a sweep.
    let credentials = Arc::new(crate::snmp::resolve_all(app, &request.snmp_credential_ids)?);

    let scan = Scan {
        id: util::new_id(),
        kind: "sweep".into(),
        targets: request.targets.trim().to_string(),
        note: request.note.clone(),
        started_at: util::now_ms(),
        state: "running".into(),
        planned: plan.addresses.len() as u32,
        allow_public: plan.public,
        ..Scan::default()
    };

    let title = format!(
        "Scanning {} ({} addresses)",
        scan.targets,
        plan.addresses.len()
    );
    let context = Context {
        probes: db::state(app).probes.get(),
        rate: Arc::new(Rate::new(limits.rate)),
        timeout: Duration::from_millis(limits.timeout_ms),
        ports,
        icmp: request.icmp,
        identify_ssh: request.identify_ssh,
        resolve_names: request.resolve_names,
        snmp: credentials,
        snmp_port: match request.snmp_port {
            0 => SNMP_PORT,
            port => port,
        },
    };

    db::state(app).start_live(&scan.id);
    db::put_scan(app, &scan)?;
    let scan_id = scan.id.clone();
    let notes = plan.notes.clone();
    let addresses = plan.addresses.clone();
    let concurrency = limits.concurrency as usize;
    let rate = limits.rate;

    let job = jobs::start(
        app,
        JobSpec::new("discovery.scan", title).total(addresses.len() as u64),
        move |ctx| async move {
            let scan_id = scan.id.clone();
            let outcome = run(
                ctx.clone(),
                scan,
                context,
                addresses,
                notes,
                concurrency,
                rate,
            )
            .await;
            // A scan whose job failed must not stay `running` for ever: the
            // record carries the reason, and the retention sweep can reach it.
            if let Err(error) = &outcome
                && error.code != "cancelled"
            {
                crate::db::fail_scan(ctx.app(), &scan_id, &error.message);
            }
            outcome
        },
    );

    let mut scan = db::get_scan(app, &scan_id)?;
    scan.job_id = job.id.clone();
    db::put_scan(app, &scan)?;
    app.audit(
        "discovery.scan",
        None,
        format!("Started a discovery scan of `{}`", scan.targets),
        json!({"scan_id": scan_id, "addresses": scan.planned, "allow_public": scan.allow_public}),
    );
    Ok(json!({"job_id": job.id, "scan_id": scan_id}))
}

/// The sweep itself.
async fn run(
    ctx: JobCtx,
    mut scan: Scan,
    context: Context,
    addresses: Vec<IpAddr>,
    notes: Vec<String>,
    concurrency: usize,
    rate: u32,
) -> AppResult<Value> {
    let app = ctx.app().clone();
    for note in &notes {
        ctx.info(note);
    }
    ctx.info(format!(
        "{} addresses, at most {rate} probes per second, {concurrency} at a time",
        addresses.len()
    ));
    if scan.allow_public {
        ctx.warn("this scan covers public address space, as `allow_public` asked");
    }
    if !context.snmp.is_empty() {
        ctx.info(format!(
            "SNMP will be tried with {} credential(s): {}",
            context.snmp.len(),
            context
                .snmp
                .iter()
                .map(|credential| credential.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let state = db::state(&app);
    let known = Known::load(&app)?;
    let ignored = db::ignored_set(&app)?;
    let context = Arc::new(context);
    let streamed = Arc::new(AtomicU64::new(0));
    let probed = Arc::new(AtomicU64::new(0));

    // A ticker writes what has been found so far and keeps the UI's counters
    // moving; the finds themselves stream as events until the cap.
    let flusher = {
        let (app, scan_id, probed, streamed) = (
            app.clone(),
            scan.id.clone(),
            probed.clone(),
            streamed.clone(),
        );
        let token = ctx.token().clone();
        tokio::spawn(async move {
            let state = db::state(&app);
            loop {
                tokio::select! {
                    _ = token.cancelled() => return,
                    _ = tokio::time::sleep(FLUSH_EVERY) => {}
                }
                let Some(hosts) = state.live(&scan_id) else {
                    return;
                };
                let _ = db::put_hosts(&app, &scan_id, &hosts);
                if streamed.load(Ordering::Relaxed) >= FOUND_EVENTS {
                    app.publish(
                        "discovery.progress",
                        &json!({
                            "scan_id": scan_id,
                            "probed": probed.load(Ordering::Relaxed),
                            "found": hosts.len(),
                            "streaming": false,
                        }),
                    );
                }
            }
        })
    };

    let results = ctx
        .for_each(addresses.clone(), concurrency.max(1), |ctx, address| {
            let (app, context, state, known, ignored, streamed, probed, scan_id) = (
                app.clone(),
                context.clone(),
                state.clone(),
                &known,
                &ignored,
                streamed.clone(),
                probed.clone(),
                scan.id.clone(),
            );
            async move {
                ctx.check()?;
                let found = identify::probe_address(&context, address).await;
                probed.fetch_add(1, Ordering::Relaxed);
                let Some(mut found) = found else {
                    return Ok(false);
                };
                finish_find(&app, &mut found, known, ignored, "", "");
                state.push_live(&scan_id, found.clone());
                let already = streamed.fetch_add(1, Ordering::Relaxed);
                if already < FOUND_EVENTS {
                    app.publish(
                        "discovery.found",
                        &json!({"scan_id": scan_id, "host": found}),
                    );
                } else if already == FOUND_EVENTS {
                    ctx.info(format!(
                        "more than {FOUND_EVENTS} hosts have answered; the rest are in the scan's results rather than in events"
                    ));
                }
                ctx.status(format!("{} answered", found.address));
                Ok(true)
            }
        })
        .await;

    let cancelled = results
        .iter()
        .any(|result| matches!(result, Err(error) if error.code == "cancelled"));
    for error in results.iter().filter_map(|result| result.as_ref().err()) {
        if error.code != "cancelled" {
            ctx.warn(error.message.clone());
        }
    }

    // The neighbour table knows the hardware address of every host on a
    // segment this machine is attached to — including hosts that ignored
    // every probe but could not ignore an ARP request.
    if !cancelled {
        ctx.status("reading this machine's neighbour table");
        let neighbours = neighbour_map(&context).await;
        let swept: std::collections::HashSet<String> =
            addresses.iter().map(ToString::to_string).collect();
        let mut hosts = state.end_live(&scan.id);
        let answered: std::collections::HashSet<String> =
            hosts.iter().map(|host| host.address.clone()).collect();
        for host in &mut hosts {
            identify::attach_mac(host, &neighbours);
        }
        for (address, neighbour) in &neighbours {
            if answered.contains(address) || !swept.contains(address) {
                continue;
            }
            let Ok(parsed) = address.parse::<IpAddr>() else {
                continue;
            };
            let mut found = Found {
                address: address.clone(),
                alive: true,
                via: "arp".into(),
                found_at: util::now_ms(),
                ..Found::default()
            };
            found.say(
                "alive",
                "arp",
                format!(
                    "answered neighbour discovery on {} although it ignored every probe",
                    neighbour.interface
                ),
            );
            identify::attach_mac(&mut found, &neighbours);
            identify::identify(&context, parsed, &mut found).await;
            finish_find(&app, &mut found, &known, &ignored, "", "");
            app.publish(
                "discovery.found",
                &json!({"scan_id": scan.id, "host": found}),
            );
            hosts.push(found);
        }
        hosts.sort_by_key(|host| host.address.parse::<IpAddr>().ok());
        db::put_hosts(&app, &scan.id, &hosts)?;
        summarise(&mut scan, &hosts);
    } else {
        let hosts = state.end_live(&scan.id);
        db::put_hosts(&app, &scan.id, &hosts)?;
        summarise(&mut scan, &hosts);
    }
    flusher.abort();

    scan.probed = probed.load(Ordering::Relaxed) as u32;
    scan.probes_sent = context.rate.sent();
    scan.finished_at = Some(util::now_ms());
    scan.state = if cancelled { "cancelled" } else { "finished" }.into();
    // The record is updated in place rather than overwritten, because the
    // job id was written to it after this task was handed its copy — and
    // because a scan deleted while it ran must stay deleted.
    let summarised = &scan;
    let stored = db::update_scan(&app, &scan.id, |record| {
        record.probed = summarised.probed;
        record.probes_sent = summarised.probes_sent;
        record.finished_at = summarised.finished_at;
        record.state = summarised.state.clone();
        record.found = summarised.found;
        record.identified = summarised.identified;
        record.new = summarised.new;
    })?;
    if stored.is_none() {
        ctx.info("this scan was deleted while it ran; its results were dropped");
        return if cancelled {
            Err(jobs::cancelled())
        } else {
            Ok(Value::Null)
        };
    }
    db::prune(&app, DiscoverySettings::load(&app).keep_scans as usize)?;

    let summary = json!({
        "scan_id": scan.id,
        "state": scan.state,
        "probed": scan.probed,
        "planned": scan.planned,
        "found": scan.found,
        "identified": scan.identified,
        "new": scan.new,
        "probes_sent": scan.probes_sent,
    });
    app.publish(
        "discovery.finished",
        &json!({"scan_id": scan.id, "summary": summary}),
    );
    ctx.info(format!(
        "{} of {} addresses answered; {} could be identified, {} are not in the inventory",
        scan.found, scan.probed, scan.identified, scan.new
    ));
    if cancelled {
        return Err(jobs::cancelled());
    }
    Ok(summary)
}

/// The last steps every find goes through, whether it came from a sweep, the
/// neighbour table or a crawl.
pub fn finish_find(
    app: &App,
    found: &mut Found,
    known: &Known,
    ignored: &std::collections::HashSet<String>,
    site: &str,
    role: &str,
) {
    identify::match_profile(app, found);
    known.mark(found);
    found.ignored = ignored.contains(&found.address);
    identify::suggest(found, site, role);
}

async fn neighbour_map(context: &Context) -> HashMap<String, nocturne_netprobe::local::Neighbour> {
    context
        .probes
        .neighbours()
        .await
        .into_iter()
        .map(|neighbour| (neighbour.address.clone(), neighbour))
        .collect()
}

fn summarise(scan: &mut Scan, hosts: &[Found]) {
    scan.found = hosts.len() as u32;
    scan.identified = hosts.iter().filter(|host| host.identified()).count() as u32;
    scan.new = hosts
        .iter()
        .filter(|host| host.device_id.is_none() && !host.ignored)
        .count() as u32;
}

fn parse_ports(request: &ScanRequest, settings: &DiscoverySettings) -> AppResult<Vec<u16>> {
    // No `ports` at all means "the usual ones", from the settings; an empty
    // string means the user asked for no port probing.
    let spec = match request.ports.as_deref() {
        None => settings.ports.trim().to_string(),
        Some(given) => given.trim().to_string(),
    };
    if spec.is_empty() {
        return Ok(Vec::new());
    }
    tcp::parse_ports(&spec).map_err(|error| AppError::invalid(error.to_string()))
}

/// Re-read what may have changed since the scan ran: what is in the inventory
/// now, and what the user has since chosen to ignore. A find is a record of
/// what answered, not of what Nocturne managed at the time.
pub fn refresh(app: &App, hosts: &mut [Found]) -> AppResult<()> {
    let known = Known::load(app)?;
    let ignored = db::ignored_set(app)?;
    for host in hosts {
        host.device_id = None;
        host.device_name = None;
        host.evidence
            .retain(|evidence| evidence.field != "device_id");
        known.mark(host);
        host.ignored = ignored.contains(&host.address);
    }
    Ok(())
}

pub fn get(app: &App, params: &ScanGetParams) -> AppResult<ScanGet> {
    let scan = db::get_scan(app, &params.scan_id)?;
    let mut hosts = db::hosts(app, &params.scan_id)?;
    refresh(app, &mut hosts)?;
    if params.only_new {
        hosts.retain(|host| host.device_id.is_none() && !host.ignored);
    }
    if let Some(query) = params
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
    {
        hosts.retain(|host| util::matches_query(&haystack(host), query));
    }
    let total = hosts.len();
    let offset = match params.cursor.as_deref() {
        None | Some("") => 0,
        Some(cursor) => cursor
            .parse::<usize>()
            .map_err(|_| AppError::invalid("the cursor is the number of finds already received"))?,
    };
    let limit = params.limit.unwrap_or(1_000).clamp(1, 5_000) as usize;
    let hosts = hosts.into_iter().skip(offset).take(limit).collect();
    Ok(ScanGet { scan, hosts, total })
}

fn haystack(host: &Found) -> String {
    let ports: Vec<String> = host
        .open_ports
        .iter()
        .map(|port| {
            format!(
                "{} {} {}",
                port.port,
                port.service,
                port.banner.clone().unwrap_or_default()
            )
        })
        .collect();
    format!(
        "{} {} {} {} {} {} {} {} {}",
        host.address,
        host.hostname.clone().unwrap_or_default(),
        host.mac.clone().unwrap_or_default(),
        host.vendor.clone().unwrap_or_default(),
        host.ssh_banner.clone().unwrap_or_default(),
        host.snmp
            .as_ref()
            .map(|snmp| format!(
                "{} {}",
                snmp.sys_name.clone().unwrap_or_default(),
                snmp.sys_descr.clone().unwrap_or_default()
            ))
            .unwrap_or_default(),
        host.profile
            .as_ref()
            .map(|profile| format!("{} {}", profile.id, profile.name))
            .unwrap_or_default(),
        host.suggested.name,
        ports.join(" "),
    )
}

pub fn register(router: &mut Router) {
    router.add(
        "discovery.scan",
        |app: App, request: ScanRequest| async move { start(&app, request).await },
    );
    router.add(
        "discovery.scans",
        |app: App, params: ScansParams| async move {
            db::list_scans(&app, params.limit.unwrap_or(50).clamp(1, 500))
        },
    );
    router.add(
        "discovery.scan_get",
        |app: App, params: ScanGetParams| async move { get(&app, &params) },
    );
    router.add(
        "discovery.scan_delete",
        |app: App, params: ScanDeleteParams| async move {
            let mut deleted = 0;
            for scan_id in &params.scan_ids {
                // A scan that is still running is stopped first: deleting its
                // record while its job writes to it would resurrect it.
                if let Ok(scan) = db::get_scan(&app, scan_id)
                    && scan.state == "running"
                    && !scan.job_id.is_empty()
                {
                    let _ = jobs::cancel(&app, &scan.job_id);
                }
                if db::delete_scan(&app, scan_id)? {
                    deleted += 1;
                }
            }
            app.publish("discovery.scans_changed", &json!({"deleted": deleted}));
            Ok(json!({"deleted": deleted}))
        },
    );
}
