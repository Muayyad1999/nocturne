//! Interoperability with the OpenSSH daemon installed on this machine.
//!
//! Each test starts its own private `sshd` (own configuration, own host keys,
//! a port of its own on the loopback interface) and logs in as a throwaway
//! user. The tests are skipped — not failed — where that is not possible
//! (no `sshd`, not root); the reason is printed.

#![cfg(unix)]

mod common;

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use common::{Account, Tap, contains, run, skipped, soon, tap, write_private};
use nocturne_transport::{
    AcceptAnyHostKey, Connection, HostKeyVerifier, KeyKind, LegacyCrypto, MemoryKnownHosts,
    SshAuth, SshHop, SshParams, SshSession, TransportError, connect_ssh, generate_keypair,
};

const SSHD: &str = "/usr/sbin/sshd";

// ----------------------------------------------------------------- fixture

/// Keys shared by every daemon and test of this binary.
struct Keys {
    /// Host key files by type (`ed25519`, `rsa`, `ecdsa`, `dsa`).
    host: Vec<(&'static str, PathBuf)>,
    /// Client keys whose public halves are in the account's `authorized_keys`.
    ed25519: String,
    rsa: String,
    ecdsa: String,
    encrypted_ed25519: String,
    encrypted_pem_rsa: String,
    dsa: Option<String>,
    /// A key the server has never heard of.
    stranger: String,
}

const PASSPHRASE: &str = "correct horse battery staple";

/// The account and the keys, or `None` where these tests cannot run.
fn fixture() -> Option<(&'static Account, &'static Keys)> {
    static KEYS: OnceLock<Option<Keys>> = OnceLock::new();
    let account = common::account()?;
    let keys = KEYS
        .get_or_init(|| match Keys::create(account) {
            Ok(keys) => Some(keys),
            Err(reason) => {
                skipped(&format!(
                    "OpenSSH interoperability tests cannot run here: {reason}"
                ));
                None
            }
        })
        .as_ref()?;
    Some((account, keys))
}

impl Keys {
    fn create(account: &Account) -> Result<Keys, String> {
        if !std::path::Path::new(SSHD).is_file() {
            return Err(format!("{SSHD} is not installed"));
        }
        let scratch = |name: &str| account.scratch.join(name).to_string_lossy().into_owned();
        let keygen = |args: &[&str]| run("ssh-keygen", args).map(drop);
        let mut host = Vec::new();
        for kind in ["ed25519", "rsa", "ecdsa"] {
            let path = scratch(&format!("host_{kind}"));
            keygen(&["-q", "-t", kind, "-N", "", "-f", &path])?;
            host.push((kind, PathBuf::from(path)));
        }
        // DSA was removed from newer OpenSSH releases; optional.
        let dsa_host = scratch("host_dsa");
        if keygen(&["-q", "-t", "dsa", "-N", "", "-f", &dsa_host]).is_ok() {
            host.push(("dsa", PathBuf::from(dsa_host)));
        }

        let generated = |kind| generate_keypair(kind, "nocturne-test").map_err(|e| e.to_string());
        let (ed25519, ed25519_public) = generated(KeyKind::Ed25519)?;
        let (rsa, rsa_public) = generated(KeyKind::Rsa3072)?;
        let (stranger, _) = generated(KeyKind::Ed25519)?;

        // Keys made by OpenSSH's own tool, in its formats.
        let made_by_openssh = |name: &str, args: &[&str]| -> Result<(String, String), String> {
            let path = scratch(name);
            let mut full = vec!["-q", "-f", &path];
            full.extend_from_slice(args);
            keygen(&full)?;
            let read = |path: &str| std::fs::read_to_string(path).map_err(|e| e.to_string());
            Ok((read(&path)?, read(&format!("{path}.pub"))?))
        };
        let (ecdsa, ecdsa_public) =
            made_by_openssh("client_ecdsa", &["-t", "ecdsa", "-b", "384", "-N", ""])?;
        let (encrypted_ed25519, encrypted_ed25519_public) =
            made_by_openssh("client_ed25519_enc", &["-t", "ed25519", "-N", PASSPHRASE])?;
        let (encrypted_pem_rsa, encrypted_pem_rsa_public) = made_by_openssh(
            "client_rsa_pem",
            &["-t", "rsa", "-b", "2048", "-m", "PEM", "-N", PASSPHRASE],
        )?;
        let dsa = made_by_openssh("client_dsa", &["-t", "dsa", "-N", ""]).ok();

        let mut authorized = vec![
            ed25519_public,
            rsa_public,
            ecdsa_public,
            encrypted_ed25519_public,
            encrypted_pem_rsa_public,
        ];
        authorized.extend(dsa.iter().map(|(_, public)| public.clone()));
        let authorized: Vec<&str> = authorized.iter().map(|line| line.trim()).collect();
        account.install(".ssh/authorized_keys", &(authorized.join("\n") + "\n"));

        Ok(Keys {
            host,
            ed25519,
            rsa,
            ecdsa,
            encrypted_ed25519,
            encrypted_pem_rsa,
            dsa: dsa.map(|(private, _)| private),
            stranger,
        })
    }

    fn host_key(&self, kind: &str) -> Option<&PathBuf> {
        self.host
            .iter()
            .find(|(name, _)| *name == kind)
            .map(|(_, path)| path)
    }
}

/// A private OpenSSH daemon; stopped when dropped.
struct Sshd {
    child: Child,
    port: u16,
    log: PathBuf,
    account: &'static Account,
    keys: &'static Keys,
}

impl Sshd {
    /// Starts a daemon with the default settings plus `extra` configuration
    /// lines. `None` where these tests cannot run.
    fn start(extra: &str) -> Option<Sshd> {
        Self::start_with_host_keys(&["ed25519", "rsa", "ecdsa"], extra)
    }

