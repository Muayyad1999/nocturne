//! Fakes that let the tests drive time, packets and agents.
//!
//! The point of the probe and SNMP traits is this module: every test in the
//! crate runs on a paused tokio timer against devices whose answers it
//! decides, so damping, counter arithmetic, scheduling and the alert
//! lifecycle are all exercised without a packet leaving the process.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nocturne_app::App;
use nocturne_snmp::{Auth, Oid, Value, VarBind, oids};
use parking_lot::Mutex;
use serde_json::{Value as Json, json};

use crate::clock::{Clock, VirtualClock};
use crate::probe::{Echoes, Reachability};
use crate::snmp::{SnmpSession, SnmpTransport, TableRow};
use crate::state::MonitorState;

/// Where a test's virtual clock starts.
///
/// It has to be the real present, not a fixed date: the store decides which
/// resolution to answer a metric query from by comparing the range against
/// the *system* clock, so samples written far in the past would come back
/// from roll-ups that do not exist yet.
pub fn epoch() -> i64 {
    nocturne_app::util::now_ms()
}

/// A clock a test moves by hand.
///
/// For the tests that need both a real socket and the passing of an hour: a
/// paused tokio timer cannot be mixed with real I/O (it fast-forwards over
/// the request), so those run on real time and step this instead.
pub struct ManualClock(std::sync::atomic::AtomicI64);

impl ManualClock {
    pub fn new(now: i64) -> ManualClock {
        ManualClock(std::sync::atomic::AtomicI64::new(now))
    }

