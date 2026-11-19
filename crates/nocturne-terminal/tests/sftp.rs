//! SFTP against a real OpenSSH daemon.
//!
//! An in-process `serve_ssh` has no SFTP subsystem, so this starts a private
//! `sshd` on a free high port with its own host key, its own
//! `authorized_keys` and its own directory, and talks to that. Where no
//! `sshd` can be started (no binary, or the environment will not let one
//! run) the tests say so and pass: they are an interoperability check, not a
//! reason to fail a build.

mod common;

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nocturne_app::App;
use serde_json::{Value, json};

/// A private `sshd`, stopped when the test is done with it.
struct Sshd {
    child: Child,
    port: u16,
    key: String,
    user: String,
    home: PathBuf,
    _dir: tempfile::TempDir,
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn skip(why: &str) {
    eprintln!("skipping the SFTP tests: {why}");
}

fn find(candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

fn keygen(path: &Path) -> Option<String> {
    let done = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "nocturne-test", "-f"])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    done.success()
        .then(|| std::fs::read_to_string(path).ok())
        .flatten()
}

/// A free port, released again straight away. A private daemon has to be
/// given a port, and this is the least unreliable way to choose one.
fn free_port() -> Option<u16> {
    let listener = std::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).ok()?;
    listener.local_addr().ok().map(|address| address.port())
}

