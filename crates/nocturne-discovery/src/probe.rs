//! The packets discovery is allowed to send, behind one trait.
//!
//! Everything that leaves this machine during a scan goes through [`Probes`]:
//! an echo request, a TCP connection, an SNMP GET, a reverse lookup, a read
//! of this machine's own neighbour table. Nothing else — in particular there
//! is no method here that logs in anywhere, because a find is a find and a
//! login is a separate, explicit step the user takes afterwards.
//!
//! Having one trait buys two things. Tests can count and schedule every
//! packet (the rate-limit test does exactly that); and the places that may
//! talk to the network are a list of five functions rather than a habit.
//!
//! [`Rate`] wraps the call sites, not the implementation, so the bound holds
//! for the real probes and for anything a test injects.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use nocturne_netprobe::local::{LocalInterface, LocalRoute, Neighbour};
use nocturne_netprobe::ping::Pinger;
use nocturne_netprobe::{dns, local, tcp};
use nocturne_snmp::{Auth, Client, ClientConfig, Oid, oids};

/// What one TCP port answered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PortAnswer {
    pub port: u16,
    pub open: bool,
    pub service: String,
    pub banner: Option<String>,
    pub rtt_ms: Option<f64>,
}

/// What an agent said about itself, straight from the four system scalars.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SysInfo {
    pub sys_descr: Option<String>,
    pub sys_object_id: Option<String>,
    pub sys_name: Option<String>,
    pub uptime_s: Option<u64>,
}

impl SysInfo {
    /// An agent that answered but said nothing identifiable is not a find.
    pub fn is_empty(&self) -> bool {
        self.sys_descr.is_none()
            && self.sys_object_id.is_none()
            && self.sys_name.is_none()
            && self.uptime_s.is_none()
    }
}

/// Everything discovery may send, and the two local tables it may read.
#[async_trait]
pub trait Probes: Send + Sync + 'static {
    /// One echo request. `Some(rtt)` when the host answered.
    async fn echo(&self, address: IpAddr, timeout: Duration) -> Option<f64>;

    /// One TCP connection per port. `grab_banners` reads only what the server
    /// volunteers (and one `HEAD /` on plain-HTTP ports); it never logs in.
    async fn ports(
        &self,
        address: IpAddr,
        ports: &[u16],
        timeout: Duration,
        grab_banners: bool,
    ) -> Vec<PortAnswer>;

    /// `sysDescr`, `sysObjectID`, `sysName`, `sysUpTime` with one credential.
    async fn snmp(
        &self,
        target: SocketAddr,
        auth: Auth,
        timeout: Duration,
    ) -> Result<SysInfo, String>;

    /// Reverse DNS. Asked of the resolver, not of the host.
    async fn reverse_dns(&self, address: IpAddr, timeout: Duration) -> Option<String>;

    /// This machine's ARP / NDP table.
    async fn neighbours(&self) -> Vec<Neighbour>;

    /// This machine's own interfaces.
    async fn interfaces(&self) -> Vec<LocalInterface>;

    /// This machine's default routes.
    async fn gateways(&self) -> Vec<LocalRoute>;
}

/// The real thing: `nocturne-netprobe` and `nocturne-snmp`.
pub struct RealProbes {
    pinger: Pinger,
}

impl Default for RealProbes {
    fn default() -> Self {
        RealProbes {
            pinger: Pinger::new(),
        }
    }
}

#[async_trait]
impl Probes for RealProbes {
    async fn echo(&self, address: IpAddr, timeout: Duration) -> Option<f64> {
        // A host that did not answer and a machine that cannot send are both
        // "no reply" to the caller; the difference is reported by the job log
        // through `Pinger::method`, not by inventing a result here.
        match self.pinger.ping_once(address, timeout, 56).await {
            Ok(reply) => Some(reply.rtt_ms),
            Err(error) => {
                tracing::trace!(%address, %error, "echo did not answer");
                None
            }
        }
    }

    async fn ports(
        &self,
        address: IpAddr,
        ports: &[u16],
        timeout: Duration,
        grab_banners: bool,
    ) -> Vec<PortAnswer> {
        if ports.is_empty() {
            return Vec::new();
        }
        let options = tcp::ScanOptions {
            timeout_ms: timeout.as_millis().max(1) as u64,
            // The sweep's own concurrency decides how many hosts are in
            // flight; a host's handful of ports goes at once.
            concurrency: ports.len().max(1),
            grab_banners,
        };
        match tcp::scan(&address.to_string(), ports, &options, None).await {
            Ok(results) => results
                .into_iter()
                .map(|result| PortAnswer {
                    port: result.port,
                    open: result.state == tcp::PortState::Open,
                    service: result.service,
                    banner: result.banner,
                    rtt_ms: result.rtt_ms,
                })
                .collect(),
            Err(error) => {
                tracing::debug!(%address, %error, "port check failed");
                Vec::new()
            }
        }
    }

