//! SSH parameters for a terminal session.
//!
//! The rules — which authentication methods to try and in what order, how to
//! follow a jump-host chain, which connection settings apply, which host keys
//! are trusted — belong to the core, so that the terminal, the collectors and
//! the change pipeline cannot drift apart on any of them. This module only
//! says what a terminal session wants on top: its terminal type and size.

use nocturne_app::connect;
use nocturne_app::model::Device;
use nocturne_app::{App, AppResult};
use nocturne_transport::SshParams;

/// Where an SSH connection goes and as whom.
pub(crate) use nocturne_app::connect::SshEndpoint as Endpoint;
/// How to authenticate: a password typed for this session, then the stored credential.
pub(crate) use nocturne_app::connect::ssh_auth_methods as auth_methods;

/// Parameters for an SSH connection to `endpoint` with a terminal of `cols` × `rows`.
pub(crate) fn params(
    app: &App,
    endpoint: Endpoint,
    size: (u16, u16),
    keepalive: bool,
) -> AppResult<SshParams> {
    connect::ssh_params(app, endpoint, crate::TERM, size, keepalive)
}

/// Parameters for an SSH connection to an inventory device, as every other
/// part of Nocturne would make it.
pub(crate) fn device_params(app: &App, device: &Device) -> AppResult<SshParams> {
    // SFTP and one-shot channels need no pseudo-terminal, hence the token size.
    params(app, connect::device_endpoint(app, device)?, (80, 24), true)
}
