//! Turning the credentials a user picked for a scan into SNMP authentication.
//!
//! Only the credentials named in the request are ever used, they are resolved
//! once at the start of a scan (so a locked vault fails before a single
//! packet goes out), and the secrets stay inside [`Credential::auth`] —
//! `Auth`'s own `Debug` masks communities and passphrases, so a scan can be
//! traced without leaking one.

use nocturne_app::model::{Credential as CredentialRecord, SnmpAccess, collections::CREDENTIALS};
use nocturne_app::{App, AppError, AppResult, credentials};
use nocturne_snmp::{Auth, AuthParts, AuthProblem};

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
    // A credential with nothing SNMP about it is a mistake in the *request*
    // here, not a default: the engine reads an empty version as v2c, which is
    // right when monitoring a device somebody chose, and wrong when a scan was
    // handed a credential that has no SNMP in it at all.
    if access.version.trim().is_empty() {
        return Err(AppError::invalid(format!(
            "the credential `{name}` has no SNMP access configured (no version, community or v3 user)"
        )));
    }
    // Everything after that is SNMP's own rules, and lives in the engine.
    let community = secrets.snmp_community.as_ref().map(|s| s.to_string());
    let auth_password = secrets.snmp_auth_password.as_ref().map(|s| s.to_string());
    let priv_password = secrets.snmp_priv_password.as_ref().map(|s| s.to_string());
    Auth::from_parts(AuthParts {
        version: &access.version,
        user: &access.user,
        auth_protocol: &access.auth_protocol,
        priv_protocol: &access.priv_protocol,
        community: community.as_deref(),
        auth_password: auth_password.as_deref(),
        priv_password: priv_password.as_deref(),
        context_name: "",
    })
    .map_err(|problem| match problem {
        AuthProblem::NoCommunity => AppError::invalid(format!(
            "the credential `{name}` has no SNMP community, so a scan cannot use it for SNMP. Add one to the credential, or leave it out of the scan"
        )),
        other => AppError::invalid(format!("the credential `{name}`: {other}")),
    })
}
