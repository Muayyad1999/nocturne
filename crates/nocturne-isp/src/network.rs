//! The part an ISP cannot get from a spreadsheet: used against sold,
//! oversubscription and margin.
//!
//! This is the only module that reaches outside the feature, and it reaches
//! by RPC name — `monitor.interfaces` and `metrics.query` — because a feature
//! crate never links to another one. A build without the monitor answers
//! `unknown_method`, and that is a coverage state here, not an error and
//! certainly not a zero.
//!
//! **A customer whose serving interface has never been collected has no usage
//! figure.** Not zero. [`Coverage`] says which of the several different
//! nothings it is: nobody recorded a device, nobody recorded an interface,
//! the monitor is not installed, the interface has never been seen, the rate
//! could not be measured, or the window asked for has no samples in it. In a
//! billing dispute that distinction is the difference between a fact and a
//! guess, so every row and every report carries it.

use std::collections::BTreeMap;

use nocturne_app::{App, AppError, AppResult, util};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db;
use crate::model::{Customer, CustomerState, Link, Settings};
use crate::money::{Currency, Money, Totals};
use crate::{billing, records};

/// Bits per second in a megabit per second.
const MBIT: f64 = 1_000_000.0;

const HOUR_MS: i64 = 3_600_000;

/// Why there is, or is not, a figure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    /// There is a figure, and it was measured.
    Measured,
    /// Nobody has recorded which device serves this customer.
    NoServingDevice,
    /// A device is recorded but not which interface.
    NoServingInterface,
    /// The device was deleted from the inventory, or never existed.
    DeviceMissing,
    /// This build has no monitor feature, so nothing measures interfaces.
    MonitorUnavailable,
    /// The monitor is there but has never recorded that interface.
    InterfaceNeverCollected,
    /// The interface is known and the rate is not: the first poll after a
    /// start or a reboot, or a poll that failed — or only one direction was
    /// measured where the usage basis needs the other, or both. A gap, not a
    /// zero.
    NoRateYet,
    /// The interface's latest reading is older than the usage window: the
    /// monitor keeps a device's last rows for as long as the device stays
    /// unreachable, so this is what it last saw, not what is passing now.
    NoRecentReading,
    /// A window was asked for and no samples fall inside it.
    NoSamplesInWindow,
    /// The monitor was asked and refused.
    MonitorFailed,
}

impl Coverage {
    pub fn note(self) -> &'static str {
        match self {
            Coverage::Measured => "measured",
            Coverage::NoServingDevice => {
                "no serving device is recorded for this customer, so there is nothing to measure \
                 their usage on — this is not a usage of zero"
            }
            Coverage::NoServingInterface => {
                "a serving device is recorded but not which interface, so there is no usage \
                 figure — this is not a usage of zero"
            }
            Coverage::DeviceMissing => {
                "the serving device is not in the inventory any more, so nothing can be measured"
            }
            Coverage::MonitorUnavailable => {
                "this build has no monitoring, so no interface rate exists to compare the plan \
                 against"
            }
            Coverage::InterfaceNeverCollected => {
                "monitoring has never recorded that interface on that device: nobody has looked, \
                 which is not the same as nothing passing"
            }
            Coverage::NoRateYet => {
                "the interface is known but its rate could not be measured (a first poll, a \
                 reboot or a failed poll): a gap, not a zero"
            }
            Coverage::NoRecentReading => {
                "monitoring has not measured that interface within the usage window: its latest \
                 reading is what it last saw, not what is passing now — and not a usage of zero"
            }
            Coverage::NoSamplesInWindow => {
                "nothing was recorded for that interface inside the window asked for"
            }
            Coverage::MonitorFailed => "monitoring was asked for the rate and refused",
        }
    }
}

/// The window a figure covers, and which resolution it came from.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Window {
    pub since: i64,
    pub until: i64,
    /// `raw`, `5m` or `1h`, as the monitor answered.
    pub rollup: String,
}

/// What is actually passing, in megabits per second.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UsageFigure {
    pub in_mbit: Option<f64>,
    pub out_mbit: Option<f64>,
    /// The figure a plan is compared against, per the `usage_basis` setting.
    pub used_mbit: f64,
    /// Which direction that is, in words.
    pub basis: String,
    /// The moment the figure is from, for a latest reading.
    pub at: Option<i64>,
    /// The window it averages, for a windowed reading.
    pub window: Option<Window>,
    /// The highest point in the window.
    pub peak_mbit: Option<f64>,
    /// How many samples the average is of.
    pub samples: Option<u32>,
    /// What the interface itself says it can carry.
    pub speed_mbit: Option<f64>,
}

