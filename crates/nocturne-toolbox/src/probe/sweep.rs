//! `probe.sweep` — which addresses in a range answer.
//!
//! A sweep sends packets to machines nobody asked Nocturne to talk to, so it
//! is held to the same leash as discovery: the targets are vetted before
//! anything is sent, multicast and broadcast addresses are never probed,
//! public address space needs `allow_public` on the request **and** in the
//! settings, and a range larger than the cap is refused rather than quietly
//! truncated — somebody who asks for a /16 and gets its first 4096 addresses
//! would believe the rest of it is empty.
//!
//! This is the deliberately smaller cousin of `discovery.scan`: it answers
//! "what is on this wire?" and nothing more. Identification, evidence and
//! adding devices to the inventory are the discovery feature's, and features
//! do not reach into each other.

use std::net::IpAddr;

use nocturne_app::jobs::{self, JobCtx, JobSpec};
use nocturne_app::{App, AppError, AppResult, Router};
use nocturne_netprobe::ping::Pinger;
use nocturne_netprobe::sweep::{self, HostResult, SweepOptions};
use nocturne_netprobe::tcp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc::unbounded_channel;

use crate::errors::probe as probe_error;
use crate::probe::history::{self, Run};
use crate::probe::known::{DeviceHint, Known};
use crate::settings::ToolboxSettings;

/// Hosts streamed as events before the UI is asked to read the result
/// instead. The bus is shared with every other window.
const STREAMED_HOSTS: usize = 200;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SweepParams {
    /// `10.0.0.0/24`, `10.0.1.5-10.0.1.40`, single addresses, comma separated.
    pub targets: String,
    pub timeout_ms: Option<u64>,
    pub concurrency: Option<usize>,
    /// Look up the name of every address that answers.
    pub resolve_names: Option<bool>,
    /// TCP ports to try on an address that ignores echo requests.
    pub ports: Option<String>,
    /// Sweep address space that is not private. Refused unless the settings
    /// allow it too.
    pub allow_public: bool,
}

#[derive(Debug, Serialize)]
pub struct SweepResult {
    pub targets: String,
    /// The addresses that answered, with whose they are when Nocturne knows.
    pub hosts: Vec<SweptHost>,
    /// How many addresses were probed.
    pub planned: usize,
    pub alive: usize,
    pub from: &'static str,
    pub run_id: String,
}

#[derive(Debug, Serialize)]
pub struct SweptHost {
    #[serde(flatten)]
    pub host: HostResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceHint>,
}

/// What a sweep will probe, and what it refused on the way.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub addresses: Vec<IpAddr>,
    /// One line per target as typed, for the job log.
    pub notes: Vec<String>,
    pub public: bool,
}

/// Why an address is never probed, or `None` when it may be.
fn unprobeable(address: IpAddr) -> Option<&'static str> {
    match address {
        IpAddr::V4(v4) if v4.is_unspecified() => Some("the unspecified address"),
        IpAddr::V4(v4) if v4.is_multicast() => Some("a multicast address"),
        IpAddr::V4(v4) if v4.is_broadcast() => Some("the broadcast address"),
        IpAddr::V6(v6) if v6.is_unspecified() => Some("the unspecified address"),
        IpAddr::V6(v6) if v6.is_multicast() => Some("a multicast address"),
        _ => None,
    }
}

/// Whether an address belongs to space an operator plausibly owns: RFC 1918,
/// carrier-grade NAT, link-local, loopback, IPv6 unique-local.
fn is_internal(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            v4.is_private()
                || v4.is_link_local()
                || v4.is_loopback()
                || (octets[0] == 100 && (64..128).contains(&octets[1]))
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
        }
    }
}

