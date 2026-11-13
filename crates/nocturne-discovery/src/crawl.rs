//! Discovery from what is already known.
//!
//! The politest way to map a network is to ask it. Every managed device
//! already holds a list of its neighbours (CDP/LLDP), the hosts it has
//! spoken to (ARP) and the next hops it routes through; walking those tables
//! finds the network the way its own control plane sees it, without sending
//! a single packet to a stranger.
//!
//! Rules this follows, and the reasons:
//!
//! * **Every find says who told us.** `told_by` names the device, the table
//!   and the interface, and `path` is the chain of devices from the seed, so
//!   a wrong find can be traced to the table it came out of.
//! * **It only reads from devices the user already manages.** Reading a
//!   neighbour table is `collect::run`, which goes through the core's
//!   read-only command gate with the device's own credential. A candidate
//!   that is not in the inventory is probed exactly like a scan host —
//!   never logged into.
//! * **It never crawls into something it could not identify.** A candidate
//!   becomes a seed for the next level only when it is in the inventory, or
//!   when `add` was asked for *and* the profile registry recognised it.
//! * **The SNMP credentials are the ones the request named**, and nothing
//!   else is ever tried.

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use nocturne_app::inventory::{self, DeviceInput};
use nocturne_app::jobs::{self, JobCtx, JobSpec};
use nocturne_app::model::{Device, MonitorSpec, Transport, collections::DEVICES};
use nocturne_app::{App, AppError, AppResult, Router, collect, util};
use nocturne_drivers::model::Kind;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::db;
use crate::identify::{self, Context, Known};
use crate::model::{Found, Scan, ToldBy};
use crate::probe::Rate;
use crate::scan::finish_find;
use crate::settings::DiscoverySettings;

/// ARP entries taken from one device. A distribution switch can know ten
/// thousand hosts; a crawl is about finding the network, not inventorying
/// every workstation on it.
const ARP_PER_DEVICE: usize = 256;
/// Next hops taken from one device's routing table, for the same reason.
const ROUTES_PER_DEVICE: usize = 128;
/// Candidates one crawl will probe, whatever its seeds hold.
const MAX_CANDIDATES: usize = 4_096;
const SNMP_PORT: u16 = 161;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct CrawlRequest {
    /// Devices to start from. Empty = every device in the inventory.
    pub seed_device_ids: Vec<String>,
    /// How many levels out from the seeds.
    pub depth: u32,
    /// SNMP credentials to try on candidates, in order. Nothing else is tried.
    pub credential_ids: Vec<String>,
    /// Add what is positively identified, with the seed's credential and site.
    pub add: bool,
    /// Also walk ARP caches (hosts the seed has spoken to).
    pub use_arp: bool,
    /// Also walk routing tables (next hops).
    pub use_routes: bool,
    /// TCP ports to try on candidates; empty = the setting's ports.
    pub ports: String,
    pub note: String,
}

impl Default for CrawlRequest {
    fn default() -> Self {
        CrawlRequest {
            seed_device_ids: Vec::new(),
            depth: 2,
            credential_ids: Vec::new(),
            add: false,
            use_arp: true,
            use_routes: false,
            ports: String::new(),
            note: String::new(),
        }
    }
}

/// One address a seed told us about.
#[derive(Clone, Debug)]
struct Candidate {
    address: IpAddr,
    told_by: ToldBy,
    path: Vec<String>,
    /// From the neighbour's reported capabilities, where there were any.
    role: String,
    /// A MAC the seed's own table knew.
    mac: Option<String>,
}

