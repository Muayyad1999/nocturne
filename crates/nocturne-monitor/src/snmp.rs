//! The SNMP side of a poll, behind a trait so the tests can answer for it.
//!
//! One long-lived session per device (SNMPv3 discovers the agent's engine and
//! localises its keys once, and a v2c session keeps its socket), kept in
//! [`Sessions`] and reopened when the device's credentials change.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nocturne_app::model::{Credential, Device, SnmpAccess};
use nocturne_app::{App, AppError, AppResult, credentials};
use nocturne_snmp::{Auth, AuthProtocol, Client, ClientConfig, Oid, PrivProtocol, Value, VarBind};

/// One row of a walked table: the index below the column OIDs, and the value
/// of each column that had one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableRow {
    pub index: Vec<u32>,
    pub values: Vec<(Oid, Value)>,
}

impl TableRow {
    pub fn get(&self, column: &Oid) -> Option<&Value> {
        self.values
            .iter()
            .find(|(oid, _)| oid == column)
            .map(|(_, value)| value)
    }

    pub fn u64(&self, column: &Oid) -> Option<u64> {
        self.get(column).and_then(Value::as_u64)
    }

    pub fn text(&self, column: &Oid) -> Option<String> {
        self.get(column).and_then(Value::as_text)
    }
}

/// A conversation with one agent.
#[async_trait]
pub trait SnmpSession: Send + Sync {
    async fn get(&mut self, oids: &[Oid]) -> Result<Vec<VarBind>, String>;
    /// Walk several columns of one table and pivot by row index.
    async fn table(&mut self, columns: &[Oid], limit: usize) -> Result<Vec<TableRow>, String>;
    /// Whether this session can ask for 64-bit counters (v2c and v3 can, v1 cannot).
    fn supports_counter64(&self) -> bool;
}

/// Opens sessions. The real one makes a [`nocturne_snmp::Client`]; a test
/// hands back whatever it wants to answer with.
#[async_trait]
pub trait SnmpTransport: Send + Sync + 'static {
    async fn open(
        &self,
        target: SocketAddr,
        auth: Auth,
        timeout: Duration,
    ) -> Result<Box<dyn SnmpSession>, String>;
}

pub type SharedSnmp = Arc<dyn SnmpTransport>;

// ------------------------------------------------------------------- real

/// SNMP over UDP, through `nocturne-snmp`.
#[derive(Default)]
pub struct RealSnmp;

struct ClientSession {
    client: Client,
    counter64: bool,
}

#[async_trait]
impl SnmpSession for ClientSession {
    async fn get(&mut self, oids: &[Oid]) -> Result<Vec<VarBind>, String> {
        self.client.get(oids).await.map_err(|err| err.to_string())
    }

    async fn table(&mut self, columns: &[Oid], limit: usize) -> Result<Vec<TableRow>, String> {
        let pivoted = self
            .client
            .table(columns, limit)
            .await
            .map_err(|err| err.to_string())?;
        Ok(pivoted
            .into_iter()
            .map(|(index, row)| TableRow {
                index,
                values: row.into_iter().collect(),
            })
            .collect())
    }

    fn supports_counter64(&self) -> bool {
        self.counter64
    }
}

#[async_trait]
impl SnmpTransport for RealSnmp {
    async fn open(
        &self,
        target: SocketAddr,
        auth: Auth,
        timeout: Duration,
    ) -> Result<Box<dyn SnmpSession>, String> {
        let counter64 = !matches!(auth, Auth::V1 { .. });
        let config = ClientConfig {
            timeout,
            ..ClientConfig::new(target, auth)
        };
        let client = Client::connect(config)
            .await
            .map_err(|err| err.to_string())?;
        Ok(Box::new(ClientSession { client, counter64 }))
    }
}

// --------------------------------------------------------------- sessions

