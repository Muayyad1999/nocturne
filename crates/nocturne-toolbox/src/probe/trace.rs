//! `probe.traceroute` — the path to a host, hop by hop.
//!
//! Hops stream as `probe.trace.hop` as they are found, and each one is looked
//! up in the inventory on the way past: a trace that says "hop 2 is CORE1,
//! hop 3 is the firewall" is worth a great deal more than one that says
//! `10.0.0.1`. A hop nobody answered for stays empty — there is no guess at
//! what is behind a silent router.

use nocturne_app::jobs::{self, JobCtx, JobSpec};
use nocturne_app::{App, AppResult, Router};
use nocturne_netprobe::traceroute::{self, Hop, TraceOptions, TraceReport};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc::unbounded_channel;

use crate::errors::probe as probe_error;
use crate::probe::history::{self, Run};
use crate::probe::known::{self, DeviceHint, Known};
use crate::settings::ToolboxSettings;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct TraceParams {
    pub host: Option<String>,
    pub device: Option<String>,
    pub max_hops: Option<u8>,
    pub probes: Option<u8>,
    pub timeout_ms: Option<u64>,
    pub resolve_names: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct TraceResult {
    #[serde(flatten)]
    pub report: TraceReport,
    pub from: &'static str,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceHint>,
    /// The hops that are devices Nocturne manages.
    pub hop_devices: Vec<HopDevice>,
}

#[derive(Debug, Serialize)]
pub struct HopDevice {
    pub ttl: u8,
    #[serde(flatten)]
    pub device: DeviceHint,
}

fn summary_of(report: &TraceReport) -> String {
    let answered = report
        .hops
        .iter()
        .filter(|hop| hop.address.is_some())
        .count();
    format!(
        "{} after {} hops ({answered} answered), {}",
        if report.reached {
            "reached"
        } else {
            "did not reach"
        },
        report.hops.len(),
        report.method
    )
}

async fn start(app: &App, params: TraceParams) -> AppResult<Value> {
    let settings = ToolboxSettings::load(app).probe;
    let known = Known::load(app);
    let target = known::resolve(
        app,
        params.host.as_deref(),
        params.device.as_deref(),
        &known,
    )?;
    let options = TraceOptions {
        max_hops: params
            .max_hops
            .unwrap_or(settings.trace_max_hops)
            .clamp(1, 64),
        probes: params.probes.unwrap_or(settings.trace_probes).clamp(1, 10),
        timeout_ms: params
            .timeout_ms
            .unwrap_or(settings.trace_timeout_ms)
            .clamp(100, 60_000),
        resolve_names: params.resolve_names.unwrap_or(settings.resolve_names),
    };

    let title = match &target.device {
        Some(device) if device.name != target.input => {
            format!("Traceroute to {} ({})", target.host, device.name)
        }
        _ => format!("Traceroute to {}", target.host),
    };
    let mut run = Run::new(
        "traceroute",
        title.clone(),
        history::where_from(app),
        &target,
    );
    history::begin(app, &run);
    let run_id = run.id.clone();

    let spec = JobSpec::new("probe.traceroute", title);
    let spec = match &target.device {
        Some(device) => spec.devices(std::slice::from_ref(&device.id)),
        None => spec,
    };
    let spec = spec.total(u64::from(options.max_hops));
    let job = jobs::start(app, spec, move |ctx| async move {
        let outcome = run_trace(&ctx, &target, options, &known, &mut run).await;
        match &outcome {
            Ok(result) => {
                history::succeeded(ctx.app(), &mut run, summary_of(&result.report), result)
            }
            Err(error) => history::failed(ctx.app(), &mut run, error),
        }
        outcome.and_then(|result| Ok(serde_json::to_value(result)?))
    });
    Ok(json!({"job_id": job.id, "run_id": run_id}))
}

async fn run_trace(
    ctx: &JobCtx,
    target: &known::Target,
    options: TraceOptions,
    known: &Known,
    run: &mut Run,
) -> AppResult<TraceResult> {
    run.job_id = Some(ctx.id().to_string());
    history::running(ctx.app(), run);
    let (sender, mut receiver) = unbounded_channel::<Hop>();
    let forwarder = {
        let (ctx, run_id) = (ctx.clone(), run.id.clone());
        // The inventory lookup is a map in memory, so it is cheap enough to
        // do on the hop that has just arrived rather than at the end.
        let known = known.clone();
        tokio::spawn(async move {
            while let Some(hop) = receiver.recv().await {
                let device = hop
                    .address
                    .as_deref()
                    .and_then(|address| known.of_text(address));
                ctx.progress(
                    u64::from(hop.ttl),
                    match (&hop.address, &device) {
                        (Some(address), Some(device)) => {
                            format!("hop {}: {address} ({})", hop.ttl, device.name)
                        }
                        (Some(address), None) => format!("hop {}: {address}", hop.ttl),
                        (None, _) => format!("hop {}: no reply", hop.ttl),
                    },
                );
                ctx.app().publish(
                    "probe.trace.hop",
                    &json!({"job_id": ctx.id(), "run_id": run_id, "hop": hop, "device": device}),
                );
            }
        })
    };

    let outcome = ctx
        .race(async {
            traceroute::traceroute(&target.host, &options, Some(sender.clone()))
                .await
                .map_err(probe_error)
        })
        .await;
    drop(sender);
    let _ = forwarder.await;
    let report = outcome?;
    run.address = Some(report.address.clone());
    run.method = Some(report.method.clone());
    let hop_devices = report
        .hops
        .iter()
        .filter_map(|hop| {
            let address = hop.address.as_deref()?;
            Some(HopDevice {
                ttl: hop.ttl,
                device: known.of_text(address)?,
            })
        })
        .collect();
    Ok(TraceResult {
        from: history::where_from(ctx.app()),
        run_id: run.id.clone(),
        device: target.device.clone(),
        hop_devices,
        report,
    })
}

pub fn register(router: &mut Router) {
    router.add(
        "probe.traceroute",
        |app: App, params: TraceParams| async move { start(&app, params).await },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_says_whether_it_arrived_and_how_it_was_traced() {
        let hop = |ttl: u8, address: Option<&str>| Hop {
            ttl,
            address: address.map(str::to_string),
            hostname: None,
            rtts_ms: vec![Some(1.0)],
            note: String::new(),
        };
        let report = TraceReport {
            target: "10.0.0.9".into(),
            address: "10.0.0.9".into(),
            hops: vec![
                hop(1, Some("10.0.0.1")),
                hop(2, None),
                hop(3, Some("10.0.0.9")),
            ],
            reached: true,
            method: "native-udp".into(),
        };
        assert_eq!(
            summary_of(&report),
            "reached after 3 hops (2 answered), native-udp"
        );
        let lost = TraceReport {
            reached: false,
            ..report
        };
        assert_eq!(
            summary_of(&lost),
            "did not reach after 3 hops (2 answered), native-udp"
        );
    }
}
