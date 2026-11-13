//! The probes that answer in the reply: DNS, WHOIS, TLS, HTTP and NTP.
//!
//! These take a second at most, so making a caller follow a job for them would
//! be ceremony — but they still leave a history record, because "what did that
//! certificate say before we replaced it?" is exactly the question the history
//! is for. Each result carries the engine's own report with `from` and
//! `run_id` beside it.
//!
//! The honesty rules apply unchanged. An NXDOMAIN is an answer and is returned
//! as one; a certificate that does not verify is described in full with
//! `trusted: false` and the reason, because inspecting a broken certificate is
//! the whole point of the tool; an HTTP 500 is a successful check with a
//! status of 500. What is never returned is a plausible-looking result for a
//! probe that did not happen.

use std::time::Duration;

use nocturne_app::{App, AppError, AppResult, Router, util};
use nocturne_netprobe::http::{HttpOptions, HttpReport};
use nocturne_netprobe::ntp::NtpReport;
use nocturne_netprobe::tls::TlsReport;
use nocturne_netprobe::whois::WhoisAnswer;
use nocturne_netprobe::{dns, http, ntp, tls, whois};
use serde::{Deserialize, Serialize};

use crate::errors::probe as probe_error;
use crate::probe::history::{self, Run};
use crate::probe::known::{self, DeviceHint, Known};
use crate::settings::ToolboxSettings;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct DnsParams {
    /// The name to look up, or an address for a `PTR` query.
    pub name: String,
    /// `A`, `AAAA`, `CNAME`, `MX`, `NS`, `TXT`, `SOA`, `SRV`, `PTR`, `CAA`, `ANY`.
    pub kind: Option<String>,
    /// A server to ask (`10.0.0.53`, `9.9.9.9:53`); left out, the system resolvers.
    pub server: Option<String>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct WhoisParams {
    /// An address, a CIDR block, an AS number (`AS13335`) or a domain.
    pub query: String,
    /// Ask this server instead of starting at IANA.
    pub server: Option<String>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct TlsParams {
    pub host: Option<String>,
    pub device: Option<String>,
    pub port: Option<u16>,
    /// The server name to send and check instead of `host`.
    pub sni: Option<String>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct HttpParams {
    pub url: String,
    pub method: Option<String>,
    /// Extra request headers. They are sent and then forgotten: a header can
    /// carry a token, so no part of the request is recorded or logged.
    pub headers: Vec<(String, String)>,
    pub follow_redirects: Option<u8>,
    pub verify_tls: Option<bool>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct NtpParams {
    /// `pool.ntp.org`, `10.0.0.1`, `10.0.0.1:123`.
    pub server: String,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct DnsResult {
    #[serde(flatten)]
    pub answer: dns::DnsAnswer,
    pub from: &'static str,
    pub run_id: String,
    /// The addresses in the answer that belong to devices Nocturne manages.
    pub devices: Vec<DeviceHint>,
}

#[derive(Debug, Serialize)]
pub struct WhoisResult {
    #[serde(flatten)]
    pub answer: WhoisAnswer,
    pub from: &'static str,
    pub run_id: String,
}

#[derive(Debug, Serialize)]
pub struct TlsResult {
    #[serde(flatten)]
    pub report: TlsReport,
    pub from: &'static str,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceHint>,
}

#[derive(Debug, Serialize)]
pub struct HttpResult {
    #[serde(flatten)]
    pub report: HttpReport,
    pub from: &'static str,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceHint>,
}

#[derive(Debug, Serialize)]
pub struct NtpResult {
    #[serde(flatten)]
    pub report: NtpReport,
    pub from: &'static str,
    pub run_id: String,
}

fn seconds_now() -> i64 {
    util::now_ms() / 1000
}

async fn lookup(app: &App, params: DnsParams) -> AppResult<DnsResult> {
    let settings = ToolboxSettings::load(app).probe;
    let name = known::host_name(&params.name)?;
    let kind = params
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
        .unwrap_or("A")
        .to_string();
    let server = match params
        .server
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        // A server may carry a port, which is not a host name, so only the
        // host part goes through the host-name check.
        Some(server) => {
            let (host, _) = server.rsplit_once(':').unwrap_or((server, ""));
            known::host_name(host.trim_matches(['[', ']']))?;
            Some(server.to_string())
        }
        None => None,
    };
    let timeout = Duration::from_millis(
        params
            .timeout_ms
            .unwrap_or(settings.dns_timeout_ms)
            .clamp(100, 60_000),
    );

    let mut run = Run::plain(
        "dns",
        format!("DNS {kind} of {name}"),
        history::where_from(app),
        &name,
    );
    run.method = server
        .clone()
        .or_else(|| Some("the system resolvers".into()));
    history::begin(app, &run);

    match dns::lookup(&name, &kind, server.as_deref(), timeout)
        .await
        .map_err(probe_error)
    {
        Ok(answer) => {
            let known = Known::load(app);
            let devices = answer
                .records
                .iter()
                .filter_map(|record| known.of_text(&record.data))
                .collect();
            let result = DnsResult {
                from: history::where_from(app),
                run_id: run.id.clone(),
                devices,
                answer,
            };
            // An NXDOMAIN is an answer: the summary says so rather than
            // calling the probe a failure.
            let summary = format!(
                "{}: {} record(s) in {:.0} ms from {}",
                result.answer.rcode,
                result.answer.records.len(),
                result.answer.elapsed_ms,
                result.answer.server
            );
            history::succeeded(app, &mut run, summary, &result);
            Ok(result)
        }
        Err(error) => {
            history::failed(app, &mut run, &error);
            Err(error)
        }
    }
}

async fn ownership(app: &App, params: WhoisParams) -> AppResult<WhoisResult> {
    let settings = ToolboxSettings::load(app).probe;
    let query = params.query.trim().to_string();
    if query.is_empty() {
        return Err(AppError::invalid(
            "give an address, a CIDR block, an AS number (`AS13335`) or a domain",
        ));
    }
    let timeout = Duration::from_millis(
        params
            .timeout_ms
            .unwrap_or(settings.whois_timeout_ms)
            .clamp(100, 120_000),
    );
    let mut run = Run::plain(
        "whois",
        format!("WHOIS {query}"),
        history::where_from(app),
        &query,
    );
    history::begin(app, &run);

    let answer = match params
        .server
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(server) => whois::whois_at(&query, server, timeout).await,
        None => whois::whois(&query, timeout).await,
    }
    .map_err(probe_error);
    match answer {
        Ok(answer) => {
            let result = WhoisResult {
                from: history::where_from(app),
                run_id: run.id.clone(),
                answer,
            };
            run.method = Some(result.answer.servers.join(" → "));
            let summary = result
                .answer
                .summary
                .iter()
                .find(|(field, _)| field == "netname" || field == "org" || field == "domain")
                .map(|(field, value)| format!("{field}: {value}"))
                .unwrap_or_else(|| {
                    format!(
                        "{} bytes from {}",
                        result.answer.text.len(),
                        result.answer.servers.last().cloned().unwrap_or_default()
                    )
                });
            history::succeeded(app, &mut run, summary, &result);
            Ok(result)
        }
        Err(error) => {
            history::failed(app, &mut run, &error);
            Err(error)
        }
    }
}

async fn certificate(app: &App, params: TlsParams) -> AppResult<TlsResult> {
    let settings = ToolboxSettings::load(app).probe;
    let known = Known::load(app);
    let target = known::resolve(
        app,
        params.host.as_deref(),
        params.device.as_deref(),
        &known,
    )?;
    let port = params.port.unwrap_or(443);
    if port == 0 {
        return Err(AppError::invalid("port 0 is not a port"));
    }
    let sni = match params
        .sni
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(sni) => Some(known::host_name(sni)?),
        None => None,
    };
    let timeout = Duration::from_millis(
        params
            .timeout_ms
            .unwrap_or(settings.tls_timeout_ms)
            .clamp(100, 60_000),
    );
    let mut run = Run::new(
        "tls",
        format!("TLS certificate of {}:{port}", target.host),
        history::where_from(app),
        &target,
    );
    history::begin(app, &run);

    match tls::inspect(&target.host, port, sni.as_deref(), timeout, seconds_now())
        .await
        .map_err(probe_error)
    {
        Ok(report) => {
            run.address = Some(report.address.clone());
            run.method = Some(format!("{} {}", report.protocol, report.cipher));
            let summary = format!(
                "{} expires in {} days, {}{}",
                report
                    .chain
                    .first()
                    .map(|cert| cert.common_name.clone())
                    .unwrap_or_else(|| "no certificate".into()),
                report.expires_in_days,
                if report.trusted {
                    "trusted"
                } else {
                    "NOT trusted"
                },
                if report.hostname_matches {
                    ""
                } else {
                    ", name does not match"
                },
            );
            let result = TlsResult {
                from: history::where_from(app),
                run_id: run.id.clone(),
                device: target.device.clone(),
                report,
            };
            history::succeeded(app, &mut run, summary, &result);
            Ok(result)
        }
        Err(error) => {
            history::failed(app, &mut run, &error);
            Err(error)
        }
    }
}

async fn request(app: &App, params: HttpParams) -> AppResult<HttpResult> {
    let settings = ToolboxSettings::load(app).probe;
    let url = params.url.trim().to_string();
    if url.is_empty() {
        return Err(AppError::invalid("give a URL to check"));
    }
    let options = HttpOptions {
        method: params
            .method
            .as_deref()
            .map(str::trim)
            .filter(|method| !method.is_empty())
            .unwrap_or("GET")
            .to_uppercase(),
        headers: params.headers,
        follow_redirects: params.follow_redirects.unwrap_or(5).min(10),
        timeout_ms: params
            .timeout_ms
            .unwrap_or(settings.http_timeout_ms)
            .clamp(100, 120_000),
        verify_tls: params.verify_tls.unwrap_or(true),
        ..HttpOptions::default()
    };
    // The URL is recorded; the headers are not, because one of them may be a
    // token and a history record is not the place for it.
    let mut run = Run::plain(
        "http",
        format!("{} {url}", options.method),
        history::where_from(app),
        &url,
    );
    run.method = Some(options.method.clone());
    history::begin(app, &run);

    match http::check(&url, &options, seconds_now())
        .await
        .map_err(probe_error)
    {
        Ok(report) => {
            let known = Known::load(app);
            let summary = format!(
                "{} {} in {:.0} ms, {} bytes",
                report.status, report.reason, report.total_ms, report.body_bytes
            );
            let result = HttpResult {
                from: history::where_from(app),
                run_id: run.id.clone(),
                device: report
                    .tls
                    .as_ref()
                    .and_then(|tls| known.of_text(&tls.address)),
                report,
            };
            history::succeeded(app, &mut run, summary, &result);
            Ok(result)
        }
        Err(error) => {
            history::failed(app, &mut run, &error);
            Err(error)
        }
    }
}

async fn clock(app: &App, params: NtpParams) -> AppResult<NtpResult> {
    let settings = ToolboxSettings::load(app).probe;
    let server = params.server.trim().to_string();
    if server.is_empty() {
        return Err(AppError::invalid("give an NTP server to ask"));
    }
    let timeout = Duration::from_millis(
        params
            .timeout_ms
            .unwrap_or(settings.ntp_timeout_ms)
            .clamp(100, 60_000),
    );
    let mut run = Run::plain(
        "ntp",
        format!("NTP query of {server}"),
        history::where_from(app),
        &server,
    );
    history::begin(app, &run);

    // The engine never reads the wall clock itself, so the caller's reading of
    // it is what the offset is measured against: this machine's clock, which
    // is exactly what the question is about.
    let local_unix = util::now_ms() as f64 / 1000.0;
    match ntp::query(&server, timeout, local_unix)
        .await
        .map_err(probe_error)
    {
        Ok(report) => {
            run.address = Some(report.address.clone());
            let summary = format!(
                "stratum {} ({}), offset {:+.1} ms, delay {:.1} ms",
                report.stratum, report.reference, report.offset_ms, report.delay_ms
            );
            let result = NtpResult {
                from: history::where_from(app),
                run_id: run.id.clone(),
                report,
            };
            history::succeeded(app, &mut run, summary, &result);
            Ok(result)
        }
        Err(error) => {
            history::failed(app, &mut run, &error);
            Err(error)
        }
    }
}

pub fn register(router: &mut Router) {
    router.add("probe.dns", |app: App, params: DnsParams| async move {
        lookup(&app, params).await
    });
    router.add("probe.whois", |app: App, params: WhoisParams| async move {
        ownership(&app, params).await
    });
    router.add("probe.tls", |app: App, params: TlsParams| async move {
        certificate(&app, params).await
    });
    router.add("probe.http", |app: App, params: HttpParams| async move {
        request(&app, params).await
    });
    router.add("probe.ntp", |app: App, params: NtpParams| async move {
        clock(&app, params).await
    });
}
