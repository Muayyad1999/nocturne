//! An ISP with real records, built through `App::rpc`, and a monitor that is
//! either there or deliberately not.
//!
//! Two things about this harness are worth knowing before reading a test.
//!
//! **Nothing is poked into the store.** Every customer, tariff, subscription,
//! invoice and payment below is created by calling the feature's own RPC
//! methods, so a test that passes is a statement about the wire surface and
//! not about an internal function.
//!
//! **The monitor is a stub, and that is a layering decision rather than
//! laziness.** A feature crate must not depend on another feature crate
//! (`CLAUDE.md`), so `nocturne-isp` cannot link `nocturne-monitor` even in a
//! test. What it can do is register handlers under the two names it actually
//! calls — `monitor.interfaces` and `metrics.query` — answering the shape the
//! real monitor answers (`crates/nocturne-monitor/src/model.rs`, `IfStatus`,
//! and `metrics.rs`, `Series`), including the part that matters most here:
//! a rate that could not be measured is `null`, never `0`. [`Isp::start`]
//! takes a flag for whether that stub exists at all, because "this build has
//! no monitoring" is a state the feature has to cope with.

#![allow(dead_code)] // each test binary uses a different part of this module.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use nocturne_app::testing::Lab;
use nocturne_app::{App, AppError, AppResult, inventory};
use serde::Deserialize;
use serde_json::{Value, json};

/// What the monitor has measured, as tests set it up.
#[derive(Default)]
pub struct FakeMonitor {
    /// Device id → the rows `monitor.interfaces` answers for it.
    interfaces: Mutex<BTreeMap<String, Vec<Value>>>,
    /// (device id, metric, instance) → the points `metrics.query` answers.
    samples: Mutex<BTreeMap<(String, String, String), Vec<(i64, f64)>>>,
}

impl FakeMonitor {
    /// Record what an interface is doing. `None` is a gap — the first poll
    /// after a start, a reboot, a failed poll — exactly as the real monitor
    /// reports one.
    pub fn interface(
        &self,
        device_id: &str,
        name: &str,
        in_bps: Option<f64>,
        out_bps: Option<f64>,
        speed_bps: Option<u64>,
    ) {
        let row = json!({
            "device_id": device_id,
            "device": "fake",
            "name": name,
            "description": "",
            "admin_up": true,
            "oper_up": true,
            "speed_bps": speed_bps,
            "in_bps": in_bps,
            "out_bps": out_bps,
            "util_pct": null,
            "in_err_ps": null,
            "out_err_ps": null,
            "in_disc_ps": null,
            "out_disc_ps": null,
            "uplink": false,
            "lab": true,
            "at": 1_700_000_000_000_i64,
        });
        let mut interfaces = self.interfaces.lock().expect("the stub's lock");
        let rows = interfaces.entry(device_id.to_string()).or_default();
        rows.retain(|existing| existing["name"] != json!(name));
        rows.push(row);
    }

    /// Record a series of samples, as `metrics.query` would answer them.
    pub fn samples(&self, device_id: &str, metric: &str, instance: &str, points: &[(i64, f64)]) {
        self.samples.lock().expect("the stub's lock").insert(
            (
                device_id.to_string(),
                metric.to_string(),
                instance.to_string(),
            ),
            points.to_vec(),
        );
    }
}

#[derive(Deserialize)]
struct DeviceParams {
    device_id: String,
}

#[derive(Deserialize)]
struct QueryParams {
    device_id: String,
    metric: String,
    #[serde(default)]
    instance: Option<String>,
    since: i64,
    #[serde(default)]
    until: Option<i64>,
}

