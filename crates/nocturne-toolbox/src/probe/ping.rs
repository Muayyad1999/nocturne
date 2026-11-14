//! `probe.ping` — echo requests from this machine.
//!
//! The echoes stream as `probe.ping.echo` while the job runs and the report is
//! the job's result; `count: 0` keeps going until the job is cancelled, within
//! the hour the settings allow.
//!
//! What this module is careful about is the difference between *no answer* and
//! *could not ask*. `nocturne_netprobe::ping` is careful about it too — a run
//! in which nothing could be sent is an error, not a report of 100 % loss —
//! and that error is passed straight through to the job and the history
//! record, where `error_code: "forbidden"` says this machine may not send ICMP
//! at all. A report of total loss means the network was silent; it never means
//! the probe did not happen.

use std::sync::Arc;
use std::time::Duration;

use nocturne_app::jobs::{self, JobCtx, JobSpec};
use nocturne_app::{App, AppError, AppResult, Router};
use nocturne_netprobe::ping::{self, Method, PingEcho, PingOptions, PingReport, Pinger};
use nocturne_netprobe::{ProbeError, dns};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc::unbounded_channel;
use tokio::time::Instant;

use crate::errors::probe as probe_error;
use crate::probe::history::{self, Run};
use crate::probe::known::{self, DeviceHint, Known, Target};
use crate::settings::ToolboxSettings;