/// The open sessions, one per device.
#[derive(Default)]
pub struct Sessions {
    open: parking_lot::Mutex<HashMap<String, Arc<tokio::sync::Mutex<Entry>>>>,
}

pub struct Entry {
    /// What the session was opened for; a change reopens it.
    fingerprint: String,
    session: Option<Box<dyn SnmpSession>>,
}

impl Sessions {
    fn slot(&self, device_id: &str) -> Arc<tokio::sync::Mutex<Entry>> {
        self.open
            .lock()
            .entry(device_id.to_string())
            .or_insert_with(|| {
                Arc::new(tokio::sync::Mutex::new(Entry {
                    fingerprint: String::new(),
                    session: None,
                }))
            })
            .clone()
    }

    /// Drop a device's session (it is reopened on the next poll).
    pub fn forget(&self, device_id: &str) {
        self.open.lock().remove(device_id);
    }
}

/// Everything needed to reach a device's agent, worked out from the device
/// and its credential. `Auth`'s own `Debug` masks communities and
/// passphrases, so printing a target cannot leak one.
#[derive(Debug)]
pub struct Target {
    pub address: SocketAddr,
    pub auth: Auth,
    /// Identifies the settings above, so a changed credential reopens the session.
    pub fingerprint: String,
}

/// Resolve the address, version and secrets of a device's SNMP agent.
///
/// The error says exactly what is missing, because "SNMP failed" is useless
/// to the person who has to fix it.
pub fn target(app: &App, device: &Device) -> AppResult<Target> {
    let credential_id = device
        .monitor
        .snmp_credential_id
        .clone()
        .or_else(|| device.credential_id.clone())
        .ok_or_else(|| {
            AppError::invalid(format!(
                "`{}` has SNMP monitoring switched on but no credential; assign one with an SNMP community or v3 user",
                device.name
            ))
        })?;
    let stored: Credential = app.store().require(
        nocturne_app::model::collections::CREDENTIALS,
        &credential_id,
        "credential",
    )?;
    let resolved = credentials::resolve(app, &credential_id)?;
    let access = &stored.snmp;
    let auth = build_auth(&device.name, access, &resolved)?;

    let port = device.monitor.snmp_port.unwrap_or(161);
    let address = resolve_address(&device.address, port).ok_or_else(|| {
        AppError::invalid(format!(
            "`{}` has the address `{}`, which is not an IP address SNMP can be sent to",
            device.name, device.address
        ))
    })?;
    // The version, the user and *which* secrets are set — never the secrets.
    let fingerprint = format!(
        "{address}|{}|{}|{}|{}|{credential_id}",
        access.version, access.user, access.auth_protocol, access.priv_protocol
    );
    Ok(Target {
        address,
        auth,
        fingerprint,
    })
}

fn build_auth(
    device: &str,
    access: &SnmpAccess,
    resolved: &credentials::Resolved,
) -> AppResult<Auth> {
    let community = || {
        resolved
            .snmp_community
            .as_ref()
            .map(|secret| secret.to_string())
            .ok_or_else(|| {
                AppError::unauthorized(format!(
                    "the credential of `{device}` has no SNMP community; add one to the credential"
                ))
            })
    };
    match access.version.as_str() {
        "v1" => Ok(Auth::V1 {
            community: community()?,
        }),
        "v2c" | "" => Ok(Auth::V2c {
            community: community()?,
        }),
        "v3" => {
            if access.user.trim().is_empty() {
                return Err(AppError::unauthorized(format!(
                    "the credential of `{device}` selects SNMPv3 but has no user name"
                )));
            }
            let auth = match (
                auth_protocol(&access.auth_protocol),
                &resolved.snmp_auth_password,
            ) {
                (Some(protocol), Some(password)) => Some((protocol, password.to_string())),
                (Some(_), None) => {
                    return Err(AppError::unauthorized(format!(
                        "the credential of `{device}` selects SNMPv3 authentication but has no authentication password"
                    )));
                }
                (None, _) => None,
            };
            let privacy = match (
                priv_protocol(&access.priv_protocol),
                &resolved.snmp_priv_password,
            ) {
                (Some(protocol), Some(password)) => Some((protocol, password.to_string())),
                (Some(_), None) => {
                    return Err(AppError::unauthorized(format!(
                        "the credential of `{device}` selects SNMPv3 privacy but has no privacy password"
                    )));
                }
                (None, _) => None,
            };
            if privacy.is_some() && auth.is_none() {
                return Err(AppError::invalid(format!(
                    "the credential of `{device}` has SNMPv3 privacy without authentication, which SNMP does not allow"
                )));
            }
            Ok(Auth::V3 {
                user: access.user.trim().to_string(),
                auth,
                privacy,
                context_name: String::new(),
            })
        }
        other => Err(AppError::invalid(format!(
            "`{other}` is not an SNMP version Nocturne speaks (v1, v2c or v3)"
        ))),
    }
}

