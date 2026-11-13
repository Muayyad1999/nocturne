//! Scaffolding for this crate's tests.
//!
//! [`FakeProbes`] stands in for the network: it answers exactly what a test
//! says it answers, and records every packet discovery asked it to send with
//! the time it was sent, which is how the rate limit and the "no credential
//! is ever tried against a found device" rules are checked. Everything else
//! in a test is real — the core, the job surface, the profile registry, the
//! inventory and the store.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nocturne_app::testing::Lab;
use nocturne_app::{App, AppResult, Event};
use nocturne_netprobe::local::{LocalInterface, LocalRoute, Neighbour};
use nocturne_snmp::Auth;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::sync::broadcast::{self, error::TryRecvError};

use crate::db;
use crate::probe::{PortAnswer, Probes, SysInfo};

/// What one address answers.
#[derive(Clone, Debug, Default)]
pub struct FakeHost {
    /// Round-trip time of an echo reply; `None` = silence.
    pub echo_ms: Option<f64>,
    /// Ports that accept a connection, and what they say first.
    pub ports: BTreeMap<u16, Option<String>>,
    /// SNMP answers by credential key (see [`auth_key`]).
    pub snmp: BTreeMap<String, SysInfo>,
    /// What reverse DNS says.
    pub ptr: Option<String>,
}

impl FakeHost {
    pub fn alive() -> FakeHost {
        FakeHost {
            echo_ms: Some(1.5),
            ..FakeHost::default()
        }
    }

    pub fn port(mut self, port: u16, banner: Option<&str>) -> FakeHost {
        self.ports.insert(port, banner.map(str::to_string));
        self
    }

    pub fn snmp(mut self, credential: &str, info: SysInfo) -> FakeHost {
        self.snmp.insert(credential.to_string(), info);
        self
    }

    pub fn ptr(mut self, name: &str) -> FakeHost {
        self.ptr = Some(name.to_string());
        self
    }
}

/// One packet discovery asked to be sent.
#[derive(Clone, Debug, PartialEq)]
pub struct Sent {
    /// `echo`, `tcp`, `snmp` or `dns`.
    pub kind: String,
    pub address: String,
    /// The port for `tcp` and `snmp`, `0` otherwise.
    pub port: u16,
    /// For `snmp`: which credential was offered.
    pub credential: String,
    /// When it was sent, on tokio's clock.
    pub at: tokio::time::Instant,
}

#[derive(Default)]
pub struct FakeProbes {
    hosts: Mutex<HashMap<String, FakeHost>>,
    neighbours: Mutex<Vec<Neighbour>>,
    interfaces: Mutex<Vec<LocalInterface>>,
    gateways: Mutex<Vec<LocalRoute>>,
    sent: Mutex<Vec<Sent>>,
    /// Added to every probe, so a test can make one take long enough to cancel.
    delay: Mutex<Duration>,
}

impl FakeProbes {
    pub fn new() -> Arc<FakeProbes> {
        Arc::new(FakeProbes::default())
    }

    pub fn host(self: &Arc<Self>, address: &str, host: FakeHost) -> Arc<Self> {
        self.hosts.lock().insert(address.to_string(), host);
        self.clone()
    }

    pub fn neighbour(self: &Arc<Self>, address: &str, mac: &str, state: &str) -> Arc<Self> {
        self.neighbours.lock().push(Neighbour {
            address: address.to_string(),
            mac: mac.to_string(),
            interface: "eth0".to_string(),
            state: state.to_string(),
        });
        self.clone()
    }

    pub fn interface(self: &Arc<Self>, name: &str, cidr: &str, up: bool) -> Arc<Self> {
        self.interfaces.lock().push(LocalInterface {
            name: name.to_string(),
            addresses: vec![cidr.to_string()],
            mac: None,
            is_loopback: name == "lo",
            is_up: up,
        });
        self.clone()
    }

    pub fn gateway(self: &Arc<Self>, gateway: &str, interface: &str) -> Arc<Self> {
        self.gateways.lock().push(LocalRoute {
            destination: "0.0.0.0/0".to_string(),
            gateway: gateway.to_string(),
            interface: interface.to_string(),
        });
        self.clone()
    }

    pub fn delay(self: &Arc<Self>, delay: Duration) -> Arc<Self> {
        *self.delay.lock() = delay;
        self.clone()
    }

    /// Every packet sent so far, in order.
    pub fn sent(&self) -> Vec<Sent> {
        self.sent.lock().clone()
    }