pub async fn start(app: &App, request: CrawlRequest) -> AppResult<Value> {
    let settings = DiscoverySettings::load(app);
    let depth = request.depth.clamp(1, settings.max_crawl_depth);
    let seeds = seed_devices(app, &request.seed_device_ids)?;
    if seeds.is_empty() {
        return Err(AppError::invalid(
            "there is nothing to crawl from: add a device to the inventory first, or name the seeds",
        ));
    }
    let credentials = Arc::new(crate::snmp::resolve_all(app, &request.credential_ids)?);
    let ports = match request.ports.trim() {
        "" => settings.ports.clone(),
        given => given.to_string(),
    };
    let ports = nocturne_netprobe::tcp::parse_ports(&ports)
        .map_err(|error| AppError::invalid(error.to_string()))?;

    let targets = seeds
        .iter()
        .map(|device| device.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let scan = Scan {
        id: util::new_id(),
        kind: "crawl".into(),
        targets: targets.clone(),
        note: request.note.clone(),
        started_at: util::now_ms(),
        state: "running".into(),
        ..Scan::default()
    };
    db::state(app).start_live(&scan.id);
    db::put_scan(app, &scan)?;

    let context = Context {
        probes: db::state(app).probes.get(),
        rate: Arc::new(Rate::new(settings.rate)),
        timeout: Duration::from_millis(settings.timeout_ms),
        ports,
        icmp: true,
        identify_ssh: true,
        resolve_names: true,
        snmp: credentials,
        snmp_port: SNMP_PORT,
    };

    let scan_id = scan.id.clone();
    let seed_ids: Vec<String> = seeds.iter().map(|device| device.id.clone()).collect();
    let add = request.add;
    let use_arp = request.use_arp;
    let use_routes = request.use_routes;
    let job = jobs::start(
        app,
        JobSpec::new(
            "discovery.crawl",
            format!("Crawling from {} device(s)", seeds.len()),
        )
        .devices(&seed_ids),
        move |ctx| async move {
            let scan_id = scan.id.clone();
            let outcome = run(
                ctx.clone(),
                scan,
                context,
                seed_ids,
                depth,
                add,
                use_arp,
                use_routes,
            )
            .await;
            if let Err(error) = &outcome
                && error.code != "cancelled"
            {
                db::fail_scan(ctx.app(), &scan_id, &error.message);
            }
            outcome
        },
    );

    let mut scan = db::get_scan(app, &scan_id)?;
    scan.job_id = job.id.clone();
    db::put_scan(app, &scan)?;
    app.audit(
        "discovery.crawl",
        None,
        format!("Started a neighbour crawl from `{targets}`"),
        json!({"scan_id": scan_id, "depth": depth, "add": add}),
    );
    Ok(json!({"job_id": job.id, "scan_id": scan_id}))
}

fn seed_devices(app: &App, ids: &[String]) -> AppResult<Vec<Device>> {
    let all: Vec<Device> = app.store().list(DEVICES)?;
    if ids.is_empty() {
        return Ok(all);
    }
    ids.iter()
        .map(|id| {
            all.iter()
                .find(|device| &device.id == id)
                .cloned()
                .ok_or_else(|| AppError::not_found(format!("device `{id}`")))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn run(
    ctx: JobCtx,
    mut scan: Scan,
    context: Context,
    seeds: Vec<String>,
    depth: u32,
    add: bool,
    use_arp: bool,
    use_routes: bool,
) -> AppResult<Value> {
    let app = ctx.app().clone();
    let state = db::state(&app);
    let ignored = db::ignored_set(&app)?;
    let mut visited: HashSet<String> = HashSet::new();
    let mut probed: HashSet<String> = HashSet::new();
    // Each level carries, with every device to read, the chain of devices
    // that led to it — which is what `path` on a find reports.
    let mut frontier: Vec<(String, Vec<String>)> =
        seeds.into_iter().map(|id| (id, Vec::new())).collect();
    let mut hosts: Vec<Found> = Vec::new();

    ctx.info(format!(
        "crawling {} level(s) from {} seed(s){}",
        depth,
        frontier.len(),
        if add {
            ", adding what is identified"
        } else {
            ", proposing only"
        }
    ));

    for level in 1..=depth {
        if frontier.is_empty() {
            break;
        }
        ctx.check()?;
        ctx.status(format!(
            "level {level}: reading {} device(s)",
            frontier.len()
        ));
        ctx.set_total((visited.len() + frontier.len()) as u64);

        let mut candidates: Vec<Candidate> = Vec::new();
        let mut next: Vec<(String, Vec<String>)> = Vec::new();
        for (device_id, path) in std::mem::take(&mut frontier) {
            ctx.check()?;
            if !visited.insert(device_id.clone()) {
                continue;
            }
            let device = match inventory::get(&app, &device_id) {
                Ok(device) => device,
                Err(error) => {
                    ctx.warn(error.message);
                    continue;
                }
            };
            let mut chain = path.clone();
            chain.push(device.name.clone());
            match read_tables(&ctx, &device, use_arp, use_routes).await {
                Ok(found) => {
                    ctx.device(
                        &device.id,
                        "info",
                        format!("`{}` knows {} address(es)", device.name, found.len()),
                    );
                    for mut candidate in found {
                        candidate.path = chain.clone();
                        candidates.push(candidate);
                    }
                }
                Err(error) => ctx.device(
                    &device.id,
                    "warn",
                    format!("`{}` could not be read: {}", device.name, error.message),
                ),
            }
            ctx.advance(format!("read `{}`", device.name));
        }

        // The inventory changes under us when `add` is on, so it is re-read
        // at every level rather than cached across the whole crawl.
        let known = Known::load(&app)?;
        let mut seen_here: HashSet<String> = HashSet::new();
        for candidate in candidates {
            ctx.check()?;
            let address = candidate.address.to_string();
            if !seen_here.insert(address.clone()) || probed.contains(&address) {
                continue;
            }
            if let Some((device_id, _)) = known.get(&address).cloned() {
                // Already managed. It is still reported — "this is SW2, which
                // you already have" is worth knowing, and it is what makes a
                // crawl's coverage readable — but nothing is sent to it, and
                // it becomes a seed for the next level instead.
                let mut found = Found {
                    address: address.clone(),
                    via: "neighbor".into(),
                    found_at: util::now_ms(),
                    told_by: Some(candidate.told_by.clone()),
                    path: candidate.path.clone(),
                    ..Found::default()
                };
                found.say("told_by", "neighbor", describe(&candidate.told_by));
                finish_find(&app, &mut found, &known, &ignored, "", &candidate.role);
                state.push_live(&scan.id, found.clone());
                hosts.push(found);
                if !visited.contains(&device_id) {
                    next.push((device_id, candidate.path.clone()));
                }
                continue;
            }
            if probed.len() >= MAX_CANDIDATES {
                ctx.warn(format!(
                    "stopping at {MAX_CANDIDATES} candidates; narrow the seeds or the depth to go further"
                ));
                break;
            }
            probed.insert(address.clone());

            let mut found = probe_candidate(&context, &candidate).await;
            finish_find(
                &app,
                &mut found,
                &known,
                &ignored,
                &seed_site(&app, &candidate),
                &candidate.role,
            );
            app.publish(
                "discovery.found",
                &json!({"scan_id": scan.id, "host": found}),
            );
            state.push_live(&scan.id, found.clone());

            if add && found.identified() && !found.ignored {
                match add_device(&app, &found, &candidate) {
                    Ok(device_id) => {
                        ctx.info(format!(
                            "added `{}` ({}) found through `{}`",
                            found.suggested.name, found.address, candidate.told_by.device_name
                        ));
                        found.device_id = Some(device_id.clone());
                        next.push((device_id, candidate.path.clone()));
                    }
                    Err(error) => ctx.warn(format!(
                        "`{}` could not be added: {}",
                        found.address, error.message
                    )),
                }
            } else if add && !found.identified() {
                ctx.info(format!(
                    "`{}` answered but no device type matched it, so it was not added or crawled",
                    found.address
                ));
            }
            hosts.push(found);
        }
        frontier = next;
        let _ = db::put_hosts(&app, &scan.id, &state.live(&scan.id).unwrap_or_default());
    }

    let hosts = {
        let mut live = state.end_live(&scan.id);
        if live.is_empty() {
            live = hosts;
        }
        live.sort_by_key(|host| host.address.parse::<IpAddr>().ok());
        live
    };
    db::put_hosts(&app, &scan.id, &hosts)?;
    scan.planned = hosts.len() as u32;
    scan.probed = probed.len() as u32;
    scan.found = hosts.len() as u32;
    scan.identified = hosts.iter().filter(|host| host.identified()).count() as u32;
    scan.new = hosts
        .iter()
        .filter(|host| host.device_id.is_none() && !host.ignored)
        .count() as u32;
    scan.probes_sent = context.rate.sent();
    scan.finished_at = Some(util::now_ms());
    scan.state = "finished".into();
    // In place, so that the job id written after this task started is kept
    // and a crawl deleted while it ran stays deleted.
    let summarised = &scan;
    let stored = db::update_scan(&app, &scan.id, |record| {
        record.planned = summarised.planned;
        record.probed = summarised.probed;
        record.found = summarised.found;
        record.identified = summarised.identified;
        record.new = summarised.new;
        record.probes_sent = summarised.probes_sent;
        record.finished_at = summarised.finished_at;
        record.state = summarised.state.clone();
    })?;
    if stored.is_none() {
        ctx.info("this crawl was deleted while it ran; its results were dropped");
        return Ok(Value::Null);
    }
    db::prune(&app, DiscoverySettings::load(&app).keep_scans as usize)?;

    let summary = json!({
        "scan_id": scan.id,
        "state": scan.state,
        "seeds_read": visited.len(),
        "probed": scan.probed,
        "found": scan.found,
        "identified": scan.identified,
        "new": scan.new,
        "probes_sent": scan.probes_sent,
    });
    app.publish(
        "discovery.finished",
        &json!({"scan_id": scan.id, "summary": summary}),
    );
    Ok(summary)
}

/// Read one device's neighbour, ARP and route tables through the core.
async fn read_tables(
    ctx: &JobCtx,
    device: &Device,
    use_arp: bool,
    use_routes: bool,
) -> AppResult<Vec<Candidate>> {
    let app = ctx.app().clone();
    let mut kinds = vec![Kind::Neighbors];
    if use_arp {
        kinds.push(Kind::Arp);
    }
    if use_routes {
        kinds.push(Kind::Routes);
    }
    // A device that cannot be read at all is reported by the caller; one that
    // could be read but has no table for a kind simply contributes nothing.
    let snapshot = ctx.race(collect::run(&app, &device.id, &kinds)).await?;
    for problem in &snapshot.problems {
        ctx.device(
            &device.id,
            "warn",
            format!("`{}`: {}", device.name, problem.message),
        );
    }

    let mut candidates = Vec::new();
    {
        for neighbor in snapshot.neighbors.clone().unwrap_or_default() {
            let Some(address) = parse_address(&neighbor.address) else {
                continue;
            };
            candidates.push(Candidate {
                address,
                role: role_from(&neighbor.capabilities),
                told_by: ToldBy {
                    device_id: device.id.clone(),
                    device_name: device.name.clone(),
                    protocol: if neighbor.protocol.is_empty() {
                        "neighbor".into()
                    } else {
                        neighbor.protocol.clone()
                    },
                    local_interface: neighbor.local_interface.clone(),
                    remote_interface: neighbor.remote_interface.clone(),
                    reported_name: neighbor.device.clone(),
                    capabilities: neighbor.capabilities.clone(),
                },
                path: Vec::new(),
                mac: None,
            });
        }
    }
    if use_arp {
        let arp = snapshot.arp.clone().unwrap_or_default();
        if arp.len() > ARP_PER_DEVICE {
            ctx.device(
                &device.id,
                "info",
                format!(
                    "`{}` knows {} ARP entries; only the first {ARP_PER_DEVICE} are candidates",
                    device.name,
                    arp.len()
                ),
            );
        }
        for entry in arp.into_iter().take(ARP_PER_DEVICE) {
            let Some(address) = parse_address(&entry.ip) else {
                continue;
            };
            candidates.push(Candidate {
                address,
                role: String::new(),
                told_by: ToldBy {
                    device_id: device.id.clone(),
                    device_name: device.name.clone(),
                    protocol: "arp".into(),
                    local_interface: entry.interface.clone(),
                    ..ToldBy::default()
                },
                path: Vec::new(),
                mac: Some(entry.mac.clone()).filter(|mac| !mac.is_empty()),
            });
        }
    }
    if use_routes {
        let mut hops = HashSet::new();
        for route in snapshot.routes.clone().unwrap_or_default() {
            let Some(address) = parse_address(&route.next_hop) else {
                continue;
            };
            if !hops.insert(address) || hops.len() > ROUTES_PER_DEVICE {
                continue;
            }
            candidates.push(Candidate {
                address,
                role: String::new(),
                told_by: ToldBy {
                    device_id: device.id.clone(),
                    device_name: device.name.clone(),
                    protocol: "route".into(),
                    local_interface: route.interface.clone(),
                    reported_name: route.prefix.clone(),
                    ..ToldBy::default()
                },
                path: Vec::new(),
                mac: None,
            });
        }
    }
    Ok(candidates)
}

/// Probe a candidate the way a sweep probes an address, and record who sent us.
async fn probe_candidate(context: &Context, candidate: &Candidate) -> Found {
    let mut found = identify::probe_address(context, candidate.address)
        .await
        .unwrap_or_else(|| Found {
            address: candidate.address.to_string(),
            alive: false,
            via: "neighbor".into(),
            found_at: util::now_ms(),
            ..Found::default()
        });
    if !found.alive {
        // It did not answer us. That is not the same as "it is not there":
        // a device Nocturne manages had it in a table, and the record says so.
        found.say(
            "alive",
            "neighbor",
            format!(
                "did not answer any probe; `{}` has it in its {} table",
                candidate.told_by.device_name, candidate.told_by.protocol
            ),
        );
        identify::identify(context, candidate.address, &mut found).await;
    }
    found.say("told_by", "neighbor", describe(&candidate.told_by));
    if let Some(reported) = &candidate.mac {
        // Devices spell MAC addresses in their own notation (`001b.21aa.bbcc`
        // on IOS); the record keeps one spelling and the evidence keeps the
        // device's own words.
        let parsed = nocturne_tools::mac::info(reported).ok();
        let mac = parsed
            .as_ref()
            .map(|info| info.colon.clone())
            .unwrap_or_else(|| reported.clone());
        found.say(
            "mac",
            "neighbor",
            format!(
                "`{}` has {} at {reported}",
                candidate.told_by.device_name, found.address
            ),
        );
        if let Some(vendor) = parsed.as_ref().and_then(|info| info.vendor.clone()) {
            found.say(
                "vendor",
                "oui",
                format!("the OUI registry assigns {mac} to {vendor}"),
            );
            found.vendor = Some(vendor);
        }
        found.mac = Some(mac);
    }
    found.told_by = Some(candidate.told_by.clone());
    found.path = candidate.path.clone();
    found
}

fn describe(told: &ToldBy) -> String {
    let mut sentence = format!(
        "`{}` listed it in its {} table",
        told.device_name, told.protocol
    );
    if !told.reported_name.is_empty() {
        sentence.push_str(&format!(" as `{}`", told.reported_name));
    }
    if !told.local_interface.is_empty() {
        sentence.push_str(&format!(" on {}", told.local_interface));
    }
    if !told.remote_interface.is_empty() {
        sentence.push_str(&format!(" (its {})", told.remote_interface));
    }
    sentence
}

/// A role from what the neighbour said it can do — the only honest source of
/// one. Capabilities are the device's own word, spelled the way CDP and LLDP
/// spell them.
fn role_from(capabilities: &str) -> String {
    let lower = capabilities.to_lowercase();
    let has = |word: &str| {
        lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|part| part == word)
    };
    if has("router") || has("r") {
        "router".into()
    } else if has("switch") || has("bridge") || has("s") || has("b") {
        "switch".into()
    } else if has("wlan") || has("ap") || has("w") {
        "wireless".into()
    } else if has("phone") || has("p") {
        "phone".into()
    } else {
        String::new()
    }
}

fn seed_site(app: &App, candidate: &Candidate) -> String {
    inventory::get(app, &candidate.told_by.device_id)
        .map(|device| device.site)
        .unwrap_or_default()
}

/// Add a crawled device with the seed's credential and site.
fn add_device(app: &App, found: &Found, candidate: &Candidate) -> AppResult<String> {
    let seed = inventory::get(app, &candidate.told_by.device_id)?;
    let existing: Vec<Device> = app.store().list(DEVICES)?;
    let mut name = found.suggested.name.clone();
    if existing
        .iter()
        .any(|device| device.name.eq_ignore_ascii_case(&name))
    {
        name = format!("{name} ({})", found.address);
    }
    let transport = match found.suggested.transport.as_str() {
        "ssh" => Transport::Ssh,
        "telnet" => Transport::Telnet,
        _ => Transport::None,
    };
    let device = inventory::create(
        app,
        DeviceInput {
            name: Some(name),
            address: Some(found.address.clone()),
            profile: Some(found.suggested.profile.clone()),
            transport: Some(transport),
            credential_id: seed.credential_id.clone(),
            site: Some(seed.site.clone()),
            role: Some(found.suggested.role.clone()),
            notes: Some(format!(
                "Found by a neighbour crawl: {}",
                describe(&candidate.told_by)
            )),
            monitor: Some(MonitorSpec {
                enabled: true,
                icmp: true,
                snmp: found.snmp.is_some(),
                snmp_credential_id: found.snmp.as_ref().map(|snmp| snmp.credential_id.clone()),
                ..MonitorSpec::default()
            }),
            ..DeviceInput::default()
        },
    )?;
    Ok(device.device.id)
}

fn parse_address(text: &str) -> Option<IpAddr> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let address: IpAddr = text.split('/').next()?.parse().ok()?;
    // The same refusals a sweep makes: never probe multicast, broadcast or
    // the unspecified address, whatever a device's table says.
    if crate::targets::unprobeable(address).is_some() || address.is_loopback() {
        return None;
    }
    Some(address)
}

pub fn register(router: &mut Router) {
    router.add(
        "discovery.crawl",
        |app: App, request: CrawlRequest| async move { start(&app, request).await },
    );
}
