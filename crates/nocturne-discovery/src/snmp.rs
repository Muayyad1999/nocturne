//! Turning the credentials a user picked for a scan into SNMP authentication.
//!
//! Only the credentials named in the request are ever used, they are resolved
//! once at the start of a scan (so a locked vault fails before a single
//! packet goes out), and the secrets stay inside [`Credential::auth`] —
//! `Auth`'s own `Debug` masks communities and passphrases, so a scan can be
//! traced without leaking one.

use nocturne_app::model::{Credential as CredentialRecord, SnmpAccess, collections::CREDENTIALS};
use nocturne_app::{App, AppError, AppResult, credentials};
use nocturne_snmp::{Auth, AuthProtocol, PrivProtocol};

/// One SNMP credential a scan may try, ready to use.
pub struct Credential {
    pub id: String,
    pub name: String,
    pub auth: Auth,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("auth", &self.auth)
            .finish()
    }
}

/// Resolve the credentials a scan was given, in the order it gave them.
///
/// A credential that holds no SNMP access at all is an error rather than a
/// silent skip: the user chose it, and a scan that quietly ignored it would
/// report "no SNMP" for a network that answers perfectly well.
pub fn resolve_all(app: &App, ids: &[String]) -> AppResult<Vec<Credential>> {
    let mut resolved = Vec::new();
    for id in ids {
        if resolved.iter().any(|known: &Credential| &known.id == id) {
            continue;
        }
        let record: CredentialRecord = app.store().require(CREDENTIALS, id, "credential")?;
        let secrets = credentials::resolve(app, id)?;
        let auth = build_auth(&record.name, &record.snmp, &secrets)?;
        resolved.push(Credential {
            id: id.clone(),
            name: record.name,
            auth,
        });
    }
    Ok(resolved)
}

fn build_auth(name: &str, access: &SnmpAccess, secrets: &credentials::Resolved) -> AppResult<Auth> {
    let community = || {
        secrets
            .snmp_community
            .as_ref()
            .map(|secret| secret.to_string())
            .ok_or_else(|| {
                AppError::invalid(format!(
                    "the credential `{name}` has no SNMP community, so a scan cannot use it for SNMP. Add one to the credential, or leave it out of the scan"
                ))
            })
    };
    match access.version.as_str() {
        "" => Err(AppError::invalid(format!(
            "the credential `{name}` has no SNMP access configured (no version, community or v3 user)"
        ))),
        "v1" => Ok(Auth::V1 {
            community: community()?,
        }),
        "v2c" => Ok(Auth::V2c {
            community: community()?,
        }),
        "v3" => {
            if access.user.trim().is_empty() {
                return Err(AppError::invalid(format!(
                    "the credential `{name}` selects SNMPv3 but has no user name"
                )));
            }
            let auth = match (
                auth_protocol(&access.auth_protocol),
                &secrets.snmp_auth_password,
            ) {
                (Some(protocol), Some(password)) => Some((protocol, password.to_string())),
                (Some(_), None) => {
                    return Err(AppError::invalid(format!(
                        "the credential `{name}` selects SNMPv3 authentication but has no authentication password"
                    )));
                }
                (None, _) => None,
            };
            let privacy = match (
                priv_protocol(&access.priv_protocol),
                &secrets.snmp_priv_password,
            ) {
                (Some(protocol), Some(password)) => Some((protocol, password.to_string())),
                (Some(_), None) => {
                    return Err(AppError::invalid(format!(
                        "the credential `{name}` selects SNMPv3 privacy but has no privacy password"
                    )));
                }
                (None, _) => None,
            };
            if privacy.is_some() && auth.is_none() {
                return Err(AppError::invalid(format!(
                    "the credential `{name}` has SNMPv3 privacy without authentication, which SNMP does not allow"
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
        "aes128" | "aes" => Some(PrivProtocol::Aes128),
        "aes192" => Some(PrivProtocol::Aes192),
        "aes256" => Some(PrivProtocol::Aes256),
        _ => None,
    }
}