    pub fn sent_of(&self, kind: &str) -> Vec<Sent> {
        self.sent()
            .into_iter()
            .filter(|sent| sent.kind == kind)
            .collect()
    }

    fn record(&self, kind: &str, address: IpAddr, port: u16, credential: &str) {
        self.sent.lock().push(Sent {
            kind: kind.to_string(),
            address: address.to_string(),
            port,
            credential: credential.to_string(),
            at: tokio::time::Instant::now(),
        });
    }

    async fn pause(&self) {
        let delay = *self.delay.lock();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
}

#[async_trait]
impl Probes for FakeProbes {
    async fn echo(&self, address: IpAddr, _timeout: Duration) -> Option<f64> {
        self.record("echo", address, 0, "");
        self.pause().await;
        self.hosts
            .lock()
            .get(&address.to_string())
            .and_then(|host| host.echo_ms)
    }

    async fn ports(
        &self,
        address: IpAddr,
        ports: &[u16],
        _timeout: Duration,
        grab_banners: bool,
    ) -> Vec<PortAnswer> {
        let host = self.hosts.lock().get(&address.to_string()).cloned();
        let mut answers = Vec::new();
        for port in ports {
            self.record("tcp", address, *port, "");
            self.pause().await;
            let open = host.as_ref().and_then(|host| host.ports.get(port).cloned());
            let is_open = open.is_some();
            answers.push(PortAnswer {
                port: *port,
                open: is_open,
                service: match port {
                    22 => "ssh".into(),
                    23 => "telnet".into(),
                    80 => "http".into(),
                    443 => "https".into(),
                    8291 => "winbox".into(),
                    _ => String::new(),
                },
                banner: open.flatten().filter(|_| grab_banners),
                rtt_ms: is_open.then_some(2.0),
            });
        }
        answers
    }

    async fn snmp(
        &self,
        target: SocketAddr,
        auth: Auth,
        _timeout: Duration,
    ) -> Result<SysInfo, String> {
        let key = auth_key(&auth);
        self.record("snmp", target.ip(), target.port(), &key);
        self.pause().await;
        self.hosts
            .lock()
            .get(&target.ip().to_string())
            .and_then(|host| host.snmp.get(&key).cloned())
            .ok_or_else(|| "no answer".to_string())
    }

    async fn reverse_dns(&self, address: IpAddr, _timeout: Duration) -> Option<String> {
        self.record("dns", address, 0, "");
        self.hosts
            .lock()
            .get(&address.to_string())
            .and_then(|host| host.ptr.clone())
    }

    async fn neighbours(&self) -> Vec<Neighbour> {
        self.neighbours.lock().clone()
    }

    async fn interfaces(&self) -> Vec<LocalInterface> {
        self.interfaces.lock().clone()
    }