    fn start_with_host_keys(host_keys: &[&str], extra: &str) -> Option<Sshd> {
        let (account, keys) = fixture()?;
        let port = common::free_port();
        let config_path = account.unique_path("sshd", "conf");
        let pid_path = config_path.with_extension("pid");
        let log = config_path.with_extension("log");

        let mut config = format!(
            "Port {port}\nListenAddress 127.0.0.1\nPidFile {}\n",
            pid_path.display()
        );
        for kind in host_keys {
            let path = keys.host_key(kind)?;
            config.push_str(&format!("HostKey {}\n", path.display()));
        }
        config.push_str(&format!(
            "UsePAM yes\n\
             AllowUsers {}\n\
             PermitRootLogin no\n\
             Subsystem sftp internal-sftp\n\
             UseDNS no\n\
             PrintMotd no\n\
             PrintLastLog no\n\
             MaxAuthTries 10\n\
             MaxStartups 100\n\
             LogLevel VERBOSE\n",
            account.name
        ));
        // The first value given for a keyword wins, so the defaults go last.
        let config = format!(
            "{extra}\n{config}PasswordAuthentication yes\nKbdInteractiveAuthentication yes\n"
        );
        write_private(&config_path, &config);

        common::ensure_sshd_privsep_dir().unwrap();
        let log_file = std::fs::File::create(&log).unwrap();
        let child = Command::new(SSHD)
            .args(["-D", "-e", "-f"])
            .arg(&config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log_file)
            .spawn()
            .unwrap_or_else(|error| panic!("cannot start {SSHD}: {error}"));
        let mut sshd = Sshd {
            child,
            port,
            log,
            account,
            keys,
        };
        sshd.wait_until_listening();
        Some(sshd)
    }

