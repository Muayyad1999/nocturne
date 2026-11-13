//! User authentication: the configured methods are tried in order, guided by
//! what the server says it accepts.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use russh::client::{AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey};
use russh::{MethodKind, MethodSet};

use super::SshAuth;
use super::algorithms::describe_sha1_signature_refused;
use super::client::{ClientHandler, Shared};
use super::keys::decode_private_key;
use crate::error::TransportError;

/// A server that keeps asking after this many rounds is not going to let us in.
const MAX_KEYBOARD_ROUNDS: usize = 8;
/// Offering more keys than this risks "too many authentication failures".
const MAX_AGENT_KEYS: usize = 6;
const AGENT_TIMEOUT: Duration = Duration::from_secs(5);

/// What a successful authentication had to resort to.
#[derive(Debug, Default)]
pub(crate) struct AuthReport {
    /// A SHA-1 RSA signature (`ssh-rsa`) was needed to log in.
    pub legacy_signature: bool,
}

/// Why an authentication step cannot continue.
enum Halt {
    Timeout,
    /// The connection went away (possibly closed by the server).
    Lost,
    Other(String),
}

type Step = Result<bool, Halt>;

struct Authenticator<'a> {
    handle: &'a mut Handle<ClientHandler>,
    username: &'a str,
    /// Whether SHA-1 RSA signatures may be used (legacy crypto allowed).
    allow_sha1: bool,
    step_timeout: Duration,
    tried: Vec<String>,
    /// `None` until the server told us.
    server_methods: Option<MethodSet>,
    /// RSA signature hashes worth trying, most promising first (cached).
    rsa_hashes: Option<Vec<Option<HashAlg>>>,
    used_sha1: bool,
    /// A key could only have been used with a SHA-1 signature, which the
    /// policy forbids.
    sha1_refused: bool,
}

/// Runs the configured methods in order until the server lets us in.
pub(crate) async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    shared: &Shared,
    username: &str,
    methods: &[SshAuth],
    allow_sha1: bool,
    step_timeout: Duration,
) -> Result<AuthReport, TransportError> {
    let mut auth = Authenticator {
        handle,
        username,
        allow_sha1,
        step_timeout,
        tried: Vec::new(),
        server_methods: None,
        rsa_hashes: None,
        used_sha1: false,
        sha1_refused: false,
    };
    match auth.run(methods).await {
        Ok(true) => Ok(AuthReport {
            legacy_signature: auth.used_sha1,
        }),
        Ok(false) => Err(auth.failure(None)),
        Err(Halt::Timeout) => Err(TransportError::Timeout),
        Err(Halt::Other(message)) => Err(TransportError::Protocol(message)),
        Err(Halt::Lost) => {
            let cause = shared
                .disconnect_reason()
                .unwrap_or_else(|| "the connection was lost".to_owned());
            Err(auth.failure(Some(cause)))
        }
    }
}