/// Start `sshd` with a configuration of our own, or say why not.
fn start_sshd() -> Result<Sshd, String> {
    let sshd = find(&["/usr/sbin/sshd", "/usr/local/sbin/sshd", "/sbin/sshd"])
        .ok_or("no sshd on this machine")?;
    let subsystem = find(&[
        "/usr/lib/openssh/sftp-server",
        "/usr/libexec/openssh/sftp-server",
        "/usr/libexec/sftp-server",
        "/usr/lib/ssh/sftp-server",
    ])
    .ok_or("no sftp-server on this machine")?;
    // A modern sshd insists on its privilege-separation directory, which a
    // container image often has not created yet. Making it is the one thing
    // this harness needs that an ordinary user cannot do; without it the
    // tests skip.
    let privsep = Path::new("/run/sshd");
    if !privsep.is_dir() && std::fs::create_dir_all(privsep).is_err() {
        return Err(format!(
            "{} does not exist and cannot be created (sshd needs it)",
            privsep.display()
        ));
    }
    let dir = tempfile::tempdir().map_err(|err| err.to_string())?;
    let root = dir.path();
    let host_key = root.join("host_ed25519");
    keygen(&host_key).ok_or("ssh-keygen could not make a host key")?;
    let client_key = root.join("client_ed25519");
    let private = keygen(&client_key).ok_or("ssh-keygen could not make a client key")?;
    let public =
        std::fs::read_to_string(root.join("client_ed25519.pub")).map_err(|err| err.to_string())?;

    let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
    let home = root.join("home");
    std::fs::create_dir_all(&home).map_err(|err| err.to_string())?;
    let authorized = root.join("authorized_keys");
    std::fs::write(&authorized, &public).map_err(|err| err.to_string())?;

    let port = free_port().ok_or("no free port")?;
    let config = root.join("sshd_config");
    let mut file = std::fs::File::create(&config).map_err(|err| err.to_string())?;
    writeln!(
        file,
        "Port {port}\n\
         ListenAddress 127.0.0.1\n\
         HostKey {host_key}\n\
         PidFile none\n\
         StrictModes no\n\
         UsePAM no\n\
         PasswordAuthentication no\n\
         KbdInteractiveAuthentication no\n\
         PubkeyAuthentication yes\n\
         PermitRootLogin prohibit-password\n\
         AuthorizedKeysFile {authorized}\n\
         Subsystem sftp {subsystem}\n\
         LogLevel ERROR",
        host_key = host_key.display(),
        authorized = authorized.display(),
        subsystem = subsystem.display(),
    )
    .map_err(|err| err.to_string())?;

    // Its complaints are kept, so a skip can say what the daemon objected to.
    let diagnostics = root.join("sshd.log");
    let log = std::fs::File::create(&diagnostics).map_err(|err| err.to_string())?;
    let child = Command::new(&sshd)
        .arg("-D")
        .arg("-e")
        .arg("-f")
        .arg(&config)
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|err| format!("sshd would not start: {err}"))?;

    let mut daemon = Sshd {
        child,
        port,
        key: private,
        user,
        home,
        _dir: dir,
    };
    // Wait for it to listen, and notice if it gave up instead.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if std::net::TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port))).is_ok() {
            return Ok(daemon);
        }
        if let Ok(Some(status)) = daemon.child.try_status() {
            let said = std::fs::read_to_string(&diagnostics).unwrap_or_default();
            return Err(format!(
                "sshd exited at once ({status}): {}",
                said.trim().replace('\n', "; ")
            ));
        }
        if Instant::now() >= deadline {
            return Err("sshd never listened".to_string());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `Child::try_wait` under a name that reads better above.
trait TryStatus {
    fn try_status(&mut self) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl TryStatus for Child {
    fn try_status(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.try_wait()
    }
}

/// An app with the daemon in its inventory as a device.
async fn app_for(daemon: &Sshd) -> (App, String) {
    let app = common::builder().build();
    let credential = app
        .rpc(
            "credentials.create",
            json!({"name": "sftp", "username": daemon.user, "private_key": daemon.key}),
        )
        .await
        .expect("credential");
    let device = app
        .rpc(
            "devices.create",
            json!({
                "name": "FILESRV",
                "address": "127.0.0.1",
                "port": daemon.port,
                "transport": "ssh",
                "credential_id": credential["id"],
            }),
        )
        .await
        .expect("device");
    let id = device["id"].as_str().expect("id").to_string();
    (app, id)
}

async fn open_sftp(app: &App, device_id: &str) -> Value {
    app.rpc("sftp.open", json!({"device_id": device_id}))
        .await
        .expect("sftp.open")
}

#[tokio::test]
async fn files_are_browsed_edited_and_transferred_over_sftp() {
    let daemon = match start_sshd() {
        Ok(daemon) => daemon,
        Err(why) => return skip(&why),
    };
    let (app, device_id) = app_for(&daemon).await;
    let session = open_sftp(&app, &device_id).await;
    let sftp_id = session["sftp_id"].as_str().expect("an id").to_string();
    assert!(
        session["home"].as_str().expect("a home").starts_with('/'),
        "{session}"
    );
    assert_eq!(session["device_id"], device_id.as_str());
    assert_eq!(session["device_name"], "FILESRV");

    let work = daemon.home.to_string_lossy().to_string();
    let directory = format!("{work}/configs");
    app.rpc("sftp.mkdir", json!({"sftp_id": sftp_id, "path": directory}))
        .await
        .expect("mkdir");

    // Write a file and read it back.
    let path = format!("{directory}/router.cfg");
    let text = "hostname R1\ninterface Gi0/1\n description uplink\n";
    let written = app
        .rpc(
            "sftp.write_text",
            json!({"sftp_id": sftp_id, "path": path, "text": text}),
        )
        .await
        .expect("write_text");
    assert_eq!(written["size"], text.len());
    let read = app
        .rpc("sftp.read_text", json!({"sftp_id": sftp_id, "path": path}))
        .await
        .expect("read_text");
    assert_eq!(read["text"], text);
    assert_eq!(read["size"], text.len());

    // Listing shows it, directories first.
    let listing = app
        .rpc("sftp.list", json!({"sftp_id": sftp_id, "path": directory}))
        .await
        .expect("list");
    let entries = listing["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "router.cfg");
    assert_eq!(entries[0]["is_dir"], false);
    assert_eq!(entries[0]["size"], text.len());

    let stat = app
        .rpc("sftp.stat", json!({"sftp_id": sftp_id, "path": directory}))
        .await
        .expect("stat");
    assert_eq!(stat["is_dir"], true);

    // Rename, then delete.
    let renamed = format!("{directory}/router.cfg.bak");
    app.rpc(
        "sftp.rename",
        json!({"sftp_id": sftp_id, "from": path, "to": renamed}),
    )
    .await
    .expect("rename");
    assert_eq!(
        app.rpc("sftp.stat", json!({"sftp_id": sftp_id, "path": path}))
            .await
            .unwrap_err()
            .code,
        "io"
    );

    // Download it to this machine, and upload it back under a new name.
    let local = daemon.home.join("downloaded.cfg");
    let job = app
        .rpc(
            "sftp.download",
            json!({"sftp_id": sftp_id, "remote": renamed, "local": local.to_string_lossy()}),
        )
        .await
        .expect("download");
    let finished = nocturne_app::jobs::wait(&app, job["job_id"].as_str().expect("job id"))
        .await
        .expect("the download");
    assert_eq!(
        finished.state,
        nocturne_app::jobs::JobState::Succeeded,
        "{finished:?}"
    );
    assert_eq!(
        finished.result.clone().expect("a result")["bytes"],
        text.len()
    );
    assert_eq!(std::fs::read_to_string(&local).expect("the file"), text);

    let uploaded = format!("{directory}/uploaded.cfg");
    let job = app
        .rpc(
            "sftp.upload",
            json!({"sftp_id": sftp_id, "remote": uploaded, "local": local.to_string_lossy()}),
        )
        .await
        .expect("upload");
    let finished = nocturne_app::jobs::wait(&app, job["job_id"].as_str().expect("job id"))
        .await
        .expect("the upload");
    assert_eq!(
        finished.state,
        nocturne_app::jobs::JobState::Succeeded,
        "{finished:?}"
    );
    assert_eq!(
        app.rpc(
            "sftp.read_text",
            json!({"sftp_id": sftp_id, "path": uploaded})
        )
        .await
        .expect("read_text")["text"],
        text
    );

    app.rpc("sftp.remove", json!({"sftp_id": sftp_id, "path": renamed}))
        .await
        .expect("remove");
    app.rpc("sftp.remove", json!({"sftp_id": sftp_id, "path": uploaded}))
        .await
        .expect("remove");
    app.rpc(
        "sftp.remove",
        json!({"sftp_id": sftp_id, "path": directory}),
    )
    .await
    .expect("remove the now empty directory");

    // Closing it really does close it.
    app.rpc("sftp.close", json!({"sftp_id": sftp_id}))
        .await
        .expect("close");
    assert_eq!(
        app.rpc("sftp.list", json!({"sftp_id": sftp_id, "path": work}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn what_sftp_will_not_do_it_says_plainly() {
    let daemon = match start_sshd() {
        Ok(daemon) => daemon,
        Err(why) => return skip(&why),
    };
    let (app, device_id) = app_for(&daemon).await;
    let session = open_sftp(&app, &device_id).await;
    let sftp_id = session["sftp_id"].as_str().expect("an id").to_string();
    let work = daemon.home.to_string_lossy().to_string();

    // A file that is not text, and one that is too large.
    let binary = format!("{work}/binary.bin");
    std::fs::write(&binary, [0xffu8, 0xfe, 0x00, 0x01]).expect("the file");
    let refused = app
        .rpc(
            "sftp.read_text",
            json!({"sftp_id": sftp_id, "path": binary}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "invalid");
    assert!(refused.message.contains("not UTF-8"), "{refused}");

    let large = format!("{work}/large.bin");
    std::fs::write(&large, vec![b'x'; 2 * 1024 * 1024]).expect("the file");
    let refused = app
        .rpc("sftp.read_text", json!({"sftp_id": sftp_id, "path": large}))
        .await
        .unwrap_err();
    assert_eq!(refused.code, "invalid");
    assert!(refused.message.contains("Download it instead"), "{refused}");

    let refused = app
        .rpc("sftp.read_text", json!({"sftp_id": sftp_id, "path": work}))
        .await
        .unwrap_err();
    assert!(refused.message.contains("directory"), "{refused}");

    // A relative local path means nothing to the person who asked.
    let refused = app
        .rpc(
            "sftp.download",
            json!({"sftp_id": sftp_id, "remote": binary, "local": "somewhere.bin"}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "invalid");
    assert!(refused.message.contains("absolute"), "{refused}");

    let refused = app
        .rpc(
            "sftp.upload",
            json!({"sftp_id": sftp_id, "remote": binary, "local": format!("{work}/no-such-file")}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "not_found");

    assert_eq!(
        app.rpc("sftp.list", json!({"sftp_id": "ghost", "path": "/"}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
    app.rpc("sftp.close", json!({"sftp_id": sftp_id}))
        .await
        .expect("close");
}

#[tokio::test]
async fn sftp_is_only_for_ssh_devices_and_only_where_files_may_be_touched() {
    // Needs no daemon: these are refused before anything is connected.
    let lab = common::lab(&["SW1"]).await;
    let refused = lab
        .app
        .rpc("sftp.open", json!({"device_id": lab.id("SW1")}))
        .await
        .unwrap_err();
    assert_eq!(refused.code, "invalid");
    assert!(refused.message.contains("part of SSH"), "{refused}");

    let remote = common::builder_remote().build();
    let credential = remote
        .rpc(
            "credentials.create",
            json!({"name": "ops", "username": "ops", "password": "x"}),
        )
        .await
        .expect("credential");
    // A port nothing listens on, so the attempt is answered at once.
    let port = free_port().expect("a free port");
    let device = remote
        .rpc(
            "devices.create",
            json!({
                "name": "SRV", "address": "127.0.0.1", "port": port,
                "transport": "ssh", "credential_id": credential["id"],
            }),
        )
        .await
        .expect("device");
    // Transfers touch this machine, so a server-mode backend will not do them.
    for method in ["sftp.download", "sftp.upload"] {
        let refused = remote
            .rpc(
                method,
                json!({"sftp_id": "whatever", "remote": "/tmp/x", "local": "/tmp/x"}),
            )
            .await
            .unwrap_err();
        assert_eq!(refused.code, "forbidden", "{method}");
    }
    // Nothing is listening: the point is that it was allowed to try. Which
    // way the host says so differs — refused where it answers, timed out
    // where it drops the connection, as Windows does.
    let tried = remote
        .rpc("sftp.open", json!({"device_id": device["id"]}))
        .await
        .unwrap_err();
    assert!(matches!(tried.code, "refused" | "timeout"), "{tried:?}");
}