/// Echoes kept in the report of a run that goes on for an hour. The job log
/// says when older ones were dropped; the newest are the interesting ones.
const MAX_KEPT_ECHOES: usize = 2000;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PingParams {
    /// An address or a host name.
    pub host: Option<String>,
    /// Or a device in the inventory, by id, name or address.
    pub device: Option<String>,
    /// `0` pings until the job is cancelled.
    pub count: Option<u32>,
    pub interval_ms: Option<u64>,
    pub timeout_ms: Option<u64>,
    /// Data bytes per echo (56 is what `ping` sends).
    pub payload: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct PingResult {
    #[serde(flatten)]
    pub report: PingReport,
    /// `this computer` or `the Nocturne server`.
    pub from: &'static str,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceHint>,
    /// Echoes left out of `echoes` because the run was a long one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub echoes_dropped: Option<usize>,
}

/// The statistics of a set of echoes, for a report this crate has to build
/// itself: a run that was cancelled, or one that was never going to end.
///
/// `ping::ping` computes these when it returns a report of its own, but it has
/// no until-cancelled mode and no partial report, so the same arithmetic lives
/// here for those two cases. (Noted in the report as an engine gap.)
fn report_from(
    target: &Target,
    address: &str,
    method: Method,
    sent: u32,
    echoes: Vec<PingEcho>,
) -> (PingReport, Option<usize>) {
    let rtts: Vec<f64> = echoes.iter().filter_map(|echo| echo.rtt_ms).collect();
    let received = rtts.len() as u32;
    let count = rtts.len() as f64;
    let avg = (!rtts.is_empty()).then(|| rtts.iter().sum::<f64>() / count);
    let round = |value: f64| (value * 1000.0).round() / 1000.0;
    let dropped = echoes.len().saturating_sub(MAX_KEPT_ECHOES);
    let kept = echoes.into_iter().skip(dropped).collect();
    (
        PingReport {
            target: target.input.clone(),
            address: address.to_string(),
            method,
            sent,
            received,
            loss_pct: if sent == 0 {
                0.0
            } else {
                round(f64::from(sent - received) * 100.0 / f64::from(sent))
            },
            min_ms: rtts.iter().copied().reduce(f64::min).map(round),
            max_ms: rtts.iter().copied().reduce(f64::max).map(round),
            avg_ms: avg.map(round),
            stddev_ms: avg.map(|avg| {
                round((rtts.iter().map(|rtt| (rtt - avg).powi(2)).sum::<f64>() / count).sqrt())
            }),
            echoes: kept,
        },
        (dropped > 0).then_some(dropped),
    )
}

fn summary_of(report: &PingReport) -> String {
    let rtt = match report.avg_ms {
        Some(avg) => format!(", avg {avg:.1} ms"),
        None => String::new(),
    };
    format!(
        "{}/{} replies, {:.0}% loss{rtt}",
        report.received, report.sent, report.loss_pct
    )
}

/// `true` for a failure of this machine rather than silence from the network.
///
/// The same judgement `ping::ping` makes internally: a timeout, an ICMP
/// unreachable or a refusal is an *answer* about the network, anything else is
/// this machine being unable to ask.
fn is_local(error: &ProbeError) -> bool {
    !matches!(
        error,
        ProbeError::Timeout | ProbeError::Unreachable(_) | ProbeError::Refused
    )
}

async fn start(app: &App, params: PingParams) -> AppResult<Value> {
    let settings = ToolboxSettings::load(app).probe;
    let known = Known::load(app);
    let target = known::resolve(
        app,
        params.host.as_deref(),
        params.device.as_deref(),
        &known,
    )?;

    let count = params.count.unwrap_or(settings.ping_count);
    let options = PingOptions {
        count,
        interval_ms: params
            .interval_ms
            .unwrap_or(settings.ping_interval_ms)
            .clamp(10, 60_000),
        timeout_ms: params
            .timeout_ms
            .unwrap_or(settings.ping_timeout_ms)
            .clamp(10, 60_000),
        payload: params.payload.unwrap_or(settings.ping_payload).min(65_000),
    };
    let continuous = count == 0;

    let title = match &target.device {
        Some(device) if device.name != target.input => {
            format!("Ping {} ({})", target.host, device.name)
        }
        _ => format!("Ping {}", target.host),
    };
    let mut run = Run::new("ping", title.clone(), history::where_from(app), &target);
    run.method = None;
    history::begin(app, &run);
    let run_id = run.id.clone();

    // `devices` sets the total from the device list, so the real total goes last.
    let spec = JobSpec::new("probe.ping", title);
    let spec = match &target.device {
        Some(device) => spec.devices(std::slice::from_ref(&device.id)),
        None => spec,
    };
    let spec = spec.total(if continuous { 0 } else { u64::from(count) });
    let max_run = Duration::from_secs(settings.max_run_s);
    let job = jobs::start(app, spec, move |ctx| async move {
        let outcome = run_ping(&ctx, &target, options, continuous, max_run, &mut run).await;
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

async fn run_ping(
    ctx: &JobCtx,
    target: &Target,
    options: PingOptions,
    continuous: bool,
    max_run: Duration,
    run: &mut Run,
) -> AppResult<PingResult> {
    let app = ctx.app().clone();
    run.job_id = Some(ctx.id().to_string());
    let pinger = Pinger::new();
    run.method = Some(
        serde_json::to_value(pinger.method())
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default(),
    );
    history::running(&app, run);
    ctx.info(format!(
        "{} echo requests of {} bytes, every {} ms, from {} ({})",
        if continuous {
            "until cancelled:".to_string()
        } else {
            options.count.to_string()
        },
        options.payload,
        options.interval_ms,
        run.from,
        method_name(pinger.method()),
    ));

    // The echoes are collected as they arrive, so that a cancelled run still
    // reports what it saw rather than nothing at all.
    let echoes: Arc<Mutex<Vec<PingEcho>>> = Arc::new(Mutex::new(Vec::new()));
    let (sender, mut receiver) = unbounded_channel::<PingEcho>();
    let forwarder = {
        let (echoes, ctx, run_id) = (echoes.clone(), ctx.clone(), run.id.clone());
        tokio::spawn(async move {
            let mut seq = 0u32;
            while let Some(mut echo) = receiver.recv().await {
                seq += 1;
                // Continuous mode sends one engine run per echo, so the
                // sequence number has to be this loop's. A bounded run keeps
                // the engine's, which is the order the echoes were *sent* in,
                // not the order the replies came back.
                if continuous {
                    echo.seq = seq;
                }
                ctx.app().publish(
                    "probe.ping.echo",
                    &json!({"job_id": ctx.id(), "run_id": run_id, "echo": echo}),
                );
                match echo.rtt_ms {
                    Some(rtt) => ctx.advance(format!("reply from {} in {rtt:.1} ms", echo.seq)),
                    None => ctx.advance("no reply".to_string()),
                }
                echoes.lock().push(echo);
            }
        })
    };

    let address = dns::resolve(&target.host)
        .await
        .map_err(probe_error)?
        .first()
        .copied()
        .ok_or_else(|| AppError::not_found(format!("an address for `{}`", target.host)))?;
    run.address = Some(address.to_string());

    let outcome = if continuous {
        continuous_ping(ctx, &pinger, target, options, max_run, &sender).await
    } else {
        ctx.race(async {
            ping::ping(&pinger, &target.host, &options, Some(sender.clone()))
                .await
                .map_err(probe_error)
        })
        .await
        .map(Some)
        .or_else(|error| {
            // Cancelled: the partial report below is the honest answer.
            (error.code == "cancelled").then_some(None).ok_or(error)
        })
    };
    drop(sender);
    let _ = forwarder.await;
    let collected = std::mem::take(&mut *echoes.lock());

    let report = match outcome? {
        Some(report) => (report, None),
        None => {
            let sent = collected.len() as u32;
            report_from(
                target,
                &address.to_string(),
                pinger.method(),
                sent,
                collected,
            )
        }
    };
    Ok(PingResult {
        from: history::where_from(&app),
        run_id: run.id.clone(),
        device: target.device.clone(),
        echoes_dropped: report.1,
        report: report.0,
    })
}

/// Ping until the job is cancelled or the hour is up.
///
/// One engine run per echo, so that resolution, the socket method and the
/// "this machine cannot send at all" judgement stay the engine's; the loop
/// here only counts and paces.
async fn continuous_ping(
    ctx: &JobCtx,
    pinger: &Pinger,
    target: &Target,
    options: PingOptions,
    max_run: Duration,
    sender: &tokio::sync::mpsc::UnboundedSender<PingEcho>,
) -> AppResult<Option<PingReport>> {
    let one = PingOptions {
        count: 1,
        ..options
    };
    let deadline = Instant::now() + max_run;
    let interval = Duration::from_millis(options.interval_ms);
    let mut first = true;
    loop {
        if ctx.is_cancelled() {
            return Ok(None);
        }
        if Instant::now() >= deadline {
            ctx.info(format!(
                "stopping after {} s, which is as long as one run may last (Settings → Toolbox)",
                max_run.as_secs()
            ));
            return Ok(None);
        }
        let due = Instant::now() + interval;
        match ping::ping(pinger, &target.host, &one, None).await {
            Ok(report) => {
                if let Some(echo) = report.echoes.into_iter().next() {
                    let _ = sender.send(echo);
                }
            }
            // A failure of this machine ends the run with its reason; silence
            // from the network is an echo without a reply, which the engine
            // has already reported as such.
            Err(error) if is_local(&error) && first => return Err(probe_error(error)),
            Err(error) => {
                ctx.warn(error.to_string());
                let _ = sender.send(PingEcho {
                    seq: 0,
                    rtt_ms: None,
                    ttl: None,
                    error: Some(error.to_string()),
                });
            }
        }
        first = false;
        tokio::select! {
            _ = ctx.token().cancelled() => return Ok(None),
            _ = tokio::time::sleep_until(due) => {}
        }
    }
}

fn method_name(method: Method) -> &'static str {
    match method {
        Method::IcmpDatagram => "unprivileged ICMP sockets",
        Method::IcmpRaw => "raw ICMP sockets",
        Method::SystemCommand => "the system ping program",
    }
}

pub fn register(router: &mut Router) {
    router.add("probe.ping", |app: App, params: PingParams| async move {
        start(&app, params).await
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo(seq: u32, rtt: Option<f64>) -> PingEcho {
        PingEcho {
            seq,
            rtt_ms: rtt,
            ttl: Some(64),
            error: rtt.is_none().then(|| "timed out".to_string()),
        }
    }

    #[test]
    fn a_partial_report_counts_what_was_seen_and_nothing_else() {
        let target = Target {
            input: "10.0.0.1".into(),
            host: "10.0.0.1".into(),
            device: None,
        };
        let (report, dropped) = report_from(
            &target,
            "10.0.0.1",
            Method::IcmpDatagram,
            4,
            vec![
                echo(1, Some(1.0)),
                echo(2, Some(3.0)),
                echo(3, None),
                echo(4, Some(2.0)),
            ],
        );
        // Hand-checked: three replies of 1, 3 and 2 ms out of four sent.
        assert_eq!((report.sent, report.received), (4, 3));
        assert_eq!(report.loss_pct, 25.0);
        assert_eq!(
            (report.min_ms, report.avg_ms, report.max_ms),
            (Some(1.0), Some(2.0), Some(3.0))
        );
        assert_eq!(report.stddev_ms, Some(0.816));
        assert_eq!(dropped, None);
        assert_eq!(summary_of(&report), "3/4 replies, 25% loss, avg 2.0 ms");

        // Nothing came back at all: no average, and no invented one.
        let (silent, _) = report_from(&target, "10.0.0.1", Method::IcmpRaw, 2, vec![echo(1, None)]);
        assert_eq!((silent.received, silent.loss_pct), (0, 100.0));
        assert_eq!((silent.avg_ms, silent.stddev_ms), (None, None));
        assert_eq!(summary_of(&silent), "0/2 replies, 100% loss");
    }

    #[test]
    fn a_very_long_run_keeps_the_newest_echoes_and_says_how_many_it_dropped() {
        let target = Target {
            input: "10.0.0.1".into(),
            host: "10.0.0.1".into(),
            device: None,
        };
        let many: Vec<PingEcho> = (0..MAX_KEPT_ECHOES as u32 + 10)
            .map(|seq| echo(seq + 1, Some(1.0)))
            .collect();
        let (report, dropped) = report_from(
            &target,
            "10.0.0.1",
            Method::IcmpDatagram,
            many.len() as u32,
            many,
        );
        assert_eq!(dropped, Some(10));
        assert_eq!(report.echoes.len(), MAX_KEPT_ECHOES);
        assert_eq!(report.echoes[0].seq, 11, "the oldest went, not the newest");
        assert_eq!(
            report.received as usize,
            MAX_KEPT_ECHOES + 10,
            "the counts are of what was sent, not of what was kept"
        );
    }

    #[test]
    fn a_local_failure_is_told_from_network_silence() {
        assert!(!is_local(&ProbeError::Timeout));
        assert!(!is_local(&ProbeError::Unreachable("no route".into())));
        assert!(!is_local(&ProbeError::Refused));
        assert!(is_local(&ProbeError::Permission("no CAP_NET_RAW".into())));
        assert!(is_local(&ProbeError::Unsupported("no ping".into())));
    }
}