/// Register the two methods this feature reaches for by name.
fn register_monitor(builder: &mut nocturne_app::AppBuilder) {
    builder.state(Arc::new(FakeMonitor::default()));
    builder.router.add(
        "monitor.interfaces",
        |app: App, params: DeviceParams| async move {
            // The real one checks the device exists first, and answers
            // `not_found` when it does not.
            inventory::get(&app, &params.device_id)?;
            let state = app.state::<Arc<FakeMonitor>>();
            let interfaces = state.interfaces.lock().expect("the stub's lock");
            Ok::<Value, AppError>(json!(
                interfaces
                    .get(&params.device_id)
                    .cloned()
                    .unwrap_or_default()
            ))
        },
    );
    builder.router.add(
        "metrics.query",
        |app: App, params: QueryParams| async move {
            inventory::get(&app, &params.device_id)?;
            let state = app.state::<Arc<FakeMonitor>>();
            let samples = state.samples.lock().expect("the stub's lock");
            let until = params.until.unwrap_or(i64::MAX);
            let points: Vec<(i64, f64)> = samples
                .get(&(
                    params.device_id.clone(),
                    params.metric.clone(),
                    params.instance.clone().unwrap_or_default(),
                ))
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|(at, _)| *at >= params.since && *at < until)
                .collect();
            Ok::<Value, AppError>(json!({
                "device_id": params.device_id,
                "metric": params.metric,
                "instance": params.instance.unwrap_or_default(),
                "points": points,
                "step_s": 300,
                "rollup": "5m",
            }))
        },
    );
}

pub struct Isp {
    pub app: App,
    lab: Lab,
    pub monitoring: bool,
}

impl Isp {
    /// An ISP whose devices are in the inventory. `monitoring` registers the
    /// monitor stub; `false` is a build with no monitoring at all.
    pub async fn start(monitoring: bool, devices: &[&str]) -> Isp {
        let mut builder = nocturne_app::testing::builder();
        nocturne_isp::init(&mut builder);
        if monitoring {
            register_monitor(&mut builder);
        }
        let lab = nocturne_app::testing::lab(builder, devices).await;
        Isp {
            app: lab.app.clone(),
            lab,
            monitoring,
        }
    }

    pub fn monitor(&self) -> Arc<FakeMonitor> {
        (*self.app.state::<Arc<FakeMonitor>>()).clone()
    }

    /// The inventory id of a device.
    pub fn device(&self, name: &str) -> String {
        self.lab.id(name)
    }

    pub async fn rpc(&self, method: &str, params: Value) -> AppResult<Value> {
        self.app.rpc(method, params).await
    }

    /// A call that must work.
    pub async fn ok(&self, method: &str, params: Value) -> Value {
        self.rpc(method, params)
            .await
            .unwrap_or_else(|err| panic!("{method}: {err}"))
    }

    /// A call that must be refused.
    pub async fn err(&self, method: &str, params: Value) -> AppError {
        self.rpc(method, params)
            .await
            .expect_err(&format!("{method} should have been refused"))
    }