    async fn snmp(
        &self,
        target: SocketAddr,
        auth: Auth,
        timeout: Duration,
    ) -> Result<SysInfo, String> {
        let config = ClientConfig {
            timeout,
            // One try per credential: a scan has many addresses to get through
            // and a silent agent is the common case, not an error.
            retries: 0,
            ..ClientConfig::new(target, auth)
        };
        let mut client = Client::connect(config)
            .await
            .map_err(|err| err.to_string())?;
        let parse = |oid: &str| -> Result<Oid, String> {
            oid.parse::<Oid>()
                .map_err(|err| format!("{oid} is not a valid object id: {err}"))
        };
        let (descr, object_id, name, uptime) = (
            parse(oids::SYS_DESCR)?,
            parse(oids::SYS_OBJECT_ID)?,
            parse(oids::SYS_NAME)?,
            parse(oids::SYS_UP_TIME)?,
        );
        let wanted = [
            descr.clone(),
            object_id.clone(),
            name.clone(),
            uptime.clone(),
        ];
        let binds = client.get(&wanted).await.map_err(|err| err.to_string())?;
        let mut info = SysInfo::default();
        for bind in binds {
            if bind.oid == descr {
                info.sys_descr = bind.value.as_text();
            } else if bind.oid == object_id {
                info.sys_object_id = match &bind.value {
                    nocturne_snmp::Value::Oid(oid) => Some(oid.to_string()),
                    _ => None,
                };
            } else if bind.oid == name {
                info.sys_name = bind.value.as_text();
            } else if bind.oid == uptime {
                // TimeTicks are hundredths of a second.
                info.uptime_s = bind.value.as_u64().map(|ticks| ticks / 100);
            }
        }
        Ok(info)
    }

    async fn reverse_dns(&self, address: IpAddr, timeout: Duration) -> Option<String> {
        dns::reverse(address, timeout).await
    }

    async fn neighbours(&self) -> Vec<Neighbour> {
        local::neighbours().await
    }

    async fn interfaces(&self) -> Vec<LocalInterface> {
        local::interfaces()
    }

    async fn gateways(&self) -> Vec<LocalRoute> {
        local::default_gateways().await
    }
}

/// A token bucket that paces everything one scan sends.
///
/// Permits are handed out on a schedule rather than refilled on a timer, so
/// the n-th probe of a scan cannot leave before `n / rate` seconds have
/// passed however many tasks are waiting. It uses tokio's clock, which means
/// a test with `tokio::time::pause()` measures the pacing exactly instead of
/// sleeping through it.
pub struct Rate {
    rate: u32,
    next: tokio::sync::Mutex<Option<tokio::time::Instant>>,
    sent: AtomicU64,
}

impl Rate {
    pub fn new(rate: u32) -> Rate {
        Rate {
            rate: rate.max(1),
            next: tokio::sync::Mutex::new(None),
            sent: AtomicU64::new(0),
        }
    }

    /// Wait for this scan's turn to send one packet.
    pub async fn permit(&self) {
        let spacing = Duration::from_secs_f64(1.0 / f64::from(self.rate));
        let now = tokio::time::Instant::now();
        let slot = {
            let mut next = self.next.lock().await;
            let slot = match *next {
                Some(scheduled) if scheduled > now => scheduled,
                _ => now,
            };
            *next = Some(slot + spacing);
            slot
        };
        self.sent.fetch_add(1, Ordering::Relaxed);
        if slot > now {
            tokio::time::sleep_until(slot).await;
        }
    }

    /// How many permits have been handed out: the scan's packet count.
    pub fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }
}

/// The probe layer the app will use, replaceable for tests.
pub struct ProbeLayer {
    probes: parking_lot::RwLock<Arc<dyn Probes>>,
}

impl Default for ProbeLayer {
    fn default() -> Self {
        ProbeLayer {
            probes: parking_lot::RwLock::new(Arc::new(RealProbes::default())),
        }
    }
}

impl ProbeLayer {
    pub fn get(&self) -> Arc<dyn Probes> {
        self.probes.read().clone()
    }

    pub fn set(&self, probes: Arc<dyn Probes>) {
        *self.probes.write() = probes;
    }
}