    fn wait_until_listening(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if std::net::TcpStream::connect(("127.0.0.1", self.port)).is_ok() {
                return;
            }
            let exited = self.child.try_wait().ok().flatten();
            if exited.is_some() || Instant::now() > deadline {
                panic!(
                    "sshd did not come up (exit: {exited:?}); its log:\n{}",
                    self.log_text()
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn params(&self, auth: Vec<SshAuth>) -> SshParams {
        SshParams {
            host: "127.0.0.1".to_owned(),
            port: self.port,
            username: self.account.name.clone(),
            auth,
            ..SshParams::default()
        }
    }

    fn password(&self) -> Vec<SshAuth> {
        vec![SshAuth::Password {
            password: self.account.password.clone(),
        }]
    }

    fn key(&self, private_key: &str) -> Vec<SshAuth> {
        vec![SshAuth::PrivateKey {
            key: private_key.to_owned(),
            passphrase: None,
        }]
    }

    async fn session(&self, params: &SshParams) -> SshSession {
        match soon(SshSession::connect(params, any_host_key())).await {
            Ok(session) => session,
            Err(error) => panic!("cannot connect: {error}\nsshd log:\n{}", self.log_text()),
        }
    }

    async fn shell(&self, params: &SshParams) -> Tap<Connection> {
        match soon(connect_ssh(params, any_host_key())).await {
            Ok(connection) => tap(connection),
            Err(error) => panic!("cannot connect: {error}\nsshd log:\n{}", self.log_text()),
        }
    }

    async fn error(&self, params: &SshParams) -> TransportError {
        match soon(connect_ssh(params, any_host_key())).await {
            Ok(_) => panic!("the connection unexpectedly succeeded"),
            Err(error) => error,
        }
    }
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn any_host_key() -> Arc<dyn HostKeyVerifier> {
    Arc::new(AcceptAnyHostKey)
}

fn auth_failure(error: &TransportError) -> (&[String], &[String]) {
    match error {
        TransportError::AuthFailed {
            tried,
            server_methods,
        } => (tried, server_methods),
        other => panic!("expected an authentication failure, got: {other}"),
    }
}

// ------------------------------------------------------------------- login

#[tokio::test]
async fn password_login_and_interactive_shell() {
    let Some(sshd) = Sshd::start("") else { return };
    let mut shell = sshd.shell(&sshd.params(sshd.password())).await;

    let info = shell.info().clone();
    let banner = info.server_banner.unwrap();
    assert!(banner.starts_with("SSH-2.0-OpenSSH_"), "{banner}");
    assert_eq!(info.peer, format!("127.0.0.1:{}", sshd.port));
    assert_eq!(info.kex.as_deref(), Some("curve25519-sha256"));
    assert_eq!(
        info.cipher.as_deref(),
        Some("chacha20-poly1305@openssh.com")
    );
    assert_eq!(info.mac, None);
    assert_eq!(info.host_key_algorithm.as_deref(), Some("ssh-ed25519"));
    assert!(!info.legacy_crypto);

    // The fingerprint is the one OpenSSH's own tool prints for the host key.
    let host_key = sshd.keys.host_key("ed25519").unwrap().with_extension("pub");
    let listing = run("ssh-keygen", &["-l", "-f", &host_key.to_string_lossy()]).unwrap();
    let fingerprint = info.host_key_fingerprint.unwrap();
    assert!(
        listing.contains(&fingerprint),
        "{fingerprint} not in {listing}"
    );

    // The shell computes the marker, so the echo of the command cannot match.
    shell.send("echo \"M$((1000 + 234))M\"\r").await.unwrap();
    shell.until(b"M1234M").await;
    shell.send("stty size\r").await.unwrap();
    shell.until(b"40 120").await;
    shell.resize(100, 30).await.unwrap();
    shell.send("stty size\r").await.unwrap();
    shell.until(b"30 100").await;
    shell.send("id -un | tr a-z A-Z\r").await.unwrap();
    shell
        .until(sshd.account.name.to_uppercase().as_bytes())
        .await;

    shell.send("exit 7\r").await.unwrap();
    let (_, reason) = shell.until_closed().await;
    assert_eq!(reason.as_deref(), Some("exit status 7"));
}

#[tokio::test]
async fn wrong_password_is_an_authentication_failure() {
    let Some(sshd) = Sshd::start("") else { return };
    let wrong = vec![SshAuth::Password {
        password: "not the password".to_owned(),
    }];
    let error = sshd.error(&sshd.params(wrong)).await;
    assert_eq!(error.code(), "auth");
    let (tried, offered) = auth_failure(&error);
    assert_eq!(tried, ["none", "password"]);
    for method in ["publickey", "password", "keyboard-interactive"] {
        assert!(
            offered.iter().any(|m| m == method),
            "{method} not in {offered:?}"
        );
    }
}

#[tokio::test]
async fn public_key_login_with_every_kind_of_key() {
    let Some(sshd) = Sshd::start("PasswordAuthentication no\nKbdInteractiveAuthentication no")
    else {
        return;
    };
    let keys = sshd.keys;
    let key_file = sshd.account.unique_path("id", "key");
    write_private(&key_file, &keys.ecdsa);
    let with_passphrase = |key: &str| {
        vec![SshAuth::PrivateKey {
            key: key.to_owned(),
            passphrase: Some(PASSPHRASE.to_owned()),
        }]
    };
    let cases = [
        ("Ed25519", sshd.key(&keys.ed25519)),
        ("RSA 3072", sshd.key(&keys.rsa)),
        ("ECDSA P-384 made by ssh-keygen", sshd.key(&keys.ecdsa)),
        (
            "encrypted Ed25519 (OpenSSH format)",
            with_passphrase(&keys.encrypted_ed25519),
        ),
        (
            "encrypted RSA (PEM format)",
            with_passphrase(&keys.encrypted_pem_rsa),
        ),
        (
            "key file",
            vec![SshAuth::KeyFile {
                path: key_file.to_string_lossy().into_owned(),
                passphrase: None,
            }],
        ),
    ];
    for (label, auth) in cases {
        let session = match soon(SshSession::connect(&sshd.params(auth), any_host_key())).await {
            Ok(session) => session,
            Err(error) => panic!("{label}: {error}\nsshd log:\n{}", sshd.log_text()),
        };
        assert!(!session.info().legacy_crypto, "{label}");
        let output = soon(session.exec("id -un", Duration::from_secs(10)))
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            sshd.account.name,
            "{label}"
        );
        assert_eq!(output.exit_status, Some(0), "{label}");
        soon(session.close()).await;
    }

    // A key the account does not know, and a password nobody asked for.
    let mut auth = sshd.key(&keys.stranger);
    auth.extend(sshd.password());
    let error = sshd.error(&sshd.params(auth)).await;
    let (tried, offered) = auth_failure(&error);
    assert_eq!(offered, ["publickey"]);
    assert_eq!(tried.len(), 3, "{tried:?}");
    assert!(
        tried[1].starts_with("publickey (ssh-ed25519 SHA256:"),
        "{tried:?}"
    );
    assert!(tried[2].contains("skipped"), "{tried:?}");
}

#[tokio::test]
async fn keyboard_interactive_is_answered_with_the_password() {
    let Some(sshd) =
        Sshd::start("PasswordAuthentication no\nAuthenticationMethods keyboard-interactive")
    else {
        return;
    };

    // A plain password entry: the server only does keyboard-interactive.
    let session = sshd.session(&sshd.params(sshd.password())).await;
    let output = soon(session.exec("echo ok", Duration::from_secs(10)))
        .await
        .unwrap();
    assert_eq!(output.stdout, b"ok\n");
    soon(session.close()).await;

    // An explicit keyboard-interactive entry.
    let auth = vec![SshAuth::KeyboardInteractive {
        response: sshd.account.password.clone(),
    }];
    let session = sshd.session(&sshd.params(auth)).await;
    soon(session.close()).await;

    // A wrong answer fails without the client hammering the account: PAM asks
    // again, the client recognises the repeated prompt and stops answering.
    let wrong = vec![SshAuth::Password {
        password: "not the password".to_owned(),
    }];
    let error = sshd.error(&sshd.params(wrong)).await;
    let (tried, offered) = auth_failure(&error);
    assert_eq!(tried, ["none", "password (via keyboard-interactive)"]);
    assert_eq!(offered, ["keyboard-interactive"]);
}

#[tokio::test]
async fn the_pre_authentication_banner_is_reported() {
    let Some((account, _)) = fixture() else {
        return;
    };
    let banner = account.unique_path("banner", "txt");
    std::fs::write(&banner, "Authorized use only.\nActivity is logged.\n").unwrap();
    let Some(sshd) = Sshd::start(&format!("Banner {}", banner.display())) else {
        return;
    };
    let session = sshd.session(&sshd.params(sshd.password())).await;
    let reported = session.info().server_banner.clone().unwrap();
    let (identification, text) = reported.split_once('\n').unwrap();
    assert!(identification.starts_with("SSH-2.0-OpenSSH_"), "{reported}");
    assert_eq!(text, "Authorized use only.\nActivity is logged.");
    soon(session.close()).await;
}

#[tokio::test]
async fn too_many_failures_end_with_the_servers_explanation() {
    let Some(sshd) = Sshd::start("MaxAuthTries 2") else {
        return;
    };
    let guess = |password: &str| SshAuth::Password {
        password: password.to_owned(),
    };
    let params = sshd.params(vec![guess("one"), guess("two"), guess("three")]);
    let error = sshd.error(&params).await;
    let (tried, _) = auth_failure(&error);
    let last = tried.last().unwrap();
    assert!(
        last.contains("Too many authentication failures"),
        "{tried:?}"
    );
}

// -------------------------------------------------------------------- exec

#[tokio::test]
async fn exec_collects_output_streams_and_status() {
    let Some(sshd) = Sshd::start("") else { return };
    let session = sshd
        .session(&sshd.params(sshd.key(&sshd.keys.ed25519)))
        .await;
    let exec = |command: &'static str| soon(session.exec(command, Duration::from_secs(20)));

    let output = exec("echo out; echo err >&2; exit 3").await.unwrap();
    assert_eq!(output.stdout, b"out\n");
    assert_eq!(output.stderr, b"err\n");
    assert_eq!(output.exit_status, Some(3));

    // No terminal is allocated for exec.
    let output = exec("tty").await.unwrap();
    assert_eq!(output.stdout, b"not a tty\n");

    // Binary data and plenty of it.
    let output = exec("head -c 5000000 /dev/zero | tr '\\0' '\\377'")
        .await
        .unwrap();
    assert_eq!(output.stdout.len(), 5_000_000);
    assert!(output.stdout.iter().all(|&byte| byte == 0xFF));
    assert_eq!(output.exit_status, Some(0));

    // Several commands at once on the same connection.
    let (a, b, c) = tokio::join!(exec("echo a"), exec("echo b"), exec("echo c"));
    assert_eq!(a.unwrap().stdout, b"a\n");
    assert_eq!(b.unwrap().stdout, b"b\n");
    assert_eq!(c.unwrap().stdout, b"c\n");

    // A command killed by a signal has no exit status.
    let output = exec("kill -KILL $$").await.unwrap();
    assert_eq!(output.exit_status, None);

    // A command that outlives its time limit.
    let started = Instant::now();
    let error = soon(session.exec("sleep 30", Duration::from_millis(500)))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "timeout");
    assert!(started.elapsed() < Duration::from_secs(5));
    // The session is still good afterwards.
    assert_eq!(
        exec("echo still here").await.unwrap().stdout,
        b"still here\n"
    );

    // A command nobody waits for any more is not left running: neither one
    // that ran out of time, nor one whose caller walked away.
    let running = || async {
        let output = exec("pgrep -u \"$(id -u)\" -x yes | wc -l").await.unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    let error = soon(session.exec("yes", Duration::from_millis(300)))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "timeout");
    let abandoned = session.exec("yes", Duration::from_secs(60));
    assert!(
        tokio::time::timeout(Duration::from_millis(300), abandoned)
            .await
            .is_err()
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while running().await != "0" {
        assert!(Instant::now() < deadline, "`yes` is still running");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Output without end is refused rather than collected.
    let error = soon(session.exec("cat /dev/zero", Duration::from_secs(120)))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "protocol", "{error}");
    assert!(error.to_string().contains("more than 256 MiB"), "{error}");
    assert_eq!(
        exec("echo still here").await.unwrap().stdout,
        b"still here\n"
    );
    soon(session.close()).await;
}

// -------------------------------------------------------------------- sftp

#[tokio::test]
async fn sftp_file_operations() {
    let Some(sshd) = Sshd::start("") else { return };
    let account = sshd.account;
    let session = sshd.session(&sshd.params(sshd.password())).await;
    let sftp = soon(session.sftp()).await.unwrap();

    let home = soon(sftp.realpath(".")).await.unwrap();
    assert_eq!(home, account.home.to_string_lossy());
    let dir = format!("{home}/sftp-ops");
    soon(sftp.mkdir(&dir)).await.unwrap();

    // Small files.
    let hello = format!("{dir}/hello.txt");
    soon(sftp.write(&hello, b"hello sftp\n")).await.unwrap();
    assert_eq!(soon(sftp.read(&hello)).await.unwrap(), b"hello sftp\n");
    soon(sftp.write(&hello, b"shorter\n")).await.unwrap();
    assert_eq!(soon(sftp.read(&hello)).await.unwrap(), b"shorter\n");
    let entry = soon(sftp.stat(&hello)).await.unwrap();
    assert_eq!(entry.name, "hello.txt");
    assert_eq!(entry.path, hello);
    assert_eq!(entry.size, 8);
    assert!(!entry.is_dir && !entry.is_link);
    assert!(entry.permissions.is_some_and(|mode| mode & 0o600 == 0o600));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(
        entry
            .modified
            .is_some_and(|mtime| (now - mtime).abs() < 300)
    );

    // Upload with progress.
    let local_dir = tempfile::tempdir().unwrap();
    let source = local_dir.path().join("upload.bin");
    let content: Vec<u8> = (0..3_500_000u32)
        .map(|i| (i.wrapping_mul(31) >> 3) as u8)
        .collect();
    std::fs::write(&source, &content).unwrap();
    let remote = format!("{dir}/upload.bin");
    let (progress, mut updates) = tokio::sync::mpsc::channel(4096);
    let sent = soon(sftp.upload(&source, &remote, Some(progress)))
        .await
        .unwrap();
    assert_eq!(sent as usize, content.len());
    let mut last = (0, 0);
    while let Ok(update) = updates.try_recv() {
        assert!(
            update.0 >= last.0 && update.1 == content.len() as u64,
            "{update:?}"
        );
        last = update;
    }
    assert_eq!(last, (content.len() as u64, content.len() as u64));
    assert_eq!(
        soon(sftp.stat(&remote)).await.unwrap().size as usize,
        content.len()
    );

    // A directory with a bit of everything.
    soon(sftp.mkdir(&format!("{dir}/sub"))).await.unwrap();
    let made = soon(session.exec(
        &format!("cd {dir} && ln -s hello.txt link && ln -s sub linkdir && ln -s nowhere dangling"),
        Duration::from_secs(10),
    ))
    .await
    .unwrap();
    assert_eq!(made.exit_status, Some(0));
    let listing = soon(sftp.list(&dir)).await.unwrap();
    let names: Vec<&str> = listing.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "dangling",
            "hello.txt",
            "link",
            "linkdir",
            "sub",
            "upload.bin"
        ]
    );
    let entry = |name: &str| listing.iter().find(|entry| entry.name == name).unwrap();
    assert_eq!(entry("upload.bin").path, remote);
    assert_eq!(entry("upload.bin").size as usize, content.len());
    assert!(entry("sub").is_dir && !entry("sub").is_link);
    assert!(entry("link").is_link && !entry("link").is_dir);
    assert_eq!(entry("link").size, 8, "a link reports its target's size");
    assert!(entry("linkdir").is_link && entry("linkdir").is_dir);
    assert!(entry("dangling").is_link && !entry("dangling").is_dir);
    let via_link = soon(sftp.stat(&format!("{dir}/linkdir"))).await.unwrap();
    assert!(via_link.is_link && via_link.is_dir);

    // Download with progress, and compare.
    let target = local_dir.path().join("download.bin");
    let (progress, mut updates) = tokio::sync::mpsc::channel(4096);
    let received = soon(sftp.download(&remote, &target, Some(progress)))
        .await
        .unwrap();
    assert_eq!(received as usize, content.len());
    assert!(
        std::fs::read(&target).unwrap() == content,
        "downloaded data differs"
    );
    let mut last = (0, 0);
    while let Ok(update) = updates.try_recv() {
        last = update;
    }
    assert_eq!(last, (content.len() as u64, content.len() as u64));
    assert!(!target.with_file_name("download.bin.nocturne-part").exists());

    // A failed download leaves the existing local file alone.
    let error = soon(sftp.download(&format!("{dir}/missing"), &target, None))
        .await
        .unwrap_err();
    assert!(matches!(&error, TransportError::Io(e) if e.kind() == std::io::ErrorKind::NotFound));
    assert_eq!(std::fs::read(&target).unwrap().len(), content.len());

    // Rename, remove.
    let renamed = format!("{dir}/renamed.bin");
    soon(sftp.rename(&remote, &renamed)).await.unwrap();
    assert_eq!(
        soon(sftp.stat(&renamed)).await.unwrap().size as usize,
        content.len()
    );
    let error = soon(sftp.stat(&remote)).await.unwrap_err();
    assert!(matches!(&error, TransportError::Io(e) if e.kind() == std::io::ErrorKind::NotFound));

    assert!(
        soon(sftp.remove(&dir)).await.is_err(),
        "a full directory stays"
    );
    for name in [
        "renamed.bin",
        "hello.txt",
        "link",
        "linkdir",
        "dangling",
        "sub",
    ] {
        soon(sftp.remove(&format!("{dir}/{name}"))).await.unwrap();
    }
    assert!(soon(sftp.list(&dir)).await.unwrap().is_empty());
    soon(sftp.remove(&dir)).await.unwrap();
    let error = soon(sftp.list(&dir)).await.unwrap_err();
    assert!(matches!(&error, TransportError::Io(e) if e.kind() == std::io::ErrorKind::NotFound));

    // What the account may not do is a permission error.
    let error = soon(sftp.write("/etc/nocturne-test", b"x"))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, TransportError::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied),
        "{error}"
    );
    let error = soon(sftp.read("/etc/shadow")).await.unwrap_err();
    assert!(
        matches!(&error, TransportError::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied),
        "{error}"
    );

