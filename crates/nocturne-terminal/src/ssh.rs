//! SSH parameters for the connections this feature opens itself: ad-hoc
//! terminal sessions and SFTP.
//!
//! Terminal sessions to inventory devices go through the core's
//! `connect::open`. It has no counterpart that returns an `SshSession` (which
//! SFTP needs) or takes a target outside the inventory, so the same rules are
//! applied here: stored credential → authentication methods, jump devices →
//! `ProxyJump` hops, the `connections` settings, the shared known-hosts store.

use nocturne_app::connect::{self, ConnectionSettings};
use nocturne_app::credentials::{self, Resolved};
use nocturne_app::model::{Device, Transport};
use nocturne_app::{App, AppError, AppResult, inventory};
use nocturne_transport::{LegacyCrypto, SshAuth, SshHop, SshParams};

/// Most hops a jump chain may have (the transport's limit).
const MAX_JUMP_HOSTS: usize = 5;

/// Where an SSH connection goes and as whom.
pub(crate) struct Endpoint {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: Vec<SshAuth>,
    /// The device to go through, if any; its own jump host comes before it.
    pub jump_device_id: Option<String>,
}

/// How to authenticate: a password typed for this session first, then what a
/// stored credential holds, most specific first.
pub(crate) fn auth_methods(password: Option<&str>, credential: Option<&Resolved>) -> Vec<SshAuth> {
    let mut auth = Vec::new();
    let mut add_password = |password: &str| {
        auth.push(SshAuth::Password {
            password: password.to_string(),
        });
        // Network devices often offer only keyboard-interactive and ask for the same password.
        auth.push(SshAuth::KeyboardInteractive {
            response: password.to_string(),
        });
    };
    if let Some(password) = password {
        add_password(password);
    }
    let stored_password = credential.and_then(|credential| credential.password.as_ref());
    if let Some(stored) = stored_password
        && password != Some(stored.as_str())
    {
        add_password(stored);
    }
    if let Some(key) = credential.and_then(|credential| credential.private_key.as_ref()) {
        let passphrase = credential
            .and_then(|credential| credential.passphrase.as_ref())
            .map(|passphrase| passphrase.to_string());
        // A key is tried before any password: it cannot lock an account.
        auth.insert(
            0,
            SshAuth::PrivateKey {
                key: key.to_string(),
                passphrase,
            },
        );
    }
    if auth.is_empty() {
        // No secret at hand: the user's agent may hold a key, and lab gear often needs none.
        auth.push(SshAuth::Agent);
        auth.push(SshAuth::None);
    }
    auth
}

fn tcp_port(device: &Device, default: u16) -> AppResult<u16> {
    match device.port {
        None => Ok(default),
        Some(port) => u16::try_from(port)
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| {
                AppError::invalid(format!(
                    "{port} is not a valid TCP port for `{}`",
                    device.name
                ))
            }),
    }
}

fn resolved_credential(app: &App, device: &Device) -> AppResult<Resolved> {
    match &device.credential_id {
        Some(id) => credentials::resolve(app, id),
        None => Ok(Resolved::default()),
    }
}

/// The hops to go through to use `first` as a jump host, outermost first.
fn jump_chain(app: &App, first: Option<&str>, target: &str) -> AppResult<Vec<SshHop>> {
    let mut hops = Vec::new();
    let mut next = first.map(str::to_string);
    while let Some(id) = next {
        if hops.len() >= MAX_JUMP_HOSTS {
            return Err(AppError::invalid(format!(
                "the jump-host chain of {target} is more than {MAX_JUMP_HOSTS} hops long"
            )));
        }
        let hop = inventory::find(app, &id).map_err(|_| {
            AppError::invalid(format!(
                "the jump host of {target} (`{id}`) is not in the inventory; pick another"
            ))
        })?;
        if hop.transport != Transport::Ssh {
            return Err(AppError::invalid(format!(
                "`{}` cannot be a jump host: it is not reached over SSH",
                hop.name
            )));
        }
        let credential = resolved_credential(app, &hop)?;
        if credential.username.is_empty() {
            return Err(AppError::unauthorized(format!(
                "the jump host `{}` has no credential with a user name",
                hop.name
            )));
        }
        hops.push(SshHop {
            host: hop.address.clone(),
            port: tcp_port(&hop, 22)?,
            username: credential.username.clone(),
            auth: auth_methods(None, Some(&credential)),
        });
        next = hop.jump_device_id.clone();
    }
    hops.reverse();
    Ok(hops)
}