/// One row of the monitor's interface table. Mirrors `IfStatus` in
/// `crates/nocturne-monitor/src/model.rs`, read by name rather than by type
/// because this crate does not link to that one. Every field is optional:
/// the monitor adding one must not break this read.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct InterfaceRow {
    name: String,
    description: String,
    oper_up: Option<bool>,
    speed_bps: Option<f64>,
    /// `null` when the interval could not be measured — a gap, never a zero.
    in_bps: Option<f64>,
    out_bps: Option<f64>,
    /// When the poll that produced the row ran. A row without one cannot be
    /// taken for what is passing now.
    at: Option<i64>,
}

/// What asking the monitor about a device produced.
enum Interfaces {
    Rows(Vec<InterfaceRow>),
    /// No monitor in this build.
    Absent,
    DeviceMissing,
    Failed(String),
}

async fn interfaces_of(app: &App, device_id: &str) -> Interfaces {
    match app
        .rpc(
            "monitor.interfaces",
            json!({"device_id": device_id, "limit": 5_000}),
        )
        .await
    {
        Ok(value) => match serde_json::from_value::<Vec<InterfaceRow>>(value) {
            Ok(rows) => Interfaces::Rows(rows),
            Err(err) => Interfaces::Failed(format!(
                "the monitor's interface table could not be read: {err}"
            )),
        },
        Err(err) if err.code == "unknown_method" => Interfaces::Absent,
        Err(err) if err.code == "not_found" => Interfaces::DeviceMissing,
        Err(err) => Interfaces::Failed(err.message),
    }
}

fn find_interface<'rows>(
    rows: &'rows [InterfaceRow],
    interface: &str,
) -> Option<&'rows InterfaceRow> {
    let wanted = interface.trim();
    rows.iter()
        .find(|row| row.name.trim().eq_ignore_ascii_case(wanted))
}

/// A figure, or which of the nothings it is.
struct Reading {
    used: Option<UsageFigure>,
    coverage: Coverage,
    /// Said about this reading in particular, where the general sentence for
    /// its coverage would leave out what matters: which direction is
    /// missing, how old the reading is.
    note: Option<String>,
}

impl Reading {
    fn none(coverage: Coverage) -> Reading {
        Reading {
            used: None,
            coverage,
            note: None,
        }
    }

    fn gap(coverage: Coverage, note: String) -> Reading {
        Reading {
            used: None,
            coverage,
            note: Some(note),
        }
    }

    fn measured(figure: UsageFigure) -> Reading {
        Reading {
            used: Some(figure),
            coverage: Coverage::Measured,
            note: None,
        }
    }

    fn note(&self) -> String {
        self.note
            .clone()
            .unwrap_or_else(|| self.coverage.note().to_string())
    }
}

/// Which direction counts as the customer's traffic. There is no universally
/// right answer — it depends which side of the link the interface is — so it
/// is a setting, and the figure says which it used.
///
/// `Err` when what the basis needs was not measured, with the sentence that
/// says so — or `None` in it when neither direction was, which the general
/// sentence already says. `when` is how the sentence places the gap:
/// nothing for a latest reading, " inside the window" for an average.
fn pick(
    settings: &Settings,
    inbound: Option<f64>,
    outbound: Option<f64>,
    when: &str,
) -> Result<(f64, String), Option<String>> {
    let one_of = |needed: &str, other: Option<f64>, other_name: &str| {
        other.map(|_| {
            format!(
                "the {needed} rate, which is what the usage basis takes, could not be \
                 measured{when} (the {other_name} rate was): a gap, not a zero"
            )
        })
    };
    match settings.usage_basis.trim().to_ascii_lowercase().as_str() {
        "in" => inbound
            .map(|value| (value, "the interface's inbound rate".to_string()))
            .ok_or_else(|| one_of("inbound", outbound, "outbound")),
        "out" => outbound
            .map(|value| (value, "the interface's outbound rate".to_string()))
            .ok_or_else(|| one_of("outbound", inbound, "inbound")),
        // The greater of two rates needs both. With one of them missing the
        // other may be the smaller, and calling it the greater would pass a
        // lower bound off as the most the customer is pulling.
        _ => match (inbound, outbound) {
            (Some(inbound), Some(outbound)) => Ok((
                inbound.max(outbound),
                "the greater of the interface's inbound and outbound rates".to_string(),
            )),
            (None, None) => Err(None),
            (inbound, _) => {
                let (measured, missing) = if inbound.is_some() {
                    ("inbound", "outbound")
                } else {
                    ("outbound", "inbound")
                };
                Err(Some(format!(
                    "only the {measured} rate was measured{when} and the {missing} one could \
                     not be, so which of the two is greater is not known — the {measured} rate \
                     may be the smaller: a gap, not a zero"
                )))
            }
        },
    }
}

/// How old a reading is, in the unit a person would say it in.
fn age_text(age_ms: i64) -> String {
    let hours = age_ms / HOUR_MS;
    if hours >= 48 {
        format!("{} days", hours / 24)
    } else if hours >= 2 {
        format!("{hours} hours")
    } else {
        match (age_ms / 60_000).max(1) {
            1 => "1 minute".to_string(),
            minutes => format!("{minutes} minutes"),
        }
    }
}