    // The session stays usable next to SFTP, and outlives it.
    soon(sftp.close()).await;
    let output = soon(session.exec("echo after", Duration::from_secs(10)))
        .await
        .unwrap();
    assert_eq!(output.stdout, b"after\n");
    soon(session.close()).await;
}

#[tokio::test]
async fn sftp_keeps_the_connection_alive_on_its_own() {
    let Some(sshd) = Sshd::start("") else { return };
    let session = sshd.session(&sshd.params(sshd.password())).await;
    let sftp = soon(session.sftp()).await.unwrap();
    drop(session);
    let home = soon(sftp.realpath(".")).await.unwrap();
    assert!(!soon(sftp.list(&home)).await.unwrap().is_empty());
    soon(sftp.close()).await;
}

// -------------------------------------------------------------- jump hosts

#[tokio::test]
async fn jump_through_the_same_daemon() {
    let Some(sshd) = Sshd::start("") else { return };
    let hop = SshHop {
        host: "127.0.0.1".to_owned(),
        port: sshd.port,
        username: sshd.account.name.clone(),
        auth: sshd.password(),
    };
    // The hop logs in with the password, the target with a key.
    let params = SshParams {
        jump: vec![hop.clone()],
        ..sshd.params(sshd.key(&sshd.keys.ed25519))
    };
    let session = sshd.session(&params).await;
    assert_eq!(session.info().via, [format!("127.0.0.1:{}", sshd.port)]);
    let output = soon(session.exec(
        "echo \"from ${SSH_CONNECTION%% *}\"; head -c 2000000 /dev/zero | wc -c",
        Duration::from_secs(20),
    ))
    .await
    .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.starts_with("from 127.0.0.1"), "{text}");
    assert!(text.trim_end().ends_with("2000000"), "{text}");

    let mut shell = tap(soon(session.shell()).await.unwrap());
    shell.send("echo \"J$((40 + 2))J\"\r").await.unwrap();
    shell.until(b"J42J").await;
    shell.close().await;
    soon(session.close()).await;

    // Two hops through the same daemon, with trust-on-first-use checking the
    // (identical) key each time.
    let known = Arc::new(MemoryKnownHosts::new());
    let params = SshParams {
        jump: vec![hop.clone(), hop.clone()],
        ..sshd.params(sshd.password())
    };
    let session = soon(SshSession::connect(&params, known.clone()))
        .await
        .unwrap();
    assert_eq!(session.info().via.len(), 2);
    assert_eq!(known.entries().len(), 1);
    soon(session.close()).await;

    // A hop that is not allowed to forward.
    let Some(strict) = Sshd::start("AllowTcpForwarding no") else {
        return;
    };
    let params = SshParams {
        jump: vec![SshHop {
            port: strict.port,
            ..hop
        }],
        ..sshd.params(sshd.password())
    };
    let error = sshd.error(&params).await;
    assert_eq!(error.code(), "protocol", "{error}");
    assert!(
        error.to_string().contains("does not allow forwarding"),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bulk_traffic_both_ways_on_several_channels_through_a_jump_host() {
    const SHELLS: usize = 3;
    const CHUNK: usize = 32_000;
    const CHUNKS: usize = 100;
    const FILE: usize = 6 * 1024 * 1024;
    let Some(sshd) = Sshd::start("") else { return };
    let hop = SshHop {
        host: "127.0.0.1".to_owned(),
        port: sshd.port,
        username: sshd.account.name.clone(),
        auth: sshd.password(),
    };
    let params = SshParams {
        jump: vec![hop],
        ..sshd.params(sshd.password())
    };
    let session = sshd.session(&params).await;
    let letter = |index: usize| b'a' + (index % 26) as u8;

    // Shells that send megabytes to `cat` and read them back…
    let mut work = Vec::new();
    for _ in 0..SHELLS {
        let (writer, reader) = soon(session.shell()).await.unwrap().split();
        work.push(tokio::spawn(async move {
            let mut reader = tap(reader);
            // Raw mode: the terminal passes every byte through untouched.
            writer
                .send("stty raw -echo; echo \"G$((20 + 2))G\"; exec cat\r")
                .await
                .unwrap();
            reader.until(b"G22G\n").await;
            let sender = tokio::spawn(async move {
                let chunk = bytes::Bytes::from((0..CHUNK).map(letter).collect::<Vec<u8>>());
                for _ in 0..CHUNKS {
                    writer.send(chunk.clone()).await.unwrap();
                }
                writer
            });
            let echoed = reader.exactly(CHUNK * CHUNKS).await;
            for (index, byte) in echoed.iter().enumerate() {
                assert_eq!(*byte, letter(index % CHUNK), "byte {index} is wrong");
            }
            let writer = soon(sender).await.unwrap();
            writer.close().await;
            let (_, reason) = reader.until_closed().await;
            assert_eq!(reason, None);
        }));
    }

    // …while a file goes up and comes back down over the same session.
    let sftp = soon(session.sftp()).await.unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let original = scratch.path().join("original.bin");
    let returned = scratch.path().join("returned.bin");
    let content: Vec<u8> = (0..FILE).map(|index| (index % 251) as u8).collect();
    std::fs::write(&original, &content).unwrap();
    let remote = format!("{}/bulk.bin", sshd.account.home.display());
    for _ in 0..2 {
        assert_eq!(
            soon(sftp.upload(&original, &remote, None)).await.unwrap(),
            FILE as u64
        );
        assert_eq!(
            soon(sftp.download(&remote, &returned, None)).await.unwrap(),
            FILE as u64
        );
        assert!(
            std::fs::read(&returned).unwrap() == content,
            "the file changed"
        );
    }
    soon(sftp.remove(&remote)).await.unwrap();
    soon(sftp.close()).await;

    for shell in work {
        soon(shell).await.unwrap();
    }
    soon(session.close()).await;
}

// ------------------------------------------------------------ robustness

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_reader_keeps_answering_the_servers_keepalives() {
    // This daemon drops clients that miss two keep-alives one second apart.
    let Some(sshd) = Sshd::start("ClientAliveInterval 1\nClientAliveCountMax 2") else {
        return;
    };
    let mut shell = sshd.shell(&sshd.params(sshd.password())).await;
    shell
        .send("stty -echo; echo \"R$((1 + 1))R\"\r")
        .await
        .unwrap();
    shell.until(b"R2R").await;

    // Half a million short lines, and a reader that looks away for a while.
    shell
        .send("seq 1 500000; echo \"D$((3 + 4))D\"\r")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    let output = shell.until(b"D7D").await;
    let lines = output.split(|&byte| byte == b'\n').count();
    assert!(lines > 500_000, "only {lines} lines arrived");
    assert!(contains(&output, b"\r\n499999\r\n500000\r\n"));

    shell.send("echo \"S$((5 + 5))S\"\r").await.unwrap();
    shell.until(b"S10S").await;
    shell.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_session_survives_re_keying_under_load() {
    const CHUNK: usize = 32_000;
    const CHUNKS: usize = 100;
    const FILE: usize = 4 * 1024 * 1024;
    // The daemon asks for new keys after every 300 KiB, in the middle of
    // megabytes moving both ways; once with today's algorithms and once
    // with what an old device would use.
    for (algorithms, legacy) in [
        ("", false),
        (
            "KexAlgorithms diffie-hellman-group14-sha1\nCiphers aes128-cbc\nMACs hmac-sha1\n",
            true,
        ),
    ] {
        let config = format!("{algorithms}RekeyLimit 300K\nLogLevel DEBUG1");
        let Some(sshd) = Sshd::start(&config) else {
            return;
        };
        let session = sshd.session(&sshd.params(sshd.password())).await;
        assert_eq!(session.info().legacy_crypto, legacy);
        let letter = |index: usize| b'a' + (index % 26) as u8;

        let (writer, reader) = soon(session.shell()).await.unwrap().split();
        let echo = tokio::spawn(async move {
            let mut reader = tap(reader);
            writer
                .send("stty raw -echo; echo \"G$((20 + 2))G\"; exec cat\r")
                .await
                .unwrap();
            reader.until(b"G22G\n").await;
            let sender = tokio::spawn(async move {
                let chunk = bytes::Bytes::from((0..CHUNK).map(letter).collect::<Vec<u8>>());
                for _ in 0..CHUNKS {
                    writer.send(chunk.clone()).await.unwrap();
                }
                writer
            });
            let echoed = reader.exactly(CHUNK * CHUNKS).await;
            for (index, byte) in echoed.iter().enumerate() {
                assert_eq!(*byte, letter(index % CHUNK), "byte {index} is wrong");
            }
            soon(sender).await.unwrap().close().await;
            let (_, reason) = reader.until_closed().await;
            assert_eq!(reason, None);
        });

        let sftp = soon(session.sftp()).await.unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let original = scratch.path().join("original.bin");
        let returned = scratch.path().join("returned.bin");
        let content: Vec<u8> = (0..FILE).map(|index| (index % 251) as u8).collect();
        std::fs::write(&original, &content).unwrap();
        let remote = format!("{}/rekey.bin", sshd.account.home.display());
        soon(sftp.upload(&original, &remote, None)).await.unwrap();
        soon(sftp.download(&remote, &returned, None)).await.unwrap();
        assert!(
            std::fs::read(&returned).unwrap() == content,
            "the file changed"
        );
        soon(sftp.remove(&remote)).await.unwrap();
        soon(sftp.close()).await;
        soon(echo).await.unwrap();
        soon(session.close()).await;

        let log = sshd.log_text();
        let rekeys = log.lines().filter(|line| line.contains("rekeying")).count();
        assert!(
            rekeys >= 10,
            "the daemon re-keyed only {rekeys} times:\n{log}"
        );
    }
}

#[tokio::test]
async fn a_killed_session_is_reported_as_a_lost_connection() {
    let Some(sshd) = Sshd::start("") else { return };
    let mut shell = sshd.shell(&sshd.params(sshd.password())).await;
    shell.send("echo \"K$((1 + 2))K\"\r").await.unwrap();
    shell.until(b"K3K").await;
    // The shell's parent is the sshd process serving this connection.
    shell.send("kill -KILL $PPID\r").await.unwrap();
    let (_, reason) = shell.until_closed().await;
    let reason = reason.unwrap();
    assert!(
        reason == "connection closed by remote host" || reason.starts_with("connection "),
        "{reason}"
    );
}

#[tokio::test]
async fn known_hosts_accept_the_same_daemon_and_reject_another() {
    let Some(first) = Sshd::start("") else { return };
    let known = Arc::new(MemoryKnownHosts::new());
    let params = first.params(first.password());
    for _ in 0..2 {
        let session = soon(SshSession::connect(&params, known.clone()))
            .await
            .unwrap();
        soon(session.close()).await;
    }

    // Another daemon presenting another key under the remembered name.
    let Some(second) = Sshd::start_with_host_keys(&["rsa"], "") else {
        return;
    };
    let stored = known.get("127.0.0.1", first.port);
    assert_eq!(stored.len(), 1);
    known.insert("127.0.0.1", second.port, &stored[0]);
    let error = soon(connect_ssh(
        &second.params(second.password()),
        known.clone(),
    ))
    .await
    .unwrap_err();
    assert_eq!(error.code(), "host_key");
    assert!(error.to_string().contains("has changed"), "{error}");
}

// ------------------------------------------------------------ legacy crypto

/// Which negotiated value a legacy-only daemon is expected to produce.
#[derive(Clone, Copy)]
enum Negotiated {
    Kex,
    Cipher,
    Mac,
    HostKey,
}

/// Against a daemon restricted to one legacy algorithm: `Auto` and `Always`
/// connect and flag the session, `Never` fails and names the algorithm.
async fn assert_legacy(sshd: &Sshd, what: Negotiated, algorithm: &str) {
    for legacy in [LegacyCrypto::Auto, LegacyCrypto::Always] {
        let params = SshParams {
            legacy,
            ..sshd.params(sshd.password())
        };
        let session = match soon(SshSession::connect(&params, any_host_key())).await {
            Ok(session) => session,
            Err(error) => panic!(
                "{algorithm} with {legacy:?}: {error}\nsshd log:\n{}",
                sshd.log_text()
            ),
        };
        let info = session.info().clone();
        assert!(
            info.legacy_crypto,
            "{algorithm} with {legacy:?} must be flagged"
        );
        let negotiated = match what {
            Negotiated::Kex => info.kex,
            Negotiated::Cipher => info.cipher,
            Negotiated::Mac => info.mac,
            Negotiated::HostKey => info.host_key_algorithm,
        };
        assert_eq!(negotiated.as_deref(), Some(algorithm));
        // The session really works under these algorithms.
        let output =
            soon(session.exec("head -c 300000 /dev/zero | wc -c", Duration::from_secs(20)))
                .await
                .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "300000");
        soon(session.close()).await;
    }

    let params = SshParams {
        legacy: LegacyCrypto::Never,
        ..sshd.params(sshd.password())
    };
    let error = sshd.error(&params).await;
    assert_eq!(error.code(), "algorithms", "{algorithm}: {error}");
    let message = error.to_string();
    assert!(message.contains(algorithm), "{message}");
    assert!(message.contains("legacy"), "{message}");
}

#[tokio::test]
async fn legacy_key_exchanges_against_openssh() {
    for algorithm in [
        "diffie-hellman-group1-sha1",
        "diffie-hellman-group14-sha1",
        "diffie-hellman-group-exchange-sha1",
    ] {
        let Some(sshd) = Sshd::start(&format!("KexAlgorithms {algorithm}")) else {
            return;
        };
        assert_legacy(&sshd, Negotiated::Kex, algorithm).await;
    }
}

#[tokio::test]
async fn legacy_ciphers_against_openssh() {
    for algorithm in ["aes128-cbc", "aes192-cbc", "aes256-cbc", "3des-cbc"] {
        let Some(sshd) = Sshd::start(&format!("Ciphers {algorithm}")) else {
            return;
        };
        assert_legacy(&sshd, Negotiated::Cipher, algorithm).await;
    }
}

#[tokio::test]
async fn legacy_macs_against_openssh() {
    for algorithm in ["hmac-sha1", "hmac-sha1-etm@openssh.com"] {
        // A cipher that needs a MAC, or the MAC would not be used at all.
        let Some(sshd) = Sshd::start(&format!("Ciphers aes256-ctr\nMACs {algorithm}")) else {
            return;
        };
        assert_legacy(&sshd, Negotiated::Mac, algorithm).await;
    }
}

#[tokio::test]
async fn legacy_host_keys_against_openssh() {
    let Some((_, keys)) = fixture() else { return };
    let Some(sshd) = Sshd::start_with_host_keys(&["rsa"], "HostKeyAlgorithms ssh-rsa") else {
        return;
    };
    assert_legacy(&sshd, Negotiated::HostKey, "ssh-rsa").await;

    if keys.host_key("dsa").is_none() {
        skipped("this OpenSSH cannot create DSA keys; ssh-dss host keys not tested");
        return;
    }
    let Some(sshd) = Sshd::start_with_host_keys(&["dsa"], "HostKeyAlgorithms ssh-dss") else {
        return;
    };
    assert_legacy(&sshd, Negotiated::HostKey, "ssh-dss").await;
}

#[tokio::test]
async fn legacy_public_key_signatures_against_openssh() {
    // The daemon only takes SHA-1 signatures from RSA user keys.
    let Some(sshd) = Sshd::start("PubkeyAcceptedAlgorithms ssh-rsa\nPasswordAuthentication no")
    else {
        return;
    };
    let rsa = sshd.key(&sshd.keys.rsa);
    for legacy in [LegacyCrypto::Auto, LegacyCrypto::Always] {
        let params = SshParams {
            legacy,
            ..sshd.params(rsa.clone())
        };
        let session = sshd.session(&params).await;
        assert!(session.info().legacy_crypto, "{legacy:?} must be flagged");
        // Everything negotiated for the transport is modern.
        assert_eq!(session.info().kex.as_deref(), Some("curve25519-sha256"));
        soon(session.close()).await;
    }
    let params = SshParams {
        legacy: LegacyCrypto::Never,
        ..sshd.params(rsa)
    };
    let error = sshd.error(&params).await;
    assert_eq!(error.code(), "algorithms", "{error}");
    assert!(error.to_string().contains("ssh-rsa"), "{error}");
    assert!(error.to_string().contains("legacy"), "{error}");

    // Other key types are not affected by the restriction's reason.
    let params = SshParams {
        legacy: LegacyCrypto::Never,
        ..sshd.params(sshd.key(&sshd.keys.ed25519))
    };
    let error = sshd.error(&params).await;
    assert_eq!(error.code(), "auth", "{error}");
}

#[tokio::test]
async fn dsa_user_keys_against_openssh() {
    let Some((_, keys)) = fixture() else { return };
    let Some(dsa) = keys.dsa.as_deref() else {
        skipped("this OpenSSH cannot create DSA keys; DSA user keys not tested");
        return;
    };
    let Some(sshd) = Sshd::start("PubkeyAcceptedAlgorithms ssh-dss\nPasswordAuthentication no")
    else {
        return;
    };
    let session = sshd.session(&sshd.params(sshd.key(dsa))).await;
    assert!(session.info().legacy_crypto, "DSA signatures are SHA-1");
    let output = soon(session.exec("echo dsa", Duration::from_secs(10)))
        .await
        .unwrap();
    assert_eq!(output.stdout, b"dsa\n");
    soon(session.close()).await;

    let params = SshParams {
        legacy: LegacyCrypto::Never,
        ..sshd.params(sshd.key(dsa))
    };
    let error = sshd.error(&params).await;
    assert_eq!(error.code(), "algorithms", "{error}");
}

#[tokio::test]
async fn an_old_device_profile_works_with_auto() {
    // Everything at once, the way a fifteen-year-old switch would have it.
    let Some(sshd) = Sshd::start_with_host_keys(
        &["rsa"],
        "KexAlgorithms diffie-hellman-group1-sha1\n\
         Ciphers 3des-cbc,aes128-cbc\n\
         MACs hmac-sha1\n\
         HostKeyAlgorithms ssh-rsa\n\
         PubkeyAcceptedAlgorithms ssh-rsa",
    ) else {
        return;
    };
    let mut shell = sshd.shell(&sshd.params(sshd.key(&sshd.keys.rsa))).await;
    let info = shell.info().clone();
    assert!(info.legacy_crypto);
    assert_eq!(info.kex.as_deref(), Some("diffie-hellman-group1-sha1"));
    assert_eq!(info.cipher.as_deref(), Some("aes128-cbc"));
    assert_eq!(info.mac.as_deref(), Some("hmac-sha1"));
    assert_eq!(info.host_key_algorithm.as_deref(), Some("ssh-rsa"));

    shell
        .send("seq 1 20000 | tail -n 1; echo \"L$((6 * 7))L\"\r")
        .await
        .unwrap();
    let output = shell.until(b"L42L").await;
    assert!(contains(&output, b"20000"));
    shell.send("exit\r").await.unwrap();
    let (_, reason) = shell.until_closed().await;
    assert_eq!(reason.as_deref(), Some("exit status 0"));
}

#[tokio::test]
async fn algorithms_russh_lacks_are_named_in_the_error() {
    // Legacy MACs OpenSSH still has but this client cannot do at all, and
    // the one modern key exchange russh does not implement.
    let cases = [
        ("Ciphers aes256-ctr\nMACs hmac-md5", "MAC", "hmac-md5"),
        ("Ciphers aes256-ctr\nMACs hmac-md5-96", "MAC", "hmac-md5-96"),
        (
            "Ciphers aes256-ctr\nMACs hmac-sha1-96",
            "MAC",
            "hmac-sha1-96",
        ),
        (
            "Ciphers aes256-ctr\nMACs umac-64@openssh.com",
            "MAC",
            "umac-64@openssh.com",
        ),
        (
            "Ciphers aes256-ctr\nMACs umac-128-etm@openssh.com",
            "MAC",
            "umac-128-etm@openssh.com",
        ),
        (
            "KexAlgorithms sntrup761x25519-sha512@openssh.com",
            "key exchange",
            "sntrup761x25519-sha512@openssh.com",
        ),
    ];
    for (config, family, algorithm) in cases {
        let Some(sshd) = Sshd::start(config) else {
            return;
        };
        for legacy in [
            LegacyCrypto::Auto,
            LegacyCrypto::Always,
            LegacyCrypto::Never,
        ] {
            let params = SshParams {
                legacy,
                ..sshd.params(sshd.password())
            };
            let error = sshd.error(&params).await;
            assert_eq!(
                error.code(),
                "algorithms",
                "{algorithm} with {legacy:?}: {error}"
            );
            let message = error.to_string();
            assert!(
                message.contains(&format!("no common {family} algorithm")),
                "{message}"
            );
            assert!(message.contains(algorithm), "{message}");
            assert!(message.contains("none of them is supported"), "{message}");
        }
    }
}

#[tokio::test]
async fn modern_daemon_is_never_flagged() {
    let Some(sshd) = Sshd::start("") else { return };
    for legacy in [
        LegacyCrypto::Auto,
        LegacyCrypto::Always,
        LegacyCrypto::Never,
    ] {
        let params = SshParams {
            legacy,
            ..sshd.params(sshd.password())
        };
        let session = sshd.session(&params).await;
        assert!(!session.info().legacy_crypto, "{legacy:?}");
        soon(session.close()).await;
    }
    // Each modern family on its own, with legacy algorithms forbidden.
    let cases = [
        ("KexAlgorithms ecdh-sha2-nistp256", "ecdh-sha2-nistp256"),
        ("KexAlgorithms ecdh-sha2-nistp521", "ecdh-sha2-nistp521"),
        (
            "KexAlgorithms diffie-hellman-group-exchange-sha256",
            "diffie-hellman-group-exchange-sha256",
        ),
        (
            "KexAlgorithms diffie-hellman-group14-sha256",
            "diffie-hellman-group14-sha256",
        ),
        (
            "KexAlgorithms diffie-hellman-group16-sha512",
            "diffie-hellman-group16-sha512",
        ),
    ];
    for (config, kex) in cases {
        let Some(sshd) = Sshd::start(config) else {
            return;
        };
        let params = SshParams {
            legacy: LegacyCrypto::Never,
            ..sshd.params(sshd.password())
        };
        let session = sshd.session(&params).await;
        assert_eq!(session.info().kex.as_deref(), Some(kex));
        assert!(!session.info().legacy_crypto, "{kex}");
        soon(session.close()).await;
    }
    for cipher in [
        "aes256-gcm@openssh.com",
        "aes128-gcm@openssh.com",
        "aes128-ctr",
        "aes256-ctr",
    ] {
        let Some(sshd) = Sshd::start(&format!("Ciphers {cipher}\nMACs hmac-sha2-512")) else {
            return;
        };
        let params = SshParams {
            legacy: LegacyCrypto::Never,
            ..sshd.params(sshd.password())
        };
        let session = sshd.session(&params).await;
        assert_eq!(session.info().cipher.as_deref(), Some(cipher));
        assert!(!session.info().legacy_crypto, "{cipher}");
        soon(session.close()).await;
    }
    for (host_key, algorithm) in [("rsa", "rsa-sha2-512"), ("ecdsa", "ecdsa-sha2-nistp256")] {
        let Some(sshd) = Sshd::start_with_host_keys(&[host_key], "") else {
            return;
        };
        let params = SshParams {
            legacy: LegacyCrypto::Never,
            ..sshd.params(sshd.password())
        };
        let session = sshd.session(&params).await;
        assert_eq!(
            session.info().host_key_algorithm.as_deref(),
            Some(algorithm)
        );
        assert!(!session.info().legacy_crypto, "{algorithm}");
        soon(session.close()).await;
    }
}
