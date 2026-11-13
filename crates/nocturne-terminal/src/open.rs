//! `terminal.open`: turning a device or an ad-hoc target into a live session.
//!
//! Sessions to inventory devices go through the core's `connect::open`, so a
//! terminal tab reaches a device exactly the way automation does — same
//! credential, same jump hosts, same host-key policy. Ad-hoc targets are
//! built here from what the caller typed, with the same known-hosts store.
//!
//! Nothing is published and nothing is audited until the connection stands:
//! `terminal.open` returns once there is a session to type into, or an error
//! carrying the transport's own code (`auth`, `host_key`, `refused`, …).

use nocturne_app::connect::{self, OpenOptions};
use nocturne_app::credentials::{self, Resolved};
use nocturne_app::model::{Device, Transport};
use nocturne_app::{App, AppError, AppResult, Router, inventory};
use nocturne_drivers::profile::Cli;
use nocturne_transport::{
    Connection, LocalParams, SerialParams, SshParams, TelnetParams, TransportKind, connect_local,
    connect_serial, connect_ssh, connect_telnet,
};
use serde::Deserialize;
use serde_json::json;
use tokio::time::Instant;
use zeroize::Zeroizing;

use crate::autologin::{AutoLogin, Credentials};
use crate::hostkey;
use crate::model::{Adhoc, AdhocTransport, SessionInfo, kind_name};
use crate::platform;
use crate::session::{Opening, TerminalState};
use crate::settings::TerminalSettings;
use crate::ssh::{self as sshparams, Endpoint};

/// Terminal sizes Nocturne will ask a device for. A device told it has three
/// columns wraps everything; one told it has 10 000 pads its output.
const COLS: std::ops::RangeInclusive<u16> = 20..=500;
const ROWS: std::ops::RangeInclusive<u16> = 5..=200;
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 40;

#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct OpenParams {
    /// A device in the inventory (id, or its name or address).
    device_id: Option<String>,
    /// A target that is not in the inventory.
    adhoc: Option<Adhoc>,
    cols: Option<u16>,
    rows: Option<u16>,
    /// Write a session log; defaults to the `terminal.log_sessions` setting.
    log: Option<bool>,
}

/// Everything needed to make the connection, worked out before anything is
/// registered or announced.
struct Plan {
    opening: Opening,
    dial: Dial,
    /// Login answers for a transport that asks for them in the data stream.
    login: Option<(Cli, Credentials)>,
}

enum Dial {
    /// An inventory device, through the core.
    Device(Box<Device>, OpenOptions),
    Ssh(Box<SshParams>),
    Telnet(Box<TelnetParams>),
    Serial(Box<SerialParams>),
    Local(Box<LocalParams>),
}

fn size(params: &OpenParams) -> (u16, u16) {
    (
        params
            .cols
            .unwrap_or(DEFAULT_COLS)
            .clamp(*COLS.start(), *COLS.end()),
        params
            .rows
            .unwrap_or(DEFAULT_ROWS)
            .clamp(*ROWS.start(), *ROWS.end()),
    )
}

fn kind_of(transport: Transport) -> Option<TransportKind> {
    match transport {
        Transport::Ssh => Some(TransportKind::Ssh),
        Transport::Telnet => Some(TransportKind::Telnet),
        Transport::Serial => Some(TransportKind::Serial),
        Transport::Sim => Some(TransportKind::Sim),
        Transport::None => None,
    }
}