/// The latest rate on one interface, or why there is none.
///
/// The monitor keeps a device's rows from its last poll that read them, for
/// as long as the device stays unreachable, so the latest row may be days
/// old. With no window asked for, a figure looks back `usage_window_hours`
/// and no further: an older reading is what the customer was using then.
fn latest_figure(settings: &Settings, rows: &[InterfaceRow], interface: &str, now: i64) -> Reading {
    let Some(row) = find_interface(rows, interface) else {
        return Reading::none(Coverage::InterfaceNeverCollected);
    };
    let window_hours = settings.usage_window_hours.max(1);
    let within = i64::from(window_hours) * HOUR_MS;
    let at = match row.at {
        Some(at) if now.saturating_sub(at) <= within => at,
        Some(at) => {
            return Reading::gap(
                Coverage::NoRecentReading,
                format!(
                    "the latest reading of that interface is {} old, older than the usage \
                     window of {}: monitoring has not measured it since, so it is not what is \
                     passing now — and not a usage of zero",
                    age_text(now.saturating_sub(at)),
                    if window_hours == 1 {
                        "1 hour".to_string()
                    } else {
                        format!("{window_hours} hours")
                    }
                ),
            );
        }
        None => {
            return Reading::gap(
                Coverage::NoRecentReading,
                "monitoring gave no time for its latest reading of that interface, so it cannot \
                 be taken for what is passing now"
                    .to_string(),
            );
        }
    };
    let inbound = row.in_bps.map(|bps| bps / MBIT);
    let outbound = row.out_bps.map(|bps| bps / MBIT);
    match pick(settings, inbound, outbound, "") {
        Err(None) => Reading::none(Coverage::NoRateYet),
        Err(Some(why)) => Reading::gap(Coverage::NoRateYet, why),
        Ok((used_mbit, basis)) => Reading::measured(UsageFigure {
            in_mbit: inbound,
            out_mbit: outbound,
            used_mbit,
            basis,
            at: Some(at),
            window: None,
            peak_mbit: None,
            samples: None,
            speed_mbit: row.speed_bps.map(|bps| bps / MBIT),
        }),
    }
}

/// One series as `metrics.query` answers it.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Series {
    points: Vec<(i64, f64)>,
    rollup: String,
}

enum SeriesAnswer {
    Series(Series),
    Absent,
    Failed(String),
}

async fn series(
    app: &App,
    device_id: &str,
    metric: &str,
    instance: &str,
    since: i64,
    until: i64,
) -> SeriesAnswer {
    match app
        .rpc(
            "metrics.query",
            json!({
                "device_id": device_id,
                "metric": metric,
                "instance": instance,
                "since": since,
                "until": until,
            }),
        )
        .await
    {
        Ok(value) => match serde_json::from_value::<Series>(value) {
            Ok(series) => SeriesAnswer::Series(series),
            Err(err) => SeriesAnswer::Failed(format!("the monitor's series was unreadable: {err}")),
        },
        Err(err) if err.code == "unknown_method" => SeriesAnswer::Absent,
        Err(err) => SeriesAnswer::Failed(err.message),
    }
}

fn mean(points: &[(i64, f64)]) -> Option<f64> {
    if points.is_empty() {
        return None;
    }
    let sum: f64 = points.iter().map(|(_, value)| value).sum();
    Some(sum / points.len() as f64)
}

/// The average and peak over a window, or why there is none.
async fn window_figure(
    app: &App,
    settings: &Settings,
    device_id: &str,
    interface: &str,
    since: i64,
    until: i64,
    speed_mbit: Option<f64>,
) -> Reading {
    let inbound = series(app, device_id, "if.in_bps", interface, since, until).await;
    if let SeriesAnswer::Absent = inbound {
        return Reading::none(Coverage::MonitorUnavailable);
    }
    let outbound = series(app, device_id, "if.out_bps", interface, since, until).await;
    let (inbound, outbound) = match (inbound, outbound) {
        (SeriesAnswer::Series(inbound), SeriesAnswer::Series(outbound)) => (inbound, outbound),
        (SeriesAnswer::Failed(why), _) | (_, SeriesAnswer::Failed(why)) => {
            tracing::debug!(
                device_id,
                interface,
                why,
                "isp: no usage figure for a customer"
            );
            return Reading::none(Coverage::MonitorFailed);
        }
        _ => return Reading::none(Coverage::MonitorUnavailable),
    };
    if inbound.points.is_empty() && outbound.points.is_empty() {
        return Reading::none(Coverage::NoSamplesInWindow);
    }
    let in_mbit = mean(&inbound.points).map(|bps| bps / MBIT);
    let out_mbit = mean(&outbound.points).map(|bps| bps / MBIT);
    let peak = inbound
        .points
        .iter()
        .chain(outbound.points.iter())
        .map(|(_, value)| value / MBIT)
        .fold(f64::NAN, f64::max);
    // Samples in one direction and not the one the basis needs is a gap in
    // the rate, not a window with nothing in it.
    match pick(settings, in_mbit, out_mbit, " inside the window") {
        Err(None) => Reading::none(Coverage::NoSamplesInWindow),
        Err(Some(why)) => Reading::gap(Coverage::NoRateYet, why),
        Ok((used_mbit, basis)) => Reading::measured(UsageFigure {
            in_mbit,
            out_mbit,
            used_mbit,
            basis: format!("{basis}, averaged over the window"),
            at: None,
            window: Some(Window {
                since,
                until,
                rollup: if inbound.rollup.is_empty() {
                    outbound.rollup
                } else {
                    inbound.rollup
                },
            }),
            peak_mbit: peak.is_finite().then_some(peak),
            samples: u32::try_from(inbound.points.len().max(outbound.points.len())).ok(),
            speed_mbit,
        }),
    }
}