/// Expand and vet `targets`. Nothing is sent before this has succeeded.
pub fn plan(targets: &str, max_hosts: u32, allow_public: bool) -> AppResult<Plan> {
    let limit = max_hosts as usize;
    let tokens: Vec<&str> = targets
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() {
        return Err(AppError::invalid(
            "give the sweep something to look at: a CIDR block (10.0.0.0/24), a range (10.0.1.5-10.0.1.40) or addresses",
        ));
    }
    let mut plan = Plan::default();
    let mut seen = std::collections::HashSet::new();
    for token in tokens {
        let before = plan.addresses.len();
        // The address maths is the engine's; the judgement is ours.
        let expanded = sweep::expand_targets(token, limit)
            .map_err(|error| AppError::invalid(format!("`{token}`: {error}")))?;
        for address in expanded {
            if let Some(reason) = unprobeable(address) {
                return Err(AppError::invalid(format!(
                    "`{token}` includes {address}, which is {reason}"
                )));
            }
            if !is_internal(address) {
                if !allow_public {
                    return Err(AppError::forbidden(format!(
                        "`{token}` includes {address}, which is a public address. Sweeping address space that is not yours is rarely what you meant; if it really is, allow it under Settings → Toolbox and ask for it with `allow_public`"
                    )).with_details(json!({"address": address.to_string(), "needs": "allow_public"})));
                }
                plan.public = true;
            }
            if seen.insert(address) {
                plan.addresses.push(address);
            }
            if plan.addresses.len() > limit {
                return Err(AppError::invalid(format!(
                    "those targets cover more than {limit} addresses between them; one sweep may probe at most {limit}. Sweep them in pieces, or raise `sweep.max_hosts` under Settings → Toolbox"
                )));
            }
        }
        plan.notes.push(format!(
            "`{token}`: {} addresses",
            plan.addresses.len() - before
        ));
    }
    Ok(plan)
}

async fn start(app: &App, params: SweepParams) -> AppResult<Value> {
    let settings = ToolboxSettings::load(app).sweep;
    if params.allow_public && !settings.allow_public {
        return Err(AppError::forbidden(
            "this installation does not allow sweeping public address space. Turn on `sweep.allow_public` under Settings → Toolbox first",
        ));
    }
    let plan = plan(&params.targets, settings.max_hosts, params.allow_public)?;
    let ports = params
        .ports
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(&settings.tcp_fallback_ports);
    let options = SweepOptions {
        concurrency: params
            .concurrency
            .unwrap_or(settings.concurrency)
            .clamp(1, 512),
        timeout_ms: params
            .timeout_ms
            .unwrap_or(settings.timeout_ms)
            .clamp(50, 30_000),
        retries: 1,
        tcp_fallback_ports: tcp::parse_ports(ports).map_err(probe_error)?,
        resolve_names: params.resolve_names.unwrap_or(true),
        limit: plan.addresses.len().max(1),
    };

    let targets = params.targets.trim().to_string();
    let title = format!("Sweep {} ({} addresses)", targets, plan.addresses.len());
    let mut run = Run::plain("sweep", title.clone(), history::where_from(app), &targets);
    history::begin(app, &run);
    let run_id = run.id.clone();
    let known = Known::load(app);
    let public = plan.public;
    let notes = plan.notes.clone();
    let planned = plan.addresses.len();

    app.audit(
        "probe.sweep",
        None,
        format!("Swept `{targets}`"),
        json!({"addresses": planned, "allow_public": public, "run_id": run.id}),
    );
    let job = jobs::start(
        app,
        JobSpec::new("probe.sweep", title).total(planned as u64),
        move |ctx| async move {
            for note in &notes {
                ctx.info(note);
            }
            if public {
                ctx.warn("this sweep covers public address space, as `allow_public` asked");
            }
            let outcome = run_sweep(&ctx, &targets, &options, &known, planned, &mut run).await;
            match &outcome {
                Ok(result) => history::succeeded(
                    ctx.app(),
                    &mut run,
                    format!("{} of {} addresses answered", result.alive, result.planned),
                    result,
                ),
                Err(error) => history::failed(ctx.app(), &mut run, error),
            }
            outcome.and_then(|result| Ok(serde_json::to_value(result)?))
        },
    );
    Ok(json!({"job_id": job.id, "run_id": run_id}))
}