/// `host:port`, or just the host when the port is the transport's default.
fn endpoint(host: &str, port: u16, default: u16) -> String {
    if port == default {
        host.to_string()
    } else if host.contains(':') {
        // An IPv6 literal needs brackets before a port can be appended.
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// The login answers to use, if there are any worth having.
fn credentials(username: &str, password: Option<Zeroizing<String>>) -> Option<Credentials> {
    let username = (!username.trim().is_empty()).then(|| username.trim().to_string());
    (username.is_some() || password.is_some()).then_some(Credentials { username, password })
}

/// What a session to an inventory device needs.
fn plan_device(app: &App, reference: &str, (cols, rows): (u16, u16)) -> AppResult<Plan> {
    let device = inventory::find(app, reference)?;
    let Some(kind) = kind_of(device.transport) else {
        return Err(AppError::invalid(format!(
            "`{}` is monitor-only (no CLI access configured). Set its connection method to SSH, Telnet or serial first.",
            device.name
        )));
    };
    if kind == TransportKind::Serial {
        app.require_local_access("a serial console")?;
    }
    let peer = match device.transport {
        Transport::Ssh => endpoint(&device.address, device.port.unwrap_or(22) as u16, 22),
        Transport::Telnet => endpoint(&device.address, device.port.unwrap_or(23) as u16, 23),
        Transport::Sim => device
            .lab
            .as_ref()
            .map(|lab| format!("lab:{}", lab.node))
            .unwrap_or_else(|| device.name.clone()),
        _ => device.address.clone(),
    };
    // The profile says what this platform's login prompts look like; without
    // one there is nothing to recognise and the session is simply manual.
    let login = match platform::of_device(app, &device)? {
        Some(platform) if answers_in_band(kind) => {
            let resolved = match &device.credential_id {
                Some(id) => credentials::resolve(app, id)?,
                None => Resolved::default(),
            };
            credentials(&resolved.username, resolved.password)
                .map(|credentials| (platform.profile.cli.clone(), credentials))
        }
        _ => None,
    };
    Ok(Plan {
        opening: Opening {
            title: device.name.clone(),
            kind,
            device_id: Some(device.id.clone()),
            lab: device.lab.is_some(),
            peer,
            cols,
            rows,
        },
        dial: Dial::Device(
            Box::new(device),
            OpenOptions {
                cols,
                rows,
                term: crate::TERM.to_string(),
            },
        ),
        login,
    })
}

/// Whether the transport expects the login to be typed into the data stream.
/// SSH authenticates in the protocol and a local shell has no login at all.
fn answers_in_band(kind: TransportKind) -> bool {
    matches!(
        kind,
        TransportKind::Telnet | TransportKind::Serial | TransportKind::Sim
    )
}

fn plan_adhoc(app: &App, adhoc: Adhoc, (cols, rows): (u16, u16)) -> AppResult<Plan> {
    let settings = connect::connection_settings(app);
    let timeout_ms = settings.connect_timeout_s.clamp(1, 120) * 1000;
    let stored = match &adhoc.credential_id {
        Some(id) => Some(credentials::resolve(app, id)?),
        None => None,
    };
    let username = match adhoc.username.trim() {
        "" => stored
            .as_ref()
            .map(|stored| stored.username.clone())
            .unwrap_or_default(),
        typed => typed.to_string(),
    };
    // A password typed for this session wins over a stored one, and is the
    // only copy that exists: it is never written anywhere.
    let password = adhoc
        .password
        .as_ref()
        .map(|typed| Zeroizing::new(typed.as_str().to_string()))
        .or_else(|| stored.as_ref().and_then(|stored| stored.password.clone()));

    let host = adhoc.host.trim().to_string();
    let kind = match adhoc.transport {
        AdhocTransport::Ssh => TransportKind::Ssh,
        AdhocTransport::Telnet => TransportKind::Telnet,
        AdhocTransport::Serial => TransportKind::Serial,
        AdhocTransport::Local => TransportKind::Local,
    };
    if host.is_empty() && kind != TransportKind::Local {
        return Err(AppError::invalid(match kind {
            TransportKind::Serial => {
                "no serial port named. Pass the port name (`COM3`, `/dev/ttyUSB0`) as `adhoc.host`; `terminal.serial_ports` lists them."
            }
            _ => "no host named. Pass the host name or address as `adhoc.host`.",
        }));
    }
    let port = adhoc.port.filter(|port| *port != 0);

    let (peer, dial) = match adhoc.transport {
        AdhocTransport::Ssh => {
            if username.is_empty() {
                return Err(AppError::invalid(
                    "an SSH session needs a user name: pass `adhoc.username`, or `adhoc.credential_id` of a credential that has one",
                ));
            }
            let port = port.unwrap_or(22);
            let params = sshparams::params(
                app,
                Endpoint {
                    host: host.clone(),
                    port,
                    username: username.clone(),
                    auth: sshparams::auth_methods(
                        password.as_ref().map(|password| password.as_str()),
                        stored.as_ref(),
                    ),
                    jump_device_id: adhoc.jump_device_id.clone(),
                },
                (cols, rows),
                TerminalSettings::load(app).keepalive,
            )?;
            (endpoint(&host, port, 22), Dial::Ssh(Box::new(params)))
        }
        AdhocTransport::Telnet => {
            let port = port.unwrap_or(23);
            if adhoc.jump_device_id.is_some() {
                return Err(AppError::invalid(
                    "Telnet cannot go through a jump host; open a terminal on the jump host and telnet from there",
                ));
            }
            (
                endpoint(&host, port, 23),
                Dial::Telnet(Box::new(TelnetParams {
                    host: host.clone(),
                    port,
                    term: crate::TERM.to_string(),
                    cols,
                    rows,
                    connect_timeout_ms: timeout_ms,
                })),
            )
        }
        AdhocTransport::Serial => {
            app.require_local_access("a serial console")?;
            let serial = &adhoc.serial;
            (
                host.clone(),
                Dial::Serial(Box::new(SerialParams {
                    port: host.clone(),
                    baud: port.map_or(serial.baud, u32::from),
                    data_bits: serial.data_bits,
                    parity: serial.parity,
                    stop_bits: serial.stop_bits,
                    flow: serial.flow,
                })),
            )
        }
        AdhocTransport::Local => {
            app.require_local_access("the local shell")?;
            let local = &adhoc.local;
            (
                local.program.clone().unwrap_or_else(|| "shell".to_string()),
                Dial::Local(Box::new(LocalParams {
                    program: local.program.clone(),
                    args: local.args.clone(),
                    cwd: local.cwd.clone(),
                    cols,
                    rows,
                    env: Vec::new(),
                })),
            )
        }
    };

    let title = adhoc
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| match adhoc.transport {
            AdhocTransport::Ssh => format!("{username}@{peer}"),
            AdhocTransport::Telnet | AdhocTransport::Serial => peer.clone(),
            AdhocTransport::Local => "Local shell".to_string(),
        });

    // Nothing is known about an ad-hoc target, so the generic profile's
    // prompts are what auto-login has to go on.
    let login = match platform::generic(app) {
        Some(platform) if answers_in_band(kind) => credentials(&username, password)
            .map(|credentials| (platform.profile.cli.clone(), credentials)),
        _ => None,
    };

    Ok(Plan {
        opening: Opening {
            title,
            kind,
            device_id: None,
            lab: false,
            peer,
            cols,
            rows,
        },
        dial,
        login,
    })
}

async fn dial(app: &App, what: Dial) -> AppResult<Connection> {
    match what {
        Dial::Device(device, options) => connect::open(app, &device, &options).await,
        Dial::Ssh(params) => Ok(connect_ssh(&params, connect::verifier(app)).await?),
        Dial::Telnet(params) => Ok(connect_telnet(&params).await?),
        Dial::Serial(params) => Ok(connect_serial(&params).await?),
        Dial::Local(params) => Ok(connect_local(&params).await?),
    }
}

async fn open(app: &App, params: OpenParams) -> AppResult<SessionInfo> {
    let size = size(&params);
    let plan = match (params.device_id.as_deref(), params.adhoc) {
        (Some(device_id), None) => plan_device(app, device_id, size)?,
        (None, Some(adhoc)) => plan_adhoc(app, adhoc, size)?,
        (None, None) => {
            return Err(AppError::invalid(
                "terminal.open needs a target: either `device_id` for a device in the inventory, or `adhoc` for anything else",
            ));
        }
        (Some(_), Some(_)) => {
            return Err(AppError::invalid(
                "terminal.open takes either `device_id` or `adhoc`, not both",
            ));
        }
    };
    let settings = TerminalSettings::load(app);
    let reservation = TerminalState::reserve(app, plan.opening, settings.scrollback_bytes())?;
    let session = reservation.session().clone();

    // A session closed while it was still connecting must not leave the
    // attempt running: `terminal.close` cancels the token this waits on.
    let connection = tokio::select! {
        () = session.stopped() => {
            return Err(AppError::new(
                "closed",
                "the terminal session was closed while it was connecting",
            ));
        }
        connection = hostkey::with_details(app, dial(app, plan.dial)) => connection?,
    };

    if params.log.unwrap_or(settings.log_sessions) {
        session.start_log(app).await;
    }
    let auto_login = plan.login.filter(|_| settings.auto_login).and_then(
        |(cli, credentials)| -> Option<AutoLogin> {
            AutoLogin::new(&cli, credentials, Instant::now())
        },
    );
    let info = session.start(app, connection, auto_login);
    let session = reservation.keep();

    app.publish("terminal.opened", &info);
    app.audit(
        "terminal.open",
        info.device_id.as_deref(),
        format!("Opened a terminal session to {}", info.title),
        json!({
            "session_id": session.id,
            "kind": kind_name(info.kind),
            "peer": info.info.peer,
            "lab": info.lab,
            "logging": info.logging,
        }),
    );
    Ok(info)
}

pub(crate) fn register(router: &mut Router) {
    router.add("terminal.open", |app: App, params: OpenParams| async move {
        open(&app, params).await
    });
}