    pub fn advance(&self, by: Duration) {
        self.0
            .fetch_add(by.as_millis() as i64, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The same counter: it is only ever stepped forwards, and nothing
    /// compares the two readings with each other.
    fn monotonic_ms(&self) -> i64 {
        self.now_ms()
    }
}

/// A clock whose reading of the time of day a test can step, the way NTP or
/// a person at the control panel does, while time itself carries on at its
/// usual rate underneath.
pub struct SteppedClock {
    inner: VirtualClock,
    offset: std::sync::atomic::AtomicI64,
}

impl SteppedClock {
    /// A clock reading `base` right now. Must be created inside a runtime.
    pub fn new(base: i64) -> SteppedClock {
        SteppedClock {
            inner: VirtualClock::new(base),
            offset: std::sync::atomic::AtomicI64::new(0),
        }
    }

    /// Move the time of day by `by_ms`, backwards when negative.
    pub fn step(&self, by_ms: i64) {
        self.offset
            .fetch_add(by_ms, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Clock for SteppedClock {
    fn now_ms(&self) -> i64 {
        self.inner.now_ms() + self.offset.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Untouched by a step, which is the point of it.
    fn monotonic_ms(&self) -> i64 {
        self.inner.monotonic_ms()
    }
}

// ------------------------------------------------------------------ probe

#[derive(Clone, Debug)]
pub struct HostState {
    /// Whether echoes come back.
    pub up: bool,
    pub rtt_ms: f64,
    /// Echoes lost out of every burst, even when the host is up.
    pub lost: u32,
    /// Whether a TCP connect to the management port succeeds.
    pub tcp_up: bool,
    /// How long a probe of this host takes (drives "one slow device delays
    /// nothing else").
    pub delay: Duration,
    /// When set, the probe itself could not be attempted.
    pub error: Option<String>,
}

impl Default for HostState {
    fn default() -> Self {
        HostState {
            up: true,
            rtt_ms: 1.5,
            lost: 0,
            tcp_up: true,
            delay: Duration::ZERO,
            error: None,
        }
    }
}

/// A network that answers however the test says.
pub struct FakeProbe {
    hosts: Mutex<HashMap<String, HostState>>,
    default: Mutex<HostState>,
    method: String,
    /// Probes in flight now, and the most there have ever been at once.
    in_flight: Mutex<(usize, usize)>,
    pub pinged: Mutex<Vec<String>>,
}

impl Default for FakeProbe {
    fn default() -> Self {
        FakeProbe {
            hosts: Mutex::new(HashMap::new()),
            default: Mutex::new(HostState::default()),
            method: "icmp_datagram".into(),
            in_flight: Mutex::new((0, 0)),
            pinged: Mutex::new(Vec::new()),
        }
    }
}

impl FakeProbe {
    pub fn set(&self, address: &str, state: HostState) {
        self.hosts.lock().insert(address.to_string(), state);
    }

    pub fn down(&self, address: &str) {
        self.set(
            address,
            HostState {
                up: false,
                tcp_up: false,
                ..HostState::default()
            },
        );
    }

    pub fn up(&self, address: &str) {
        self.set(address, HostState::default());
    }

    /// The most probes that have been in flight at once.
    pub fn peak_in_flight(&self) -> usize {
        self.in_flight.lock().1
    }

    pub fn count(&self, address: &str) -> usize {
        self.pinged
            .lock()
            .iter()
            .filter(|seen| *seen == address)
            .count()
    }

    fn state(&self, address: &str) -> HostState {
        self.hosts
            .lock()
            .get(address)
            .cloned()
            .unwrap_or_else(|| self.default.lock().clone())
    }

    async fn enter(&self, delay: Duration) {
        {
            let mut counts = self.in_flight.lock();
            counts.0 += 1;
            counts.1 = counts.1.max(counts.0);
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }

    fn leave(&self) {
        self.in_flight.lock().0 -= 1;
    }
}

#[async_trait]
impl Reachability for FakeProbe {
    async fn ping(&self, address: &str, count: u32, _timeout: Duration) -> Result<Echoes, String> {
        self.pinged.lock().push(address.to_string());
        let state = self.state(address);
        self.enter(state.delay).await;
        self.leave();
        if let Some(error) = state.error {
            return Err(error);
        }
        let received = if state.up {
            count.saturating_sub(state.lost)
        } else {
            0
        };
        Ok(Echoes {
            sent: count,
            received,
            rtt_ms: (received > 0).then_some(state.rtt_ms),
        })
    }

    async fn tcp(&self, address: &str, port: u16, _timeout: Duration) -> Result<f64, String> {
        let state = self.state(address);
        self.enter(state.delay).await;
        self.leave();
        if state.tcp_up {
            Ok(state.rtt_ms)
        } else {
            Err(format!("port {port} did not answer"))
        }
    }

    fn method(&self) -> String {
        self.method.clone()
    }
}

// ------------------------------------------------------------------- SNMP

/// One interface of a fake agent.
#[derive(Clone, Debug, Default)]
pub struct AgentIf {
    pub name: String,
    pub alias: String,
    pub admin_up: bool,
    pub oper_up: bool,
    pub speed_mbps: u64,
    pub in_octets: u64,
    pub out_octets: u64,
    pub in_errors: u64,
    pub out_errors: u64,
    pub in_discards: u64,
    pub out_discards: u64,
}

impl AgentIf {
    pub fn up(name: &str, speed_mbps: u64) -> AgentIf {
        AgentIf {
            name: name.into(),
            admin_up: true,
            oper_up: true,
            speed_mbps,
            ..AgentIf::default()
        }
    }
}

/// What one fake agent answers with.
#[derive(Clone, Debug, Default)]
pub struct AgentData {
    pub uptime_ticks: Option<u32>,
    pub interfaces: Vec<AgentIf>,
    /// Scalars beyond the system group (CPU, memory, temperature OIDs).
    pub scalars: Vec<(String, Value)>,
    /// When set, every request fails with this message.
    pub fail: Option<String>,
    pub counter64: bool,
    pub delay: Duration,
}

type Agents = Arc<Mutex<HashMap<String, AgentData>>>;

/// The agents a test's devices talk to, by address.
#[derive(Default)]
pub struct FakeSnmp {
    agents: Agents,
    pub opened: Mutex<Vec<String>>,
}

impl FakeSnmp {
    pub fn set(&self, address: &str, data: AgentData) {
        self.agents.lock().insert(address.to_string(), data);
    }

    /// Change one agent in place (advance its counters, reboot it, …).
    pub fn edit(&self, address: &str, change: impl FnOnce(&mut AgentData)) {
        let mut agents = self.agents.lock();
        if let Some(agent) = agents.get_mut(address) {
            change(agent);
        }
    }

    /// How many sessions have been opened towards an address: one, however
    /// many polls there were, unless the credentials changed or a request
    /// failed.
    pub fn opens(&self, address: &str) -> usize {
        self.opened
            .lock()
            .iter()
            .filter(|seen| *seen == address)
            .count()
    }
}

#[async_trait]
impl SnmpTransport for FakeSnmp {
    async fn open(
        &self,
        target: SocketAddr,
        _auth: Auth,
        _timeout: Duration,
    ) -> Result<Box<dyn SnmpSession>, String> {
        let key = target.to_string();
        self.opened.lock().push(key.clone());
        if !self.agents.lock().contains_key(&key) {
            return Err(format!("no SNMP agent is listening at {key}"));
        }
        Ok(Box::new(FakeSession {
            address: key,
            agents: self.agents.clone(),
        }))
    }
}

/// A live session on a fake agent: it re-reads the agent each request, so a
/// test can move the counters on between polls.
struct FakeSession {
    address: String,
    agents: Agents,
}

impl FakeSession {
    fn data(&self) -> AgentData {
        self.agents
            .lock()
            .get(&self.address)
            .cloned()
            .unwrap_or_default()
    }
}

/// A session straight onto an [`AgentData`], with no transport in between.
#[derive(Default)]
pub struct FakeAgentSession {
    pub scalars: Vec<VarBind>,
    pub rows: Vec<TableRow>,
    pub fail: Option<String>,
}

#[async_trait]
impl SnmpSession for FakeAgentSession {
    async fn get(&mut self, oids: &[Oid]) -> Result<Vec<VarBind>, String> {
        if let Some(error) = &self.fail {
            return Err(error.clone());
        }
        Ok(self
            .scalars
            .iter()
            .filter(|bind| oids.contains(&bind.oid))
            .cloned()
            .collect())
    }

    async fn table(&mut self, columns: &[Oid], limit: usize) -> Result<Vec<TableRow>, String> {
        if let Some(error) = &self.fail {
            return Err(error.clone());
        }
        // As a real walk: only rows with a cell in a column asked for, and no
        // more of them than asked for.
        Ok(self
            .rows
            .iter()
            .map(|row| TableRow {
                index: row.index.clone(),
                values: row
                    .values
                    .iter()
                    .filter(|(oid, _)| columns.contains(oid))
                    .cloned()
                    .collect(),
            })
            .filter(|row| !row.values.is_empty())
            .take(limit)
            .collect())
    }

    fn supports_counter64(&self) -> bool {
        true
    }
}

fn oid(text: &str) -> Oid {
    text.parse().expect("a well-formed OID")
}

#[async_trait]
impl SnmpSession for FakeSession {
    async fn get(&mut self, oids: &[Oid]) -> Result<Vec<VarBind>, String> {
        let data = self.data();
        if !data.delay.is_zero() {
            tokio::time::sleep(data.delay).await;
        }
        if let Some(error) = data.fail {
            return Err(error);
        }
        let mut binds = Vec::new();
        let uptime = oid(oids::SYS_UP_TIME);
        for wanted in oids {
            if *wanted == uptime {
                if let Some(ticks) = data.uptime_ticks {
                    binds.push(VarBind {
                        oid: uptime.clone(),
                        value: Value::TimeTicks(ticks),
                    });
                }
                continue;
            }
            if let Some((_, value)) = data.scalars.iter().find(|(text, _)| oid(text) == *wanted) {
                binds.push(VarBind {
                    oid: wanted.clone(),
                    value: value.clone(),
                });
            }
        }
        Ok(binds)
    }

    async fn table(&mut self, columns: &[Oid], limit: usize) -> Result<Vec<TableRow>, String> {
        let data = self.data();
        if !data.delay.is_zero() {
            tokio::time::sleep(data.delay).await;
        }
        if let Some(error) = data.fail {
            return Err(error);
        }
        let mut rows: Vec<TableRow> = Vec::new();
        for (position, interface) in data.interfaces.iter().enumerate() {
            let index = position as u32 + 1;
            let mut values: Vec<(Oid, Value)> = vec![
                (oid(oids::IF_NAME), Value::from(interface.name.as_str())),
                (oid(oids::IF_DESCR), Value::from(interface.name.as_str())),
                (oid(oids::IF_ALIAS), Value::from(interface.alias.as_str())),
                (
                    oid(oids::IF_ADMIN_STATUS),
                    Value::Integer(if interface.admin_up { 1 } else { 2 }),
                ),
                (
                    oid(oids::IF_OPER_STATUS),
                    Value::Integer(if interface.oper_up { 1 } else { 2 }),
                ),
                (
                    oid(oids::IF_HIGH_SPEED),
                    Value::Gauge32(interface.speed_mbps as u32),
                ),
                (
                    oid(oids::IF_IN_ERRORS),
                    Value::Counter32(interface.in_errors as u32),
                ),
                (
                    oid(oids::IF_OUT_ERRORS),
                    Value::Counter32(interface.out_errors as u32),
                ),
                (
                    oid(oids::IF_IN_DISCARDS),
                    Value::Counter32(interface.in_discards as u32),
                ),
                (
                    oid(oids::IF_OUT_DISCARDS),
                    Value::Counter32(interface.out_discards as u32),
                ),
            ];
            if data.counter64 {
                values.push((
                    oids::IF_HC_IN_OCTETS.parse().unwrap(),
                    Value::Counter64(interface.in_octets),
                ));
                values.push((
                    oids::IF_HC_OUT_OCTETS.parse().unwrap(),
                    Value::Counter64(interface.out_octets),
                ));
            } else {
                values.push((
                    oid(oids::IF_IN_OCTETS),
                    Value::Counter32(interface.in_octets as u32),
                ));
                values.push((
                    oid(oids::IF_OUT_OCTETS),
                    Value::Counter32(interface.out_octets as u32),
                ));
            }
            values.retain(|(column, _)| columns.contains(column));
            if !values.is_empty() {
                rows.push(TableRow {
                    index: vec![index],
                    values,
                });
            }
        }
        // Everything else the agent holds, under the columns asked for, the
        // way an agent answers a walk: what follows a column in an OID is the
        // row's index, one arc or several; the cells of one index are one
        // row; rows come in index order, and no more of them than asked for.
        let mut cells: std::collections::BTreeMap<Vec<u32>, Vec<(Oid, Value)>> =
            std::collections::BTreeMap::new();
        for (text, value) in &data.scalars {
            let instance = oid(text);
            for column in columns {
                if let Some(index) = instance.suffix(column).filter(|index| !index.is_empty()) {
                    cells
                        .entry(index.to_vec())
                        .or_default()
                        .push((column.clone(), value.clone()));
                }
            }
        }
        rows.extend(
            cells
                .into_iter()
                .map(|(index, values)| TableRow { index, values }),
        );
        rows.truncate(limit);
        Ok(rows)
    }

    fn supports_counter64(&self) -> bool {
        self.data().counter64
    }
}

// --------------------------------------------------------------- harness

/// An app with this feature registered, fake probes and a virtual clock.
pub struct Harness {
    pub app: App,
    pub probe: Arc<FakeProbe>,
    pub snmp: Arc<FakeSnmp>,
    pub clock: Arc<dyn Clock>,
    /// The credential every device created here is given.
    pub credential_id: String,
}

impl Harness {
    /// A running app with no devices. Use [`Harness::device`] to add some.
    pub async fn new() -> Harness {
        Harness::with_clock(Arc::new(VirtualClock::new(epoch()))).await
    }

    /// The same, on a clock of the test's choosing.
    pub async fn with_clock(clock: Arc<dyn Clock>) -> Harness {
        let probe = Arc::new(FakeProbe::default());
        let snmp = Arc::new(FakeSnmp::default());
        let mut builder = nocturne_app::testing::builder();
        crate::init(&mut builder);
        // Replaces the state `init` registered: the same wiring, fake probes,
        // and no scheduler until a test asks for one.
        builder.state(MonitorState::fake(
            clock.clone(),
            probe.clone(),
            snmp.clone(),
        ));
        let lab = nocturne_app::testing::lab(builder, &[]).await;
        Harness {
            app: lab.app,
            probe,
            snmp,
            clock,
            credential_id: lab.credential_id,
        }
    }

    /// Give the shared credential an SNMPv2c community, so devices on it can
    /// be polled over SNMP.
    pub async fn with_snmp_credential(&self, community: &str) {
        self.rpc(
            "credentials.update",
            json!({"id": self.credential_id, "snmp_version": "v2c", "snmp_community": community}),
        )
        .await;
    }

    /// A device that is polled over SNMP as well as ICMP, with an agent
    /// listening at `<address>:161`.
    pub async fn snmp_device(&self, name: &str, address: &str, agent: AgentData) -> String {
        self.snmp.set(&format!("{address}:161"), agent);
        self.device(
            name,
            address,
            json!({"credential_id": self.credential_id, "monitor": {"snmp": true}}),
        )
        .await
    }

    /// Pin a device's polling phase so that the scheduler's very next tick
    /// finds it due.
    ///
    /// The scheduler spreads devices across their interval with a jitter
    /// hashed from the device id ([`crate::scheduler::jitter`]), and the core
    /// hands out a fresh random id for every device created — so *when* a
    /// device is first polled is random from a test's point of view. The
    /// jitter only applies to a device the scheduler has not seen before
    /// (no `last_poll_mono` yet), so writing a phase here makes it part of the
    /// fixture. Any test that asserts how often something was polled in a
    /// given window pins its devices first; the test that is *about* the
    /// spread deliberately does not.
    pub fn due_now(&self, device_id: &str, interval_s: u32) {
        let interval_ms = i64::from(interval_s.max(5)) * 1_000;
        let now = self.clock.monotonic_ms();
        self.app
            .state::<MonitorState>()
            .device(device_id, |runtime| {
                runtime.last_poll_mono = Some(now - interval_ms);
            });
    }

    /// Switch every alert rule off, for tests that are not about rules.
    pub async fn silence_rules(&self) {
        for rule in crate::rules::list(&self.app).unwrap() {
            self.rpc(
                "alerts.rules.save",
                json!({"id": rule.id, "enabled": false}),
            )
            .await;
        }
    }

    /// Leave exactly one rule enabled.
    pub async fn only_rule(&self, id: &str) {
        for rule in crate::rules::list(&self.app).unwrap() {
            self.rpc(
                "alerts.rules.save",
                json!({"id": rule.id, "enabled": rule.id == id}),
            )
            .await;
        }
    }

    /// Add a service check to a device, returning its id. `patch` is merged
    /// into the `checks.save` parameters.
    pub async fn check_on(&self, device_id: &str, name: &str, spec: Json, patch: Json) -> String {
        let mut params = json!({"device_id": device_id, "name": name, "spec": spec});
        merge(&mut params, patch);
        let saved = self.rpc("checks.save", params).await;
        saved["id"].as_str().expect("an id").to_string()
    }

    /// Run one service check and let the rules see it, as the scheduler does.
    pub async fn run_check(&self, check_id: &str) -> crate::checks::store::Recorded {
        let (check, _) = crate::checks::store::require(&self.app, check_id).expect("the check");
        crate::checks::run_and_record(&self.app, &check)
            .await
            .expect("the check runs")
    }

    /// What a check last found.
    pub fn check_state(&self, check_id: &str) -> crate::checks::model::CheckState {
        crate::checks::store::require(&self.app, check_id)
            .expect("the check")
            .1
    }

    /// Pin a check's phase so that the scheduler's very next tick finds it
    /// due, for the same reason [`Harness::due_now`] exists for devices: the
    /// slot inside the interval is hashed from a freshly generated id, so
    /// *when* a check first runs is random from a test's point of view.
    pub fn check_due_now(&self, check_id: &str, interval_s: u32) {
        let interval_ms = i64::from(interval_s.max(5)) * 1_000;
        let now = self.clock.monotonic_ms();
        self.app.state::<MonitorState>().check(check_id, |runtime| {
            runtime.last_run_mono = Some(now - interval_ms);
        });
    }

    /// Poll a device and run the alert rules over the result, as the
    /// scheduler does.
    pub async fn poll_and_evaluate(&self, device_id: &str) -> crate::poll::Poll {
        let poll = self.poll(device_id).await;
        crate::rules::evaluate(&self.app, &poll).await;
        poll
    }

    /// Open alerts, newest first.
    pub fn open_alerts(&self) -> Vec<crate::model::Alert> {
        crate::alerts::list(
            &self.app,
            &crate::alerts::AlertQuery {
                state: Some("open".into()),
                limit: Some(500),
                ..crate::alerts::AlertQuery::default()
            },
        )
        .unwrap()
    }

    /// Every alert ever raised, newest first.
    pub fn all_alerts(&self) -> Vec<crate::model::Alert> {
        crate::alerts::list(
            &self.app,
            &crate::alerts::AlertQuery {
                limit: Some(500),
                ..crate::alerts::AlertQuery::default()
            },
        )
        .unwrap()
    }

    /// Entries of the `event` log stream for a device.
    pub fn events_of(&self, device_id: &str) -> Vec<nocturne_app::store::LogEntry> {
        self.app
            .store()
            .log_query(&nocturne_app::store::LogQuery {
                stream: Some("event".into()),
                device_id: Some(device_id.to_string()),
                limit: Some(200),
                ..Default::default()
            })
            .unwrap()
    }

    pub fn now(&self) -> i64 {
        self.clock.now_ms()
    }

    /// Add a device, returning its id. `patch` is merged into the
    /// `devices.create` parameters.
    pub async fn device(&self, name: &str, address: &str, patch: Json) -> String {
        let mut params = json!({
            "name": name,
            "address": address,
            "transport": "none",
            "monitor": {"enabled": true, "icmp": true, "snmp": false, "interval_s": 60},
        });
        merge(&mut params, patch);
        let device = self
            .app
            .rpc("devices.create", params)
            .await
            .expect("the device is created");
        device["id"].as_str().expect("an id").to_string()
    }

    /// Poll one device once, as the scheduler would.
    pub async fn poll(&self, device_id: &str) -> crate::poll::Poll {
        crate::poll::poll_device(&self.app, device_id)
            .await
            .expect("the poll runs")
    }

    /// Every sample of one series, oldest first.
    ///
    /// The window is the last day rather than all of time, because the store
    /// answers a range from the finest resolution that *exists* for it: ask
    /// since the epoch and it reasonably decides that only hourly roll-ups
    /// could cover such a span, and a test's samples — written seconds ago
    /// and never rolled up — are not in them. A day keeps it on the raw
    /// table, which is where a test's samples live.
    pub fn samples(&self, device_id: &str, metric: &str, instance: &str) -> Vec<(i64, f64)> {
        let store = self.app.store();
        let series = store.series_id(device_id, metric, instance).unwrap();
        let now = nocturne_app::util::now_ms();
        store
            .samples_query(series, now - 86_400_000, now + 3_600_000, 5_000)
            .unwrap()
            .into_iter()
            .map(|point| (point.ts, point.avg))
            .collect()
    }

    pub async fn rpc(&self, method: &str, params: Json) -> Json {
        self.app
            .rpc(method, params)
            .await
            .unwrap_or_else(|err| panic!("{method}: {err}"))
    }
}

fn merge(base: &mut Json, patch: Json) {
    match (base, patch) {
        (Json::Object(base), Json::Object(patch)) => {
            for (key, value) in patch {
                match base.get_mut(&key) {
                    Some(existing) => merge(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, patch) => *base = patch,
    }
}