    async fn gateways(&self) -> Vec<LocalRoute> {
        self.gateways.lock().clone()
    }
}

/// How a test names a credential when it says what an agent answers.
pub fn auth_key(auth: &Auth) -> String {
    match auth {
        Auth::V1 { community } | Auth::V2c { community } => community.clone(),
        Auth::V3 { user, .. } => format!("v3:{user}"),
    }
}

/// A device type with `detect:` rules, so identification has something real
/// to match against. Same id as the core's test profile, which it replaces,
/// plus an ARP collector the crawl tests need.
pub const TEST_PROFILE: &str = r#"
id: test_ios
name: Test IOS
vendor: Test
os: IOS
extends: test_ios_base
detect:
  priority: 10
  sysdescr: ['Test IOS Software, C2960 Software']
  sysobjectid: ['1.3.6.1.4.1.99999.1.']
  banner: ['SSH-2.0-TestIOS']
collect:
  arp:
    - command: show ip arp
      parser: {regex: '(?m)^Internet\s+(?P<ip>\S+)\s+\S+\s+(?P<mac>\S+)\s+ARPA\s+(?P<interface>\S+)\s*$'}
      map: {ip: ip, mac: mac, interface: interface}
  neighbors:
    - command: show cdp neighbors detail
      parser: {regex: 'Device ID: (?P<device>\S+)[\s\S]*?IP address: (?P<address>\S+)[\s\S]*?Platform: (?P<platform>[^,]+),\s+Capabilities: (?P<capabilities>[^\n]*)[\s\S]*?Interface: (?P<local>[^,]+),\s+Port ID \(outgoing port\): (?P<remote>\S+)'}
      map:
        device: device
        address: address
        platform: platform
        capabilities: capabilities
        local_interface: local
        remote_interface: remote
        protocol: "'cdp'"
"#;

/// An app with the core, this feature, `names` as fake switches, and the
/// fake probe layer in place of the network.
pub async fn lab(names: &[&str]) -> (Lab, Arc<FakeProbes>) {
    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    let lab = nocturne_app::testing::lab(builder, names).await;
    // The base keeps everything the core's fake device needs; the override
    // adds detection rules and an ARP collector on top of it.
    let base = nocturne_app::testing::LAB_IOS.replace("id: test_ios", "id: test_ios_base");
    nocturne_app::profiles::save(&lab.app, &base).expect("the base test profile loads");
    nocturne_app::profiles::save(&lab.app, TEST_PROFILE).expect("the test profile loads");
    let probes = FakeProbes::new();
    db::state(&lab.app).probes.set(probes.clone());
    (lab, probes)
}

/// Start a scan and wait for its job to end. Returns `(scan_id, job)`.
pub async fn scan(app: &App, params: Value) -> AppResult<(String, nocturne_app::jobs::Job)> {
    let started = app.rpc("discovery.scan", params).await?;
    let scan_id = started["scan_id"].as_str().expect("scan id").to_string();
    let job_id = started["job_id"].as_str().expect("job id").to_string();
    let job = nocturne_app::jobs::wait(app, &job_id).await?;
    Ok((scan_id, job))
}

/// The finds of a scan, as the UI would read them.
pub async fn hosts(app: &App, scan_id: &str) -> Vec<Value> {
    let result = app
        .rpc("discovery.scan_get", json!({"scan_id": scan_id}))
        .await
        .expect("the scan can be read");
    result["hosts"].as_array().cloned().unwrap_or_default()
}

/// One find by address.
pub fn host_at<'a>(hosts: &'a [Value], address: &str) -> &'a Value {
    hosts
        .iter()
        .find(|host| host["address"] == address)
        .unwrap_or_else(|| panic!("no find for {address} among {hosts:#?}"))
}

/// The evidence lines of a find for one field.
pub fn evidence_for(host: &Value, field: &str) -> Vec<String> {
    host["evidence"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry["field"] == field)
        .map(|entry| {
            format!(
                "{}: {}",
                entry["source"].as_str().unwrap_or_default(),
                entry["detail"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

/// Everything the app published since this was created.
///
/// What is read off the bus is kept, so asking about one topic does not
/// consume another's events — several `of` calls in one test all see the
/// whole run.
pub struct Events {
    events: broadcast::Receiver<Event>,
    seen: Vec<Event>,
}

impl Events {
    pub fn new(app: &App) -> Events {
        Events {
            events: app.subscribe(),
            seen: Vec::new(),
        }
    }

    pub fn drain(&mut self) -> Vec<Event> {
        loop {
            match self.events.try_recv() {
                Ok(event) => self.seen.push(event),
                Err(TryRecvError::Lagged(_)) => continue,
                Err(_) => return self.seen.clone(),
            }
        }
    }

    pub fn of(&mut self, topic: &str) -> Vec<Value> {
        self.drain()
            .into_iter()
            .filter(|event| event.topic == topic)
            .map(|event| event.payload)
            .collect()
    }

    pub fn topics(&mut self) -> Vec<String> {
        self.drain().into_iter().map(|event| event.topic).collect()
    }
}

/// A credential that holds an SNMP community, as a user would create one.
pub async fn snmp_credential(app: &App, name: &str, community: &str) -> String {
    let created = app
        .rpc(
            "credentials.create",
            json!({"name": name, "snmp_version": "v2c", "snmp_community": community}),
        )
        .await
        .expect("the credential is created");
    created["id"].as_str().expect("id").to_string()
}

/// `SysInfo` as an agent of a known platform would answer.
pub fn ios_sysinfo(name: &str) -> SysInfo {
    SysInfo {
        sys_descr: Some(
            "Test IOS Software, C2960 Software (C2960-LANBASEK9-M), Version 15.0(2)SE11".into(),
        ),
        sys_object_id: Some("1.3.6.1.4.1.99999.1.516".into()),
        sys_name: Some(name.into()),
        uptime_s: Some(864_000),
    }
}

/// The audit entries whose action starts with `prefix`.
pub fn audits(app: &App, prefix: &str) -> Vec<String> {
    app.store()
        .log_query(&nocturne_app::store::LogQuery {
            stream: Some("audit".into()),
            limit: Some(200),
            ..Default::default()
        })
        .expect("the audit trail can be read")
        .into_iter()
        .filter(|entry| entry.source.starts_with(prefix))
        .map(|entry| entry.message)
        .collect()
}