fn auth_protocol(name: &str) -> Option<AuthProtocol> {
    match name.to_ascii_lowercase().as_str() {
        "md5" => Some(AuthProtocol::Md5),
        "sha1" | "sha" => Some(AuthProtocol::Sha1),
        "sha224" => Some(AuthProtocol::Sha224),
        "sha256" => Some(AuthProtocol::Sha256),
        "sha384" => Some(AuthProtocol::Sha384),
        "sha512" => Some(AuthProtocol::Sha512),
        _ => None,
    }
}

fn priv_protocol(name: &str) -> Option<PrivProtocol> {
    match name.to_ascii_lowercase().as_str() {
        "des" => Some(PrivProtocol::Des),
        "aes" | "aes128" => Some(PrivProtocol::Aes128),
        "aes192" => Some(PrivProtocol::Aes192),
        "aes256" => Some(PrivProtocol::Aes256),
        "aes192c" | "aes192-cisco" => Some(PrivProtocol::Aes192Cisco),
        "aes256c" | "aes256-cisco" => Some(PrivProtocol::Aes256Cisco),
        _ => None,
    }
}

/// `10.0.0.1` / `[2001:db8::1]` / a bare IPv6 literal with a port.
fn resolve_address(address: &str, port: u16) -> Option<SocketAddr> {
    let address = address.trim();
    if let Ok(socket) = address.parse::<SocketAddr>() {
        return Some(socket);
    }
    if let Ok(ip) = address.parse::<std::net::IpAddr>() {
        return Some(SocketAddr::new(ip, port));
    }
    // A host name: resolving it here would block the poller, so names are
    // resolved by the standard library on a blocking-free path only when they
    // are literal. Anything else is reported rather than guessed at.
    None
}

/// Exclusive use of a device's session for as long as it is held.
pub struct Lease {
    guard: tokio::sync::OwnedMutexGuard<Entry>,
}

impl Lease {
    pub fn session(&mut self) -> &mut dyn SnmpSession {
        self.guard
            .session
            .as_mut()
            .expect("a lease always holds an open session")
            .as_mut()
    }

    /// Throw the session away; the next poll opens a fresh one. Called after
    /// anything that suggests the agent or the credentials moved on.
    pub fn discard(&mut self) {
        self.guard.session = None;
    }
}

/// Take a device's session, opening one if it has none or if the credentials
/// it was opened with have changed.
pub async fn acquire(
    sessions: &Sessions,
    transport: &SharedSnmp,
    device_id: &str,
    target: &Target,
    timeout: Duration,
) -> Result<Lease, String> {
    let mut guard = sessions.slot(device_id).lock_owned().await;
    if guard.fingerprint != target.fingerprint {
        guard.session = None;
        guard.fingerprint = target.fingerprint.clone();
    }
    if guard.session.is_none() {
        guard.session = Some(
            transport
                .open(target.address, target.auth.clone(), timeout)
                .await?,
        );
    }
    Ok(Lease { guard })
}