fn legacy(settings: &ConnectionSettings) -> LegacyCrypto {
    match settings.legacy_crypto.as_str() {
        "never" => LegacyCrypto::Never,
        "always" => LegacyCrypto::Always,
        _ => LegacyCrypto::Auto,
    }
}

/// Parameters for an SSH connection to `endpoint` with a terminal of `cols` × `rows`.
pub(crate) fn params(
    app: &App,
    endpoint: Endpoint,
    (cols, rows): (u16, u16),
    keepalive: bool,
) -> AppResult<SshParams> {
    let settings = connect::connection_settings(app);
    let target = format!("{}@{}", endpoint.username, endpoint.host);
    Ok(SshParams {
        jump: jump_chain(app, endpoint.jump_device_id.as_deref(), &target)?,
        host: endpoint.host,
        port: endpoint.port,
        username: endpoint.username,
        auth: endpoint.auth,
        term: crate::TERM.to_string(),
        cols,
        rows,
        connect_timeout_ms: settings.connect_timeout_s.clamp(1, 120) * 1000,
        keepalive_s: if keepalive { settings.keepalive_s } else { 0 },
        legacy: legacy(&settings),
    })
}

/// Parameters for an SSH connection to an inventory device, as the core
/// would make it.
pub(crate) fn device_params(app: &App, device: &Device) -> AppResult<SshParams> {
    let credential = resolved_credential(app, device)?;
    if credential.username.is_empty() {
        return Err(AppError::unauthorized(format!(
            "`{}` has no credential with a user name; SSH needs one. Assign a credential to the device.",
            device.name
        )));
    }
    let endpoint = Endpoint {
        host: device.address.clone(),
        port: tcp_port(device, 22)?,
        auth: auth_methods(None, Some(&credential)),
        username: credential.username,
        jump_device_id: device.jump_device_id.clone(),
    };
    params(app, endpoint, (80, 24), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeroize::Zeroizing;

    fn kinds(auth: &[SshAuth]) -> Vec<&'static str> {
        auth.iter()
            .map(|method| match method {
                SshAuth::Password { .. } => "password",
                SshAuth::PrivateKey { .. } => "key",
                SshAuth::KeyFile { .. } => "key_file",
                SshAuth::KeyboardInteractive { .. } => "interactive",
                SshAuth::Agent => "agent",
                SshAuth::None => "none",
            })
            .collect()
    }

    #[test]
    fn authentication_prefers_keys_then_the_typed_password() {
        assert_eq!(kinds(&auth_methods(None, None)), ["agent", "none"]);
        assert_eq!(
            kinds(&auth_methods(Some("typed"), None)),
            ["password", "interactive"]
        );

        let stored = Resolved {
            username: "admin".into(),
            password: Some(Zeroizing::new("stored".into())),
            private_key: Some(Zeroizing::new("KEY".into())),
            passphrase: Some(Zeroizing::new("phrase".into())),
            ..Resolved::default()
        };
        let auth = auth_methods(Some("typed"), Some(&stored));
        assert_eq!(
            kinds(&auth),
            ["key", "password", "interactive", "password", "interactive"]
        );
        assert!(
            matches!(&auth[0], SshAuth::PrivateKey { key, passphrase } if key == "KEY" && passphrase.as_deref() == Some("phrase"))
        );
        assert!(matches!(&auth[1], SshAuth::Password { password } if password == "typed"));
        assert!(matches!(&auth[3], SshAuth::Password { password } if password == "stored"));

        // The same password twice would only burn a login attempt.
        assert_eq!(
            kinds(&auth_methods(Some("stored"), Some(&stored))),
            ["key", "password", "interactive"]
        );
    }
}