async fn run_sweep(
    ctx: &JobCtx,
    targets: &str,
    options: &SweepOptions,
    known: &Known,
    planned: usize,
    run: &mut Run,
) -> AppResult<SweepResult> {
    run.job_id = Some(ctx.id().to_string());
    history::running(ctx.app(), run);
    let (sender, mut receiver) = unbounded_channel::<HostResult>();
    let forwarder = {
        let (ctx, run_id, known) = (ctx.clone(), run.id.clone(), known.clone());
        tokio::spawn(async move {
            let mut streamed = 0usize;
            let mut alive = 0usize;
            while let Some(host) = receiver.recv().await {
                alive += 1;
                let device = known.of_text(&host.address);
                ctx.progress(
                    alive as u64,
                    match &device {
                        Some(device) => format!("{} is {}", host.address, device.name),
                        None => format!("{} answered", host.address),
                    },
                );
                if streamed < STREAMED_HOSTS {
                    streamed += 1;
                    ctx.app().publish(
                        "probe.sweep.host",
                        &json!({"job_id": ctx.id(), "run_id": run_id, "host": host, "device": device}),
                    );
                    if streamed == STREAMED_HOSTS {
                        ctx.info(
                            "that is the first 200 answers; the rest are in the result of this job",
                        );
                    }
                }
            }
        })
    };
    let pinger = Pinger::new();
    let outcome = ctx
        .race(async {
            sweep::sweep(&pinger, targets, options, Some(sender.clone()))
                .await
                .map_err(probe_error)
        })
        .await;
    drop(sender);
    let _ = forwarder.await;
    let hosts = outcome?;
    run.method = serde_json::to_value(pinger.method())
        .ok()
        .and_then(|value| value.as_str().map(str::to_string));
    Ok(SweepResult {
        targets: targets.to_string(),
        alive: hosts.len(),
        planned,
        hosts: hosts
            .into_iter()
            .map(|host| SweptHost {
                device: known.of_text(&host.address),
                host,
            })
            .collect(),
        from: history::where_from(ctx.app()),
        run_id: run.id.clone(),
    })
}

pub fn register(router: &mut Router) {
    router.add("probe.sweep", |app: App, params: SweepParams| async move {
        start(&app, params).await
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_expands_private_space_and_refuses_the_rest() {
        let swept = plan("10.0.0.0/30, 192.168.1.5", 100, false).unwrap();
        // A /30 has two host addresses; the single address makes three.
        assert_eq!(
            swept
                .addresses
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["10.0.0.1", "10.0.0.2", "192.168.1.5"]
        );
        assert!(!swept.public);
        assert_eq!(swept.notes.len(), 2);

        // The same address twice is probed once.
        assert_eq!(
            plan("10.0.0.5, 10.0.0.5", 10, false)
                .unwrap()
                .addresses
                .len(),
            1
        );
    }

    #[test]
    fn public_space_needs_asking_and_multicast_is_never_probed() {
        let refused = plan("8.8.8.8", 100, false).unwrap_err();
        assert_eq!(refused.code, "forbidden");
        assert_eq!(refused.details.unwrap()["needs"], "allow_public");
        let asked = plan("8.8.8.8", 100, true).unwrap();
        assert!(asked.public);

        for address in ["224.0.0.1", "255.255.255.255", "0.0.0.0", "ff02::1"] {
            let refused = plan(address, 100, false).unwrap_err();
            assert_eq!(refused.code, "invalid", "{address} was not refused");
            assert!(
                refused.message.contains("multicast")
                    || refused.message.contains("broadcast")
                    || refused.message.contains("unspecified"),
                "{}",
                refused.message
            );
        }
    }

    #[test]
    fn a_range_larger_than_the_cap_is_refused_rather_than_truncated() {
        let refused = plan("10.0.0.0/16", 256, false).unwrap_err();
        assert_eq!(refused.code, "invalid");
        let together = plan("10.0.0.0/24, 10.0.1.0/24", 256, false).unwrap_err();
        assert_eq!(together.code, "invalid");
        assert!(together.message.contains("256"), "{}", together.message);
        assert_eq!(plan("", 256, false).unwrap_err().code, "invalid");
        assert_eq!(plan("not a range", 256, false).unwrap_err().code, "invalid");
    }
}