// ------------------------------------------------------------ used vs. sold

/// What one customer was sold, and what they are using.
#[derive(Clone, Debug, Serialize)]
pub struct UsedAgainstSold {
    pub customer_id: String,
    pub customer_name: String,
    pub state: CustomerState,
    /// The sum of the customer's active subscriptions. `None` means they have
    /// none — which is not the same as a plan of zero megabits.
    pub sold_mbit: Option<i64>,
    pub subscriptions: u32,
    pub device_id: Option<String>,
    pub interface: Option<String>,
    /// `None` whenever [`Self::coverage`] is not `measured`.
    pub used: Option<UsageFigure>,
    pub coverage: Coverage,
    pub coverage_note: String,
    /// What they are using as a percentage of what they were sold. Only when
    /// both numbers exist.
    pub of_plan_pct: Option<f64>,
}

/// Which customers a report is of: how many match the caller's filter in
/// all, and where the next page of them begins.
pub struct PageOf {
    pub matching: u64,
    pub next_cursor: Option<String>,
}

impl PageOf {
    /// A report of one customer, asked for by id.
    pub fn one() -> PageOf {
        PageOf {
            matching: 1,
            next_cursor: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct UsageReport {
    pub rows: Vec<UsedAgainstSold>,
    /// How many customers this report is of: the page that was asked for.
    pub customers: u32,
    /// How many match the filter in all. More than `customers` when this is
    /// one page of them — and then every count here is of this page only.
    pub matching: u64,
    /// Pass this back as `cursor` for the next page; `None` on the last one.
    pub next_cursor: Option<String>,
    /// How many of them have a usage figure at all.
    pub measured: u32,
    pub without_usage: u32,
    /// The reasons the rest have none, counted — a report's coverage, in one
    /// place, so nobody reads the rows as "these customers use nothing".
    pub coverage: Vec<CoverageCount>,
    pub window: Option<Window>,
    pub note: String,
}

#[derive(Debug, Serialize)]
pub struct CoverageCount {
    pub coverage: Coverage,
    pub customers: u32,
    pub note: String,
}

/// Used against sold for a page of customers.
pub async fn usage(
    app: &App,
    settings: &Settings,
    customers: &[Customer],
    page: PageOf,
    window: Option<(i64, i64)>,
) -> AppResult<UsageReport> {
    let store = app.store();
    let now = util::now_ms();
    let mut rows: Vec<UsedAgainstSold> = Vec::new();
    // One read of the monitor's table per device, however many customers
    // hang off it.
    let mut tables: BTreeMap<String, Option<Vec<InterfaceRow>>> = BTreeMap::new();
    let mut device_states: BTreeMap<String, Coverage> = BTreeMap::new();
    for customer in customers {
        let subscriptions = db::subscriptions(
            store,
            &db::SubscriptionQuery {
                customer_id: Some(customer.id.clone()),
                state: Some("active".to_string()),
                ..db::SubscriptionQuery::default()
            },
        )?;
        let sold_mbit = if subscriptions.is_empty() {
            None
        } else {
            Some(subscriptions.iter().map(|plan| plan.mbit).sum())
        };
        let device_id = customer.service.device_id.clone();
        let interface = customer.service.interface.clone();
        let reading = match (&device_id, &interface) {
            (None, _) => Reading::none(Coverage::NoServingDevice),
            (Some(_), None) => Reading::none(Coverage::NoServingInterface),
            (Some(device_id), Some(interface)) => {
                if !tables.contains_key(device_id) {
                    match interfaces_of(app, device_id).await {
                        Interfaces::Rows(rows) => {
                            tables.insert(device_id.clone(), Some(rows));
                        }
                        Interfaces::Absent => {
                            tables.insert(device_id.clone(), None);
                            device_states.insert(device_id.clone(), Coverage::MonitorUnavailable);
                        }
                        Interfaces::DeviceMissing => {
                            tables.insert(device_id.clone(), None);
                            device_states.insert(device_id.clone(), Coverage::DeviceMissing);
                        }
                        Interfaces::Failed(why) => {
                            tracing::debug!(device_id, why, "isp: the monitor would not answer");
                            tables.insert(device_id.clone(), None);
                            device_states.insert(device_id.clone(), Coverage::MonitorFailed);
                        }
                    }
                }
                match tables.get(device_id).and_then(|rows| rows.as_ref()) {
                    None => Reading::none(
                        device_states
                            .get(device_id)
                            .copied()
                            .unwrap_or(Coverage::MonitorFailed),
                    ),
                    Some(table) => match window {
                        None => latest_figure(settings, table, interface, now),
                        Some((since, until)) => {
                            let speed = find_interface(table, interface)
                                .and_then(|row| row.speed_bps)
                                .map(|bps| bps / MBIT);
                            if find_interface(table, interface).is_none() {
                                Reading::none(Coverage::InterfaceNeverCollected)
                            } else {
                                window_figure(
                                    app, settings, device_id, interface, since, until, speed,
                                )
                                .await
                            }
                        }
                    },
                }
            }
        };
        let of_plan_pct = match (&reading.used, sold_mbit) {
            (Some(figure), Some(sold)) if sold > 0 => Some(figure.used_mbit / sold as f64 * 100.0),
            _ => None,
        };
        let coverage_note = reading.note();
        rows.push(UsedAgainstSold {
            customer_id: customer.id.clone(),
            customer_name: customer.name.clone(),
            state: customer.state,
            sold_mbit,
            subscriptions: u32::try_from(subscriptions.len()).unwrap_or(u32::MAX),
            device_id,
            interface,
            used: reading.used,
            coverage: reading.coverage,
            coverage_note,
            of_plan_pct,
        });
    }
    let measured = rows
        .iter()
        .filter(|row| row.coverage == Coverage::Measured)
        .count();
    let mut counts: BTreeMap<&'static str, (Coverage, u32)> = BTreeMap::new();
    for row in &rows {
        let entry = counts
            .entry(row.coverage.note())
            .or_insert((row.coverage, 0));
        entry.1 += 1;
    }
    let window = rows
        .iter()
        .filter_map(|row| row.used.as_ref().and_then(|used| used.window.clone()))
        .next();
    Ok(UsageReport {
        customers: u32::try_from(rows.len()).unwrap_or(u32::MAX),
        matching: page.matching,
        next_cursor: page.next_cursor,
        measured: u32::try_from(measured).unwrap_or(u32::MAX),
        without_usage: u32::try_from(rows.len() - measured).unwrap_or(u32::MAX),
        coverage: counts
            .into_values()
            .map(|(coverage, customers)| CoverageCount {
                coverage,
                customers,
                note: coverage.note().to_string(),
            })
            .collect(),
        window,
        note: "a customer with no usage figure is not a customer using nothing: `coverage` says \
               which it is for each of them"
            .to_string(),
        rows,
    })
}

// ------------------------------------------------------- oversubscription

#[derive(Debug, Serialize)]
pub struct LinkCapacity {
    pub link_id: String,
    pub link_name: String,
    pub tower: Option<String>,
    pub device_id: Option<String>,
    pub interface: Option<String>,
    /// What the link can carry, as recorded.
    pub capacity_mbit: i64,
    /// `declared` (somebody wrote it down) or `interface_speed` (what the
    /// interface itself reports).
    pub capacity_basis: &'static str,
    /// The sum of the active plans of the active customers behind it.
    pub sold_mbit: i64,
    /// Sold to customers whose account is suspended — still sold, not
    /// cancelled, and counted separately rather than quietly included.
    pub suspended_mbit: i64,
    pub customers: u32,
    pub suspended_customers: u32,
    /// Sold divided by capacity. `None` when no capacity is recorded.
    pub ratio: Option<f64>,
    pub ratio_text: String,
    /// What the link is actually carrying, when it can be measured.
    pub measured: Option<UsageFigure>,
    pub coverage: Coverage,
    pub coverage_note: String,
    pub note: String,
}

pub async fn capacity(
    app: &App,
    settings: &Settings,
    link_id: Option<&str>,
    tower: Option<&str>,
    window: Option<(i64, i64)>,
) -> AppResult<Vec<LinkCapacity>> {
    let links = match link_id {
        Some(id) => vec![app.store().require::<Link>(db::LINKS, id, "link")?],
        None => records::links(app)?
            .into_iter()
            .filter(|link| match tower {
                Some(tower) => link.tower.as_deref() == Some(tower),
                None => true,
            })
            .collect(),
    };
    let now = util::now_ms();
    let mut report = Vec::new();
    for link in links {
        let served = db::served(app.store(), link.tower.as_deref())?;
        let (mut sold, mut suspended) = (0_i64, 0_i64);
        let (mut customers, mut suspended_customers) = (0_u32, 0_u32);
        let mut seen: Vec<&str> = Vec::new();
        for row in &served {
            match row.customer_state {
                CustomerState::Cancelled => continue,
                CustomerState::Suspended => {
                    suspended += row.subscription.mbit;
                    if !seen.contains(&row.customer_id.as_str()) {
                        suspended_customers += 1;
                    }
                }
                CustomerState::Active => {
                    sold += row.subscription.mbit;
                    if !seen.contains(&row.customer_id.as_str()) {
                        customers += 1;
                    }
                }
            }
            seen.push(&row.customer_id);
        }
        // A link with no capacity written down and an interface the monitor
        // knows can use the interface's own speed — and says so.
        let mut capacity_mbit = link.capacity_mbit;
        let mut capacity_basis = "declared";
        let reading = match (&link.device_id, &link.interface) {
            (None, _) => Reading::none(Coverage::NoServingDevice),
            (Some(_), None) => Reading::none(Coverage::NoServingInterface),
            (Some(device_id), Some(interface)) => match interfaces_of(app, device_id).await {
                Interfaces::Absent => Reading::none(Coverage::MonitorUnavailable),
                Interfaces::DeviceMissing => Reading::none(Coverage::DeviceMissing),
                Interfaces::Failed(why) => {
                    tracing::debug!(device_id, why, "isp: the monitor would not answer");
                    Reading::none(Coverage::MonitorFailed)
                }
                Interfaces::Rows(rows) => {
                    if capacity_mbit <= 0
                        && let Some(speed) =
                            find_interface(&rows, interface).and_then(|row| row.speed_bps)
                    {
                        capacity_mbit = (speed / MBIT) as i64;
                        capacity_basis = "interface_speed";
                    }
                    let speed = find_interface(&rows, interface)
                        .and_then(|row| row.speed_bps)
                        .map(|bps| bps / MBIT);
                    match window {
                        None => latest_figure(settings, &rows, interface, now),
                        Some((since, until)) => {
                            if find_interface(&rows, interface).is_none() {
                                Reading::none(Coverage::InterfaceNeverCollected)
                            } else {
                                window_figure(
                                    app, settings, device_id, interface, since, until, speed,
                                )
                                .await
                            }
                        }
                    }
                }
            },
        };
        let ratio = (capacity_mbit > 0).then(|| sold as f64 / capacity_mbit as f64);
        report.push(LinkCapacity {
            link_id: link.id.clone(),
            link_name: link.name.clone(),
            tower: link.tower.clone(),
            device_id: link.device_id.clone(),
            interface: link.interface.clone(),
            capacity_mbit,
            capacity_basis,
            sold_mbit: sold,
            suspended_mbit: suspended,
            customers,
            suspended_customers,
            ratio,
            ratio_text: match ratio {
                Some(ratio) => format!("{ratio:.1}:1"),
                None => format!(
                    "{sold} Mbit sold and no capacity recorded for `{}`, so there is no ratio",
                    link.name
                ),
            },
            coverage_note: reading.note(),
            measured: reading.used,
            coverage: reading.coverage,
            note: "both figures, not a verdict: 3:1 on a link that is never busy reads \
                   differently from 3:1 on one that saturates nightly, which is what `measured` \
                   is for"
                .to_string(),
        });
    }
    Ok(report)
}

// -------------------------------------------------------------------- margin

#[derive(Debug, Serialize)]
pub struct Margin {
    /// `link`, `tower` or `customer`.
    pub of: &'static str,
    pub id: String,
    pub name: String,
    /// Revenue for one month, per currency — never summed across them.
    pub revenue: Totals,
    /// What the bandwidth costs for one month, when a wholesale cost per
    /// megabit is recorded.
    pub cost: Option<Money>,
    /// Revenue minus cost, in the cost's currency only.
    pub margin: Option<Money>,
    /// Revenue in currencies the cost is not in: left exactly as it is.
    pub unconverted: Totals,
    /// How many megabits the cost was worked out on.
    pub cost_mbit: Option<i64>,
    /// False when a figure here came out of a division that rounded.
    pub exact: bool,
    pub note: String,
}

fn monthly_revenue(served: &[db::Served]) -> AppResult<(Totals, bool)> {
    let mut revenue = Totals::new();
    let mut exact = true;
    for row in served {
        if row.customer_state == CustomerState::Cancelled {
            continue;
        }
        let monthly = row.subscription.monthly_price()?;
        exact = exact && monthly.exact;
        revenue.add(monthly.amount)?;
    }
    Ok((revenue, exact))
}

fn margin_of(
    of: &'static str,
    id: String,
    name: String,
    revenue: Totals,
    revenue_exact: bool,
    cost_per_mbit: Option<Money>,
    cost_period: crate::model::Period,
    mbit: i64,
) -> AppResult<Margin> {
    let mut exact = revenue_exact;
    let cost = match cost_per_mbit {
        None => None,
        Some(per_mbit) => {
            let total = per_mbit.times(mbit)?;
            let monthly = total.scale_by(1, cost_period.months())?;
            exact = exact && monthly.exact;
            Some(monthly.amount)
        }
    };
    let margin = match cost {
        Some(cost) => Some(revenue.in_currency(cost.currency).minus(cost)?),
        None => None,
    };
    let unconverted = match cost {
        Some(cost) => {
            let mut rest = Totals::new();
            for amount in revenue.parts() {
                if amount.currency != cost.currency {
                    rest.add(amount)?;
                }
            }
            rest
        }
        None => revenue.clone(),
    };
    let note = match (&cost, unconverted.is_empty()) {
        (None, _) => "no wholesale cost per megabit is recorded, so there is no margin — only \
                      what is being charged"
            .to_string(),
        (Some(cost), true) => format!(
            "revenue and cost are both in {}, for one month",
            cost.currency
        ),
        (Some(cost), false) => format!(
            "the margin is in {} only: the revenue in {} is left exactly as it is, because \
             nothing here converts a currency without a rate somebody entered",
            cost.currency,
            unconverted
                .currencies()
                .iter()
                .map(|currency| currency.code())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    Ok(Margin {
        of,
        id,
        name,
        revenue,
        cost,
        margin,
        unconverted,
        cost_mbit: Some(mbit),
        exact,
        note,
    })
}

/// The margin of a link or a tower (every customer behind it), or of one
/// customer (their plans against the cost of the megabits they were sold).
pub fn margin(
    app: &App,
    link_id: Option<&str>,
    tower: Option<&str>,
    customer_id: Option<&str>,
) -> AppResult<Vec<Margin>> {
    let store = app.store();
    if let Some(customer_id) = customer_id {
        let customer = db::require_customer(store, customer_id)?;
        let served: Vec<db::Served> = db::served(store, customer.service.tower.as_deref())?
            .into_iter()
            .filter(|row| row.customer_id == customer.id)
            .collect();
        let (revenue, exact) = monthly_revenue(&served)?;
        let mbit: i64 = served.iter().map(|row| row.subscription.mbit).sum();
        // The cost comes from the link that feeds their tower, when there is
        // one with a cost on it.
        let link = records::links(app)?.into_iter().find(|link| {
            link.tower.is_some()
                && link.tower == customer.service.tower
                && link.cost_per_mbit.is_some()
        });
        let (cost_per_mbit, cost_period) = match &link {
            Some(link) => (link.cost_per_mbit, link.cost_period),
            None => (None, crate::model::Period::Monthly),
        };
        let mut margin = margin_of(
            "customer",
            customer.id.clone(),
            customer.name.clone(),
            revenue,
            exact,
            cost_per_mbit,
            cost_period,
            mbit,
        )?;
        if link.is_none() {
            margin.note = format!(
                "no link with a wholesale cost serves {}, so there is no cost to take off their \
                 revenue",
                customer
                    .service
                    .tower
                    .clone()
                    .unwrap_or_else(|| "this customer".to_string())
            );
        }
        return Ok(vec![margin]);
    }
    let links: Vec<Link> = match link_id {
        Some(id) => vec![store.require::<Link>(db::LINKS, id, "link")?],
        None => records::links(app)?
            .into_iter()
            .filter(|link| match tower {
                Some(tower) => link.tower.as_deref() == Some(tower),
                None => true,
            })
            .collect(),
    };
    if links.is_empty() {
        return Err(AppError::not_found(match tower {
            Some(tower) => format!("a link for the tower `{tower}`"),
            None => "any link; record one with `isp.link.save` so there is a capacity and a \
                     wholesale cost to work a margin out from"
                .to_string(),
        }));
    }
    let mut margins = Vec::new();
    for link in links {
        let served = db::served(store, link.tower.as_deref())?;
        let (revenue, exact) = monthly_revenue(&served)?;
        margins.push(margin_of(
            "link",
            link.id.clone(),
            link.name.clone(),
            revenue,
            exact,
            link.cost_per_mbit,
            link.cost_period,
            link.capacity_mbit,
        )?);
    }
    Ok(margins)
}

/// A customer's row for the detail screen: what they were sold, what they
/// owe, and what is known about their usage.
#[derive(Debug, Serialize)]
pub struct CustomerDetail {
    pub customer: Customer,
    pub subscriptions: Vec<crate::model::Subscription>,
    pub invoices: Vec<crate::model::InvoiceView>,
    pub balance: billing::Balance,
    pub usage: UsedAgainstSold,
    pub currency: Currency,
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn settings(basis: &str) -> Settings {
        Settings {
            usage_basis: basis.to_string(),
            ..Settings::default()
        }
    }

    /// A row as the monitor last polled it, just now.
    fn row(name: &str, in_bps: Option<f64>, out_bps: Option<f64>) -> InterfaceRow {
        InterfaceRow {
            name: name.to_string(),
            in_bps,
            out_bps,
            at: Some(NOW),
            ..InterfaceRow::default()
        }
    }

    #[test]
    fn a_null_rate_is_a_gap_and_not_a_zero() {
        let rows = vec![row("ether3", None, None)];
        let reading = latest_figure(&settings("greater"), &rows, "ether3", NOW);
        assert!(
            reading.used.is_none(),
            "no figure at all, rather than 0 Mbit"
        );
        assert_eq!(reading.coverage, Coverage::NoRateYet);
        // An interface nobody has collected is a different nothing again.
        let missing = latest_figure(&settings("greater"), &rows, "ether9", NOW);
        assert_eq!(missing.coverage, Coverage::InterfaceNeverCollected);
    }

    #[test]
    fn the_usage_basis_decides_which_direction_is_the_customers_traffic() {
        let rows = vec![row("ether3", Some(2_000_000.0), Some(8_000_000.0))];
        let greater = latest_figure(&settings("greater"), &rows, "ether3", NOW)
            .used
            .expect("a figure");
        assert_eq!(greater.used_mbit, 8.0);
        assert!(greater.basis.contains("greater"), "{}", greater.basis);
        assert_eq!(
            greater.at,
            Some(NOW),
            "a latest reading says when it is from"
        );
        let inbound = latest_figure(&settings("in"), &rows, "ether3", NOW).used;
        assert_eq!(inbound.expect("a figure").used_mbit, 2.0);
        let outbound = latest_figure(&settings("out"), &rows, "ether3", NOW).used;
        assert_eq!(outbound.expect("a figure").used_mbit, 8.0);
        // Asked for one direction that has no rate: still a gap, and the
        // sentence says which direction it was.
        let half = vec![row("ether3", None, Some(8_000_000.0))];
        let reading = latest_figure(&settings("in"), &half, "ether3", NOW);
        assert!(reading.used.is_none() && reading.coverage == Coverage::NoRateYet);
        assert!(reading.note().contains("inbound"), "{}", reading.note());
        // The greater of the two is not known either: the one that was
        // measured may be the smaller.
        let reading = latest_figure(&settings("greater"), &half, "ether3", NOW);
        assert!(reading.used.is_none() && reading.coverage == Coverage::NoRateYet);
        assert!(
            reading.note().contains("may be the smaller"),
            "{}",
            reading.note()
        );
    }

    #[test]
    fn a_reading_from_before_the_usage_window_is_not_a_current_figure() {
        let settings = Settings::default();
        let hour = 3_600_000;
        let mut rows = vec![row("ether3", Some(2_000_000.0), Some(8_000_000.0))];
        rows[0].at = Some(NOW - 24 * hour);
        let edge = latest_figure(&settings, &rows, "ether3", NOW);
        assert_eq!(
            edge.coverage,
            Coverage::Measured,
            "the window's own edge is in it"
        );
        rows[0].at = Some(NOW - 24 * hour - 1);
        let old = latest_figure(&settings, &rows, "ether3", NOW);
        assert_eq!(old.coverage, Coverage::NoRecentReading);
        assert!(old.used.is_none());
        assert!(old.note().contains("24 hours"), "{}", old.note());
        // A row the monitor gave no time for cannot be taken for now either.
        rows[0].at = None;
        let timeless = latest_figure(&settings, &rows, "ether3", NOW);
        assert_eq!(timeless.coverage, Coverage::NoRecentReading);
        // A window of nothing is not a setting anybody meant: an hour.
        let zero = Settings {
            usage_window_hours: 0,
            ..Settings::default()
        };
        rows[0].at = Some(NOW - 30 * 60_000);
        assert_eq!(
            latest_figure(&zero, &rows, "ether3", NOW).coverage,
            Coverage::Measured
        );
    }

    #[test]
    fn an_age_is_said_in_the_unit_a_person_would_use() {
        let minute = 60_000;
        assert_eq!(age_text(10), "1 minute");
        assert_eq!(age_text(90 * minute), "90 minutes");
        assert_eq!(age_text(5 * 60 * minute), "5 hours");
        assert_eq!(age_text(6 * 24 * 60 * minute), "6 days");
    }

    #[test]
    fn an_interface_row_survives_the_monitor_growing_a_field() {
        let row: InterfaceRow = serde_json::from_value(json!({
            "device_id": "d1", "device": "RB1", "name": "ether3", "description": "",
            "admin_up": true, "oper_up": true, "speed_bps": 100_000_000u64,
            "in_bps": 1_500_000.0, "out_bps": null, "util_pct": 1.5,
            "something_new": 42, "at": 17
        }))
        .expect("unknown fields are ignored");
        assert_eq!((row.name.as_str(), row.at), ("ether3", Some(17)));
        assert_eq!(row.in_bps, Some(1_500_000.0));
        assert_eq!(row.out_bps, None);
    }
}