/// Resolve a name from `nocturne_snmp::oids` once, loudly if it is wrong.
pub fn oid(constant: &str) -> Oid {
    constant
        .parse()
        .expect("the OID constants of nocturne-snmp parse")
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_app::model::MonitorSpec;
    use serde_json::json;

    fn access(version: &str) -> SnmpAccess {
        SnmpAccess {
            version: version.into(),
            ..SnmpAccess::default()
        }
    }

    #[test]
    fn literal_addresses_resolve_and_names_are_refused_rather_than_guessed() {
        assert_eq!(
            resolve_address("10.0.0.1", 161),
            Some("10.0.0.1:161".parse().unwrap())
        );
        assert_eq!(
            resolve_address("127.0.0.1:1161", 161),
            Some("127.0.0.1:1161".parse().unwrap())
        );
        assert_eq!(
            resolve_address("2001:db8::1", 161),
            Some("[2001:db8::1]:161".parse().unwrap())
        );
        assert_eq!(resolve_address("switch.example", 161), None);
    }

    #[test]
    fn building_v3_authentication_says_what_is_missing() {
        let empty = credentials::Resolved::default();
        let error = build_auth(
            "SW1",
            &SnmpAccess {
                version: "v3".into(),
                user: "noc".into(),
                auth_protocol: "sha256".into(),
                ..SnmpAccess::default()
            },
            &empty,
        )
        .unwrap_err();
        assert_eq!(error.code, "unauthorized");
        assert!(error.message.contains("authentication password"), "{error}");

        let error = build_auth("SW1", &access("v3"), &empty).unwrap_err();
        assert!(error.message.contains("no user name"), "{error}");

        let error = build_auth("SW1", &access("v2c"), &empty).unwrap_err();
        assert!(error.message.contains("no SNMP community"), "{error}");

        let error = build_auth("SW1", &access("v9"), &empty).unwrap_err();
        assert!(error.message.contains("not an SNMP version"), "{error}");
    }

    #[test]
    fn noauthnopriv_v3_needs_no_secrets() {
        let auth = build_auth(
            "SW1",
            &SnmpAccess {
                version: "v3".into(),
                user: "noc".into(),
                ..SnmpAccess::default()
            },
            &credentials::Resolved::default(),
        )
        .unwrap();
        assert!(matches!(
            auth,
            Auth::V3 {
                auth: None,
                privacy: None,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn a_device_without_a_credential_says_so() {
        let builder = nocturne_app::testing::builder();
        let app = builder.build();
        let device = Device {
            name: "SW9".into(),
            address: "10.0.0.9".into(),
            monitor: MonitorSpec {
                snmp: true,
                ..MonitorSpec::default()
            },
            ..Device::default()
        };
        let error = target(&app, &device).unwrap_err();
        assert_eq!(error.code, "invalid");
        assert!(error.message.contains("no credential"), "{error}");
    }

    #[tokio::test]
    async fn the_fingerprint_changes_with_the_credential_but_holds_no_secret() {
        let lab = nocturne_app::testing::lab(nocturne_app::testing::builder(), &["SW1"]).await;
        lab.app
            .rpc(
                "credentials.update",
                json!({"id": lab.credential_id, "snmp_version": "v2c", "snmp_community": "s3cret"}),
            )
            .await
            .unwrap();
        let mut device = nocturne_app::inventory::get(&lab.app, &lab.id("SW1")).unwrap();
        device.address = "10.0.0.5".into();
        device.monitor.snmp = true;
        let target = target(&lab.app, &device).unwrap();
        assert!(
            !target.fingerprint.contains("s3cret"),
            "{}",
            target.fingerprint
        );
        assert!(target.fingerprint.contains("10.0.0.5:161"));
        assert!(matches!(target.auth, Auth::V2c { .. }));
    }
}