    pub async fn customer(&self, name: &str, currency: &str, extra: Value) -> String {
        let mut params = json!({"name": name, "currency": currency});
        if let (Some(target), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        self.ok("isp.customer.save", params).await["customer"]["id"]
            .as_str()
            .expect("an id")
            .to_string()
    }

    pub async fn tariff(
        &self,
        name: &str,
        mbit: i64,
        unit: &str,
        minor: i64,
        currency: &str,
        period: &str,
    ) -> String {
        self.ok(
            "isp.tariff.save",
            json!({
                "name": name,
                "mbit": mbit,
                "unit": unit,
                "price": {"currency": currency, "minor": minor},
                "period": period,
            }),
        )
        .await["tariff"]["id"]
            .as_str()
            .expect("an id")
            .to_string()
    }

    /// Put a customer on a tariff. `started_at` defaults to over a year ago,
    /// so the subscription is in force for any period a test invoices — a
    /// subscription that starts now covers none of last month, which is
    /// correct and useless as a fixture.
    pub async fn subscribe(
        &self,
        customer_id: &str,
        tariff_id: &str,
        started_at: Option<i64>,
    ) -> String {
        let started = started_at.unwrap_or_else(|| nocturne_app::util::now_ms() - 400 * DAY_MS);
        let params = json!({
            "customer_id": customer_id,
            "tariff_id": tariff_id,
            "started_at": started,
        });
        self.ok("isp.subscription.create", params).await["subscription"]["id"]
            .as_str()
            .expect("an id")
            .to_string()
    }

    /// Draft and issue an invoice for a period, optionally backdated as an
    /// invoice that already exists on paper would be.
    pub async fn invoice(
        &self,
        customer_id: &str,
        period: (i64, i64),
        issued_at: Option<i64>,
        due_at: Option<i64>,
    ) -> Value {
        let draft = self
            .ok(
                "isp.invoice.draft",
                json!({
                    "customer_id": customer_id,
                    "period_start": period.0,
                    "period_end": period.1,
                }),
            )
            .await;
        let id = draft["invoice"]["id"].as_str().expect("an id").to_string();
        let mut params = json!({"id": id});
        if let Some(target) = params.as_object_mut() {
            if let Some(at) = issued_at {
                target.insert("issued_at".to_string(), json!(at));
            }
            if let Some(at) = due_at {
                target.insert("due_at".to_string(), json!(at));
            }
        }
        self.ok("isp.invoice.issue", params).await
    }

    /// The audit trail, newest first — read, never written, by tests.
    pub fn audit(&self) -> Vec<(String, String)> {
        self.app
            .store()
            .log_query(&nocturne_app::store::LogQuery {
                stream: Some("audit".to_string()),
                limit: Some(500),
                ..Default::default()
            })
            .expect("the audit trail")
            .into_iter()
            .map(|entry| (entry.source, entry.message))
            .collect()
    }
}

pub const DAY_MS: i64 = 86_400_000;

/// A 31-day window ending now, which is one monthly billing period.
pub fn last_month() -> (i64, i64) {
    let now = nocturne_app::util::now_ms();
    (now - 31 * DAY_MS, now)
}

pub fn list(value: &Value) -> Vec<Value> {
    value
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("a list, got {value:#}"))
}

/// Every money object in a result: anything with a `minor` and a `currency`.
pub fn money_objects(value: &Value) -> Vec<Value> {
    let mut found = Vec::new();
    walk(value, &mut |value| {
        if let Some(object) = value.as_object()
            && object.contains_key("minor")
            && object.contains_key("currency")
        {
            found.push(value.clone());
        }
    });
    found
}

/// Fail if any money figure anywhere in a result is a floating-point number,
/// or carries no currency. Money-ish keys are checked by name too, so a new
/// field that holds an amount cannot quietly arrive as a float.
pub fn assert_money_is_integer(value: &Value, what: &str) {
    const MONEY_KEYS: [&str; 12] = [
        "minor",
        "total",
        "price",
        "amount",
        "cost",
        "margin",
        "outstanding",
        "paid",
        "revenue",
        "unit_price",
        "period_price",
        "monthly_price",
    ];
    walk(value, &mut |value| {
        if let Some(object) = value.as_object() {
            if object.contains_key("minor") {
                let minor = &object["minor"];
                assert!(
                    minor.is_i64(),
                    "{what}: a money figure came back as {minor}, which is not whole minor units"
                );
                assert!(
                    object.get("currency").and_then(Value::as_str).is_some(),
                    "{what}: a money figure with no currency beside it: {value}"
                );
            }
            for (key, inner) in object {
                let money_ish = MONEY_KEYS.iter().any(|known| key == known);
                if money_ish && inner.is_f64() {
                    panic!("{what}: `{key}` came back as the float {inner}");
                }
            }
        }
    });
}

fn walk(value: &Value, visit: &mut impl FnMut(&Value)) {
    visit(value);
    match value {
        Value::Array(items) => {
            for item in items {
                walk(item, visit);
            }
        }
        Value::Object(object) => {
            for inner in object.values() {
                walk(inner, visit);
            }
        }
        _ => {}
    }
}
