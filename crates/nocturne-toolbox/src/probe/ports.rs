//! `probe.ports` — which TCP ports of a host are open.
//!
//! A connect scan: the engine opens a socket, reads whatever the server
//! volunteers, and for an HTTP-looking port sends one `HEAD / HTTP/1.0`. It
//! sends nothing else, ever, and this crate asks for nothing else.
//!
//! Open ports stream as `probe.port` while the job runs; the result lists
//! every port that was tried, so "closed" and "not tried" stay different
//! things.

use nocturne_app::jobs::{self, JobCtx, JobSpec};
use nocturne_app::{App, AppError, AppResult, Router};
use nocturne_netprobe::tcp::{self, PortResult, PortState, ScanOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc::unbounded_channel;

use crate::errors::probe as probe_error;
use crate::probe::history::{self, Run};
use crate::probe::known::{self, DeviceHint, Known};
use crate::settings::ToolboxSettings;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PortsParams {
    pub host: Option<String>,
    pub device: Option<String>,
    /// `22,80,443`, `1-1024`, `top100`, `common`, or a mix of those.
    pub ports: Option<String>,
    pub timeout_ms: Option<u64>,
    /// Read the banner a server volunteers (on by default).
    pub banners: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct PortsResult {
    /// Every port that was tried, in the order they were given.
    pub ports: Vec<PortResult>,
    /// Just the open ones, for a result line.
    pub open: Vec<u16>,
    pub scanned: usize,
    pub host: String,
    pub from: &'static str,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceHint>,
}

fn summary_of(result: &PortsResult) -> String {
    if result.open.is_empty() {
        return format!("none of {} ports open", result.scanned);
    }
    let listed: Vec<String> = result.open.iter().take(10).map(u16::to_string).collect();
    format!(
        "{} of {} open: {}{}",
        result.open.len(),
        result.scanned,
        listed.join(", "),
        if result.open.len() > listed.len() {
            ", …"
        } else {
            ""
        }
    )
}

async fn start(app: &App, params: PortsParams) -> AppResult<Value> {
    let settings = ToolboxSettings::load(app).probe;
    let known = Known::load(app);
    let target = known::resolve(
        app,
        params.host.as_deref(),
        params.device.as_deref(),
        &known,
    )?;

    let spec = params
        .ports
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(&settings.ports)
        .to_string();
    let ports = tcp::parse_ports(&spec).map_err(probe_error)?;
    if ports.len() > settings.max_ports as usize {
        return Err(AppError::invalid(format!(
            "`{spec}` is {} ports; this check may try at most {} (raise `probe.max_ports` under Settings → Toolbox)",
            ports.len(),
            settings.max_ports
        )));
    }
    let options = ScanOptions {
        timeout_ms: params
            .timeout_ms
            .unwrap_or(settings.port_timeout_ms)
            .clamp(10, 60_000),
        concurrency: settings.port_concurrency,
        grab_banners: params.banners.unwrap_or(true),
    };

    let title = format!("Port check of {} ({} ports)", target.host, ports.len());
    let mut run = Run::new("ports", title.clone(), history::where_from(app), &target);
    history::begin(app, &run);
    let run_id = run.id.clone();

    let job_spec = JobSpec::new("probe.ports", title);
    let job_spec = match &target.device {
        Some(device) => job_spec.devices(std::slice::from_ref(&device.id)),
        None => job_spec,
    };
    let job_spec = job_spec.total(ports.len() as u64);
    let job = jobs::start(app, job_spec, move |ctx| async move {
        let outcome = run_scan(&ctx, &target, &ports, &options, &mut run).await;
        match &outcome {
            Ok(result) => history::succeeded(ctx.app(), &mut run, summary_of(result), result),
            Err(error) => history::failed(ctx.app(), &mut run, error),
        }
        outcome.and_then(|result| Ok(serde_json::to_value(result)?))
    });
    Ok(json!({"job_id": job.id, "run_id": run_id}))
}

async fn run_scan(
    ctx: &JobCtx,
    target: &known::Target,
    ports: &[u16],
    options: &ScanOptions,
    run: &mut Run,
) -> AppResult<PortsResult> {
    run.job_id = Some(ctx.id().to_string());
    ctx.info(format!(
        "{} ports on {}, {} at a time, from {}",
        ports.len(),
        target.host,
        options.concurrency,
        run.from
    ));
    let (sender, mut receiver) = unbounded_channel::<PortResult>();
    let forwarder = {
        let (ctx, run_id) = (ctx.clone(), run.id.clone());
        tokio::spawn(async move {
            while let Some(port) = receiver.recv().await {
                ctx.status(format!("{} is open", port.port));
                ctx.app().publish(
                    "probe.port",
                    &json!({"job_id": ctx.id(), "run_id": run_id, "port": port}),
                );
            }
        })
    };
    let outcome = ctx
        .race(async {
            tcp::scan(&target.host, ports, options, Some(sender.clone()))
                .await
                .map_err(probe_error)
        })
        .await;
    drop(sender);
    let _ = forwarder.await;
    let results = outcome?;
    let open: Vec<u16> = results
        .iter()
        .filter(|port| port.state == PortState::Open)
        .map(|port| port.port)
        .collect();
    Ok(PortsResult {
        scanned: results.len(),
        open,
        host: target.host.clone(),
        from: history::where_from(ctx.app()),
        run_id: run.id.clone(),
        device: target.device.clone(),
        ports: results,
    })
}

pub fn register(router: &mut Router) {
    router.add("probe.ports", |app: App, params: PortsParams| async move {
        start(&app, params).await
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(open: Vec<u16>, scanned: usize) -> PortsResult {
        PortsResult {
            ports: Vec::new(),
            open,
            scanned,
            host: "10.0.0.1".into(),
            from: "this computer",
            run_id: "r".into(),
            device: None,
        }
    }

    #[test]
    fn the_summary_counts_the_ports_that_were_tried_not_the_ones_that_answered() {
        assert_eq!(
            summary_of(&result(vec![22, 443], 20)),
            "2 of 20 open: 22, 443"
        );
        assert_eq!(summary_of(&result(vec![], 20)), "none of 20 ports open");
        let many: Vec<u16> = (1..=12).collect();
        assert!(summary_of(&result(many, 100)).ends_with(", …"));
    }
}