impl Authenticator<'_> {
    async fn run(&mut self, methods: &[SshAuth]) -> Step {
        // The "none" request is how a client learns what the server accepts
        // (and gets in when it wants nothing at all).
        if self.none().await? {
            return Ok(true);
        }
        for method in methods {
            let authenticated = match method {
                SshAuth::None => false, // already tried above
                SshAuth::Password { password } => self.password(password).await?,
                SshAuth::KeyboardInteractive { response } => {
                    self.keyboard_interactive(response, "keyboard-interactive")
                        .await?
                }
                SshAuth::PrivateKey { key, passphrase } => {
                    self.private_key(key, passphrase.as_deref(), "private key")
                        .await?
                }
                SshAuth::KeyFile { path, passphrase } => {
                    self.key_file(path, passphrase.as_deref()).await?
                }
                SshAuth::Agent => self.agent().await?,
            };
            if authenticated {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn failure(self, cause: Option<String>) -> TransportError {
        let mut tried = self.tried;
        if let Some(cause) = cause {
            tried.push(format!("({cause})"));
        }
        let server_methods: Vec<String> = self
            .server_methods
            .iter()
            .flat_map(|methods| methods.iter())
            .map(|method| <&str>::from(method).to_owned())
            .collect();
        // The server would have taken our key, had we been allowed to sign
        // with SHA-1: that is an algorithm problem, not a credentials one.
        if self.sha1_refused {
            return TransportError::NoCommonAlgorithm(describe_sha1_signature_refused());
        }
        TransportError::AuthFailed {
            tried,
            server_methods,
        }
    }

    fn offers(&self, method: MethodKind) -> bool {
        self.server_methods
            .as_ref()
            .is_none_or(|methods| methods.contains(&method))
    }

    fn skip(&mut self, label: &str, why: &str) {
        self.tried.push(format!("{label} (skipped: {why})"));
    }

    /// Books the outcome of an attempt; `true` when we are in.
    fn record(&mut self, label: String, result: AuthResult) -> Step {
        match result {
            AuthResult::Success => {
                self.tried.push(label);
                Ok(true)
            }
            AuthResult::Failure {
                remaining_methods,
                partial_success,
            } => {
                self.tried.push(if partial_success {
                    format!("{label} (accepted, more required)")
                } else {
                    label
                });
                // russh reports a dead connection as a failure without methods.
                if remaining_methods.is_empty() && self.handle.is_closed() {
                    return Err(Halt::Lost);
                }
                self.server_methods = Some(remaining_methods);
                Ok(false)
            }
        }
    }

    async fn none(&mut self) -> Step {
        let result = bounded(self.step_timeout, self.handle.authenticate_none(self.username)).await?;
        self.record("none".to_owned(), result)
    }

    async fn password(&mut self, password: &str) -> Step {
        if self.offers(MethodKind::Password) {
            let attempt = self.handle.authenticate_password(self.username, password);
            let result = bounded(self.step_timeout, attempt).await?;
            return self.record("password".to_owned(), result);
        }
        if self.offers(MethodKind::KeyboardInteractive) {
            // Plenty of network devices only take the password this way.
            return self
                .keyboard_interactive(password, "password (via keyboard-interactive)")
                .await;
        }
        self.skip("password", "not offered by the server");
        Ok(false)
    }

    async fn keyboard_interactive(&mut self, answer: &str, label: &str) -> Step {
        if !self.offers(MethodKind::KeyboardInteractive) {
            self.skip(label, "not offered by the server");
            return Ok(false);
        }
        let start = self
            .handle
            .authenticate_keyboard_interactive_start(self.username, None);
        let mut reply = bounded(self.step_timeout, start).await?;
        let mut previous: Option<Vec<String>> = None;
        let mut refused = false;

        for _ in 0..MAX_KEYBOARD_ROUNDS {
            let prompts = match reply {
                KeyboardInteractiveAuthResponse::Success => {
                    return self.record(label.to_owned(), AuthResult::Success);
                }
                KeyboardInteractiveAuthResponse::Failure {
                    remaining_methods,
                    partial_success,
                } => {
                    let result = AuthResult::Failure {
                        remaining_methods,
                        partial_success,
                    };
                    return self.record(label.to_owned(), result);
                }
                KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => prompts,
            };
            let questions: Vec<String> = prompts.into_iter().map(|p| p.prompt).collect();
            // The same question again means the answer was refused; repeating
            // it would only burn attempts (and may lock the account). The
            // exchange still has to be answered for the session to move on.
            if !questions.is_empty() && previous.as_ref() == Some(&questions) {
                refused = true;
            }
            let fill = if refused { "" } else { answer };
            let responses = vec![fill.to_owned(); questions.len()];
            if !questions.is_empty() {
                previous = Some(questions);
            }
            let respond = self
                .handle
                .authenticate_keyboard_interactive_respond(responses);
            reply = bounded(self.step_timeout, respond).await?;
        }
        Err(Halt::Other(
            "the server keeps sending keyboard-interactive prompts".to_owned(),
        ))
    }

    async fn key_file(&mut self, path: &str, passphrase: Option<&str>) -> Step {
        let origin = format!("key file {path}");
        match tokio::fs::read_to_string(path).await {
            Ok(text) => self.private_key(&text, passphrase, &origin).await,
            Err(error) => {
                self.skip(&origin, &format!("cannot read it: {error}"));
                Ok(false)
            }
        }
    }

    async fn private_key(&mut self, text: &str, passphrase: Option<&str>, origin: &str) -> Step {
        if !self.offers(MethodKind::PublicKey) {
            self.skip(origin, "public key authentication is not offered by the server");
            return Ok(false);
        }
        let key = match decode_private_key(text, passphrase) {
            Ok(key) => Arc::new(key),
            Err(reason) => {
                self.skip(origin, &reason);
                return Ok(false);
            }
        };
        let public = key.public_key().clone();
        for hash in self.signature_hashes(&public).await? {
            let label = describe_key(&public, hash);
            if self.sha1_forbidden(&public, hash, &label) {
                continue;
            }
            let signed = PrivateKeyWithHashAlg::new(Arc::clone(&key), hash);
            let attempt = self.handle.authenticate_publickey(self.username, signed);
            let result = bounded(self.step_timeout, attempt).await?;
            if self.record(label, result)? {
                self.note_signature(&public, hash);
                return Ok(true);
            }
            if !self.offers(MethodKind::PublicKey) {
                break;
            }
        }
        Ok(false)
    }

    async fn agent(&mut self) -> Step {
        if !self.offers(MethodKind::PublicKey) {
            self.skip("agent", "public key authentication is not offered by the server");
            return Ok(false);
        }
        let mut agent = match connect_agent().await {
            Ok(agent) => agent,
            Err(reason) => {
                self.skip("agent", &reason);
                return Ok(false);
            }
        };
        let identities = match tokio::time::timeout(AGENT_TIMEOUT, agent.request_identities()).await
        {
            Ok(Ok(identities)) => identities,
            Ok(Err(error)) => {
                self.skip("agent", &format!("cannot list its keys: {error}"));
                return Ok(false);
            }
            Err(_) => {
                self.skip("agent", "it did not answer");
                return Ok(false);
            }
        };
        if identities.is_empty() {
            self.skip("agent", "it holds no keys");
            return Ok(false);
        }

        for identity in identities.into_iter().take(MAX_AGENT_KEYS) {
            let public = identity.public_key().into_owned();
            for hash in self.signature_hashes(&public).await? {
                let label = format!("agent {}", describe_key(&public, hash));
                if self.sha1_forbidden(&public, hash, &label) {
                    continue;
                }
                let attempt = self.handle.authenticate_publickey_with(
                    self.username,
                    public.clone(),
                    hash,
                    &mut agent,
                );
                match tokio::time::timeout(self.step_timeout, attempt).await {
                    Err(_) => return Err(Halt::Timeout),
                    Ok(Err(error)) => self.skip(&label, &format!("the agent could not sign: {error}")),
                    Ok(Ok(result)) => {
                        if self.record(label, result)? {
                            self.note_signature(&public, hash);
                            return Ok(true);
                        }
                    }
                }
                if !self.offers(MethodKind::PublicKey) {
                    return Ok(false);
                }
            }
        }
        Ok(false)
    }

    /// The signature hashes to try for `key`: a single `None` for anything
    /// but RSA, where the hash is what separates `ssh-rsa` from `rsa-sha2-*`.
    async fn signature_hashes(&mut self, key: &PublicKey) -> Result<Vec<Option<HashAlg>>, Halt> {
        if !key.algorithm().is_rsa() {
            return Ok(vec![None]);
        }
        if let Some(hashes) = &self.rsa_hashes {
            return Ok(hashes.clone());
        }
        let announced = bounded(self.step_timeout, self.handle.best_supported_rsa_hash()).await?;
        let hashes = match announced {
            // The server announced which signatures it takes.
            Some(Some(hash)) => vec![Some(hash)],
            Some(None) => vec![None],
            // No announcement: a server from before `rsa-sha2-*` existed is
            // by far the likeliest, so SHA-1 goes first where it is allowed.
            None => vec![None, Some(HashAlg::Sha256)],
        };
        self.rsa_hashes = Some(hashes.clone());
        Ok(hashes)
    }

    /// Whether this attempt needs a SHA-1 RSA signature that the policy forbids.
    fn sha1_forbidden(&mut self, key: &PublicKey, hash: Option<HashAlg>, label: &str) -> bool {
        let forbidden = key.algorithm().is_rsa() && hash.is_none() && !self.allow_sha1;
        if forbidden {
            self.sha1_refused = self.rsa_hashes.as_ref().is_some_and(|h| h.len() == 1);
            self.skip(label, "SHA-1 signatures need legacy crypto");
        }
        forbidden
    }

    fn note_signature(&mut self, key: &PublicKey, hash: Option<HashAlg>) {
        if key.algorithm().is_rsa() && hash.is_none() {
            self.used_sha1 = true;
        }
        // Whatever was refused before no longer matters.
        self.sha1_refused = false;
    }
}

/// `publickey (<signature algorithm> <fingerprint>)`.
fn describe_key(key: &PublicKey, hash: Option<HashAlg>) -> String {
    let algorithm = match (key.algorithm().is_rsa(), hash) {
        (true, Some(HashAlg::Sha512)) => "rsa-sha2-512".to_owned(),
        (true, Some(_)) => "rsa-sha2-256".to_owned(),
        _ => key.algorithm().as_str().to_owned(),
    };
    format!(
        "publickey ({algorithm} {})",
        key.fingerprint(HashAlg::Sha256)
    )
}

async fn bounded<T, E>(limit: Duration, step: impl Future<Output = Result<T, E>>) -> Result<T, Halt>
where
    E: Into<russh::Error>,
{
    match tokio::time::timeout(limit, step).await {
        Err(_) => Err(Halt::Timeout),
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(match error.into() {
            russh::Error::SendError
            | russh::Error::RecvError
            | russh::Error::Disconnect
            | russh::Error::HUP => Halt::Lost,
            russh::Error::IO(_) => Halt::Lost,
            other => Halt::Other(other.to_string()),
        }),
    }
}

type DynAgent = AgentClient<Box<dyn AgentStream + Send + Unpin + 'static>>;

/// Connects to the ssh-agent named by `SSH_AUTH_SOCK`.
#[cfg(unix)]
async fn connect_agent() -> Result<DynAgent, String> {
    match tokio::time::timeout(AGENT_TIMEOUT, AgentClient::connect_env()).await {
        Ok(Ok(agent)) => Ok(agent.dynamic()),
        Ok(Err(error)) => Err(format!("no ssh-agent: {error}")),
        Err(_) => Err("the ssh-agent did not answer".to_owned()),
    }
}

/// Connects to the OpenSSH agent pipe (or the pipe named by `SSH_AUTH_SOCK`),
/// falling back to Pageant.
#[cfg(windows)]
async fn connect_agent() -> Result<DynAgent, String> {
    const OPENSSH_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";
    let pipe = std::env::var("SSH_AUTH_SOCK")
        .ok()
        .filter(|path| path.starts_with(r"\\.\pipe\"))
        .unwrap_or_else(|| OPENSSH_PIPE.to_owned());
    let pipe_error = match tokio::time::timeout(AGENT_TIMEOUT, AgentClient::connect_named_pipe(&pipe)).await
    {
        Ok(Ok(agent)) => return Ok(agent.dynamic()),
        Ok(Err(error)) => error.to_string(),
        Err(_) => "the pipe is busy".to_owned(),
    };
    match tokio::time::timeout(AGENT_TIMEOUT, AgentClient::connect_pageant()).await {
        Ok(Ok(agent)) => Ok(agent.dynamic()),
        Ok(Err(error)) => Err(format!(
            "no ssh-agent: {pipe}: {pipe_error}; Pageant: {error}"
        )),
        Err(_) => Err(format!(
            "no ssh-agent: {pipe}: {pipe_error}; Pageant did not answer"
        )),
    }
}

#[cfg(not(any(unix, windows)))]
async fn connect_agent() -> Result<DynAgent, String> {
    Err("ssh-agent authentication is not supported on this platform".to_owned())
}

// `PrivateKey` is only named in signatures above through `Arc`; keep the
// import honest for readers skimming the types involved.
#[allow(dead_code)]
type LoadedKey = Arc<PrivateKey>;
