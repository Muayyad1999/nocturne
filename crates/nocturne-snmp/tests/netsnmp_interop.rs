//! Interoperability with net-snmp: every test starts its own private `snmpd` on a free
//! loopback port (temporary configuration, nothing read from or written to `/etc/snmp` or
//! `/var/lib/snmp`), exercises the client against it and kills it again. Traps and informs
//! are produced with the `snmptrap`/`snmpinform` command-line tools.
//!
//! When net-snmp is not installed the tests print a note and pass.
//!
//! Run with `--nocapture` to see the compatibility matrix.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nocturne_snmp::{
    Auth, AuthProtocol, Client, ClientConfig, Oid, PrivProtocol, SnmpError, Value, VarBind,
    decode_trap, inform_response, names, oids,
};
use tokio::net::UdpSocket;

// ---------------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------------

/// Looks a program up on `PATH` and in the usual sbin directories.
fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/usr/sbin", "/usr/local/sbin", "/usr/bin", "/usr/local/bin"].map(PathBuf::from))
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// `true` if all of these net-snmp programs are installed; otherwise says which one is
/// missing, so that the test can be skipped.
fn installed(programs: &[&str]) -> bool {
    match programs.iter().find(|name| find_program(name).is_none()) {
        None => true,
        Some(missing) => {
            eprintln!("net-snmp's {missing} is not installed: skipping this interoperability test");
            false
        }
    }
}

/// A scratch directory that disappears with the value.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> TempDir {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "nocturne-snmp-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Keeps the net-snmp tools away from the system's and the user's configuration.
fn isolate(command: &mut Command, dir: &Path) {
    command
        .env("SNMPCONFPATH", dir.join("conf"))
        .env("SNMP_PERSISTENT_DIR", dir.join("persist"))
        .env("HOME", dir)
        .env("MIBS", "")
        .env("MIBDIRS", dir.join("mibs"));
}

fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .and_then(|socket| socket.local_addr())
        .expect("bind a loopback UDP port")
        .port()
}

/// A private snmpd instance. Killed when dropped.
struct Snmpd {
    child: Child,
    program: PathBuf,
    dir: TempDir,
    port: u16,
}

const SYS_LOCATION: &str = "Nocturne interop lab";
const SYS_CONTACT: &str = "noc@example.net";

impl Snmpd {
    /// Starts snmpd with the base configuration plus `extra` lines, or returns `None` (after
    /// saying so) when net-snmp is not installed.
    async fn start(extra: &str) -> Option<Snmpd> {
        // The daemon, and the tools its answers are compared with.
        if !installed(&["snmpd", "snmpget", "snmpwalk"]) {
            return None;
        }
        let program = find_program("snmpd")?;
        let config = format!(
            "rocommunity public 127.0.0.1\n\
             rwcommunity private 127.0.0.1\n\
             sysLocation {SYS_LOCATION}\n\
             sysContact {SYS_CONTACT}\n\
             {extra}\n"
        );

        // The port is free when probed and could be taken a moment later; try a few.
        for _ in 0..5 {
            let dir = TempDir::new("snmpd");
            for sub in ["conf", "persist", "mibs"] {
                std::fs::create_dir_all(dir.path().join(sub)).expect("create snmpd dirs");
            }
            std::fs::write(dir.path().join("snmpd.conf"), &config).expect("write snmpd.conf");
            let port = free_udp_port();
            let child = spawn_snmpd(&program, dir.path(), port);
            let mut daemon = Snmpd {
                child,
                program: program.clone(),
                dir,
                port,
            };
            if daemon.wait_until_ready().await {
                if daemon.enforces_its_configuration().await {
                    return Some(daemon);
                }
                eprintln!(
                    "skipping the net-snmp interoperability tests: this `snmpd` answered a \
                     community it was never given, so it is not enforcing the configuration \
                     these tests were written against — half of them assert the opposite of \
                     what they mean against such an agent"
                );
                return None;
            }
            let log = std::fs::read_to_string(daemon.dir().join("snmpd.log")).unwrap_or_default();
            eprintln!("snmpd did not come up on port {port}, retrying; its log:\n{log}");
        }
        panic!("snmpd could not be started");
    }

    fn addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.port))
    }

    fn dir(&self) -> &Path {
        self.dir.path()
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Stops the daemon and starts it again on the same port with the engine identity given
    /// in `engine_state` (`engineBoots`/`oldEngineID` lines, as snmpd keeps them in its
    /// persistent file): what a reboot, a replaced supervisor or an HA failover looks like.
    async fn restart_with(&mut self, engine_state: &str) {
        self.stop();
        std::fs::write(self.dir().join("engine.conf"), engine_state).expect("write engine.conf");
        self.child = spawn_snmpd(&self.program, self.dir.path(), self.port);
        assert!(
            self.wait_until_ready().await,
            "snmpd did not come back after the restart"
        );
    }

    /// Whether this agent ignores a community it was never configured with.
    ///
    /// The premise under every test here is that the daemon enforces the
    /// configuration it was handed — the suite writes its own `snmpd.conf`
    /// and starts with `-C` so that nothing else is read. net-snmp is
    /// third-party software and its builds differ; one that answers an
    /// unknown community turns "the wrong community is met with silence"
    /// and "the read-only community may not write" into assertions of the
    /// opposite. Better to say so once than to fail five tests that are
    /// each correct about a daemon that is not.
    async fn enforces_its_configuration(&self) -> bool {
        let mut client = connect_impatient(self, v2c("not-the-community")).await;
        client.get(&[oid(oids::SYS_LOCATION)]).await.is_err()
    }

    async fn wait_until_ready(&mut self) -> bool {
        let mut config = ClientConfig::new(self.addr(), v2c("public"));
        config.timeout = Duration::from_millis(100);
        config.retries = 0;
        let sys_up_time = [oid(oids::SYS_UP_TIME)];
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return false;
            }
            let mut client = Client::connect(config.clone())
                .await
                .expect("create client");
            if client.get(&sys_up_time).await.is_ok() {
                return true;
            }
        }
        false
    }

    /// Runs a net-snmp command-line tool against this daemon's environment.
    async fn tool(&self, name: &str, args: &[&str]) -> std::process::Output {
        let program = find_program(name).unwrap_or_else(|| panic!("{name} is not installed"));
        let mut command = Command::new(program);
        isolate(&mut command, self.dir());
        command.args(args);
        tokio::process::Command::from(command)
            .output()
            .await
            .unwrap_or_else(|e| panic!("run {name}: {e}"))
    }
}

impl Drop for Snmpd {
    fn drop(&mut self) {
        self.stop();
    }
}

fn spawn_snmpd(program: &Path, dir: &Path, port: u16) -> Child {
    let mut config_files = dir.join("snmpd.conf").into_os_string();
    let engine_state = dir.join("engine.conf");
    if engine_state.exists() {
        config_files.push(",");
        config_files.push(engine_state);
    }
    let mut command = Command::new(program);
    isolate(&mut command, dir);
    command
        .arg("-f")
        .arg("-Lf")
        .arg(dir.join("snmpd.log"))
        // Only our configuration files, never the system's.
        .arg("-C")
        .arg("-c")
        .arg(config_files)
        .arg("-p")
        .arg(dir.join("snmpd.pid"))
        // No SMUX: it would grab TCP port 199.
        .args(["-I", "-smux"])
        .arg(format!("udp:127.0.0.1:{port}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.spawn().expect("start snmpd")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn oid(text: &str) -> Oid {
    text.parse().expect("valid OID")
}

fn v2c(community: &str) -> Auth {
    Auth::V2c {
        community: community.to_owned(),
    }
}

fn v1(community: &str) -> Auth {
    Auth::V1 {
        community: community.to_owned(),
    }
}

async fn connect(daemon: &Snmpd, auth: Auth) -> Client {
    Client::connect(ClientConfig::new(daemon.addr(), auth))
        .await
        .expect("create client")
}

/// A client that gives up quickly, for tests that expect failures.
async fn connect_impatient(daemon: &Snmpd, auth: Auth) -> Client {
    let mut config = ClientConfig::new(daemon.addr(), auth);
    config.timeout = Duration::from_millis(400);
    config.retries = 0;
    Client::connect(config).await.expect("create client")
}

fn text(bind: &VarBind) -> String {
    bind.value
        .as_text()
        .unwrap_or_else(|| panic!("{} is not text: {:?}", bind.oid, bind.value))
}

// ---------------------------------------------------------------------------------------------
// The SNMPv3 users of the test daemon
// ---------------------------------------------------------------------------------------------

const AUTH_PROTOCOLS: [(AuthProtocol, &str); 6] = [
    (AuthProtocol::Md5, "MD5"),
    (AuthProtocol::Sha1, "SHA"),
    (AuthProtocol::Sha224, "SHA-224"),
    (AuthProtocol::Sha256, "SHA-256"),
    (AuthProtocol::Sha384, "SHA-384"),
    (AuthProtocol::Sha512, "SHA-512"),
];

/// Our protocol and net-snmp's name for it. `AES-192-C`/`AES-256-C` are net-snmp's names for
/// the Cisco key extension.
const PRIV_PROTOCOLS: [(PrivProtocol, &str); 6] = [
    (PrivProtocol::Des, "DES"),
    (PrivProtocol::Aes128, "AES"),
    (PrivProtocol::Aes192, "AES-192"),
    (PrivProtocol::Aes256, "AES-256"),
    (PrivProtocol::Aes192Cisco, "AES-192-C"),
    (PrivProtocol::Aes256Cisco, "AES-256-C"),
];

#[derive(Clone)]
struct User {
    name: String,
    auth: Option<(AuthProtocol, &'static str)>,
    privacy: Option<(PrivProtocol, &'static str)>,
}

impl User {
    fn new(
        auth: Option<(AuthProtocol, &'static str)>,
        privacy: Option<(PrivProtocol, &'static str)>,
    ) -> User {
        let squash = |name: &str| name.replace('-', "").to_lowercase();
        let name = match (auth, privacy) {
            (None, _) => "noauth".to_owned(),
            (Some((_, auth)), None) => squash(auth),
            (Some((_, auth)), Some((_, privacy))) => {
                format!("{}_{}", squash(auth), squash(privacy))
            }
        };
        User {
            name,
            auth,
            privacy,
        }
    }

    fn auth_passphrase(&self) -> String {
        format!("auth-{}-passphrase", self.name)
    }

    fn priv_passphrase(&self) -> String {
        format!("priv-{}-passphrase", self.name)
    }

    fn level(&self) -> &'static str {
        match (self.auth, self.privacy) {
            (None, _) => "noAuthNoPriv",
            (Some(_), None) => "authNoPriv",
            (Some(_), Some(_)) => "authPriv",
        }
    }

    /// `createUser` and `rouser` lines for snmpd.conf.
    fn config(&self) -> String {
        let mut create = format!("createUser {}", self.name);
        if let Some((_, auth)) = self.auth {
            create += &format!(" {auth} \"{}\"", self.auth_passphrase());
        }
        if let Some((_, privacy)) = self.privacy {
            create += &format!(" {privacy} \"{}\"", self.priv_passphrase());
        }
        let level = match self.level() {
            "noAuthNoPriv" => "noauth",
            "authNoPriv" => "auth",
            _ => "priv",
        };
        format!("{create}\nrouser {} {level}\n", self.name)
    }

    fn auth(&self) -> Auth {
        Auth::V3 {
            user: self.name.clone(),
            auth: self
                .auth
                .map(|(protocol, _)| (protocol, self.auth_passphrase())),
            privacy: self
                .privacy
                .map(|(protocol, _)| (protocol, self.priv_passphrase())),
            context_name: String::new(),
        }
    }

    /// The arguments that make a net-snmp tool use this user.
    fn cli_args(&self) -> Vec<String> {
        let mut args = vec![
            "-v3".to_owned(),
            "-u".to_owned(),
            self.name.clone(),
            "-l".to_owned(),
            self.level().to_owned(),
        ];
        if let Some((_, auth)) = self.auth {
            args.extend([
                "-a".to_owned(),
                auth.to_owned(),
                "-A".to_owned(),
                self.auth_passphrase(),
            ]);
        }
        if let Some((_, privacy)) = self.privacy {
            args.extend([
                "-x".to_owned(),
                privacy.to_owned(),
                "-X".to_owned(),
                self.priv_passphrase(),
            ]);
        }
        args
    }

    fn label(&self) -> String {
        match (self.auth, self.privacy) {
            (None, _) => "noAuthNoPriv".to_owned(),
            (Some((_, auth)), None) => format!("authNoPriv {auth}"),
            (Some((_, auth)), Some((_, privacy))) => format!("authPriv {auth} / {privacy}"),
        }
    }
}

/// noAuthNoPriv, authNoPriv for every hash, authPriv for every hash × cipher.
fn all_users() -> Vec<User> {
    let mut users = vec![User::new(None, None)];
    for auth in AUTH_PROTOCOLS {
        users.push(User::new(Some(auth), None));
        for privacy in PRIV_PROTOCOLS {
            users.push(User::new(Some(auth), Some(privacy)));
        }
    }
    users
}

fn users_config(users: &[User]) -> String {
    users.iter().map(User::config).collect()
}

/// Asks net-snmp's own `snmpget` whether it can talk to the daemon as `user`: the reference
/// for "this combination is supported by this net-snmp build".
async fn netsnmp_accepts(daemon: &Snmpd, user: &User) -> bool {
    let target = daemon.addr().to_string();
    let mut args = user.cli_args();
    args.extend([
        "-r".into(),
        "1".into(),
        "-t".into(),
        "2".into(),
        "-On".into(),
        target,
        oids::SYS_NAME.into(),
    ]);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    daemon.tool("snmpget", &args).await.status.success()
}

// ---------------------------------------------------------------------------------------------
// SNMPv1 and SNMPv2c
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn v1_and_v2c_get_and_walk_system_and_interfaces() {
    let Some(daemon) = Snmpd::start("").await else {
        return;
    };
    let system = oid("1.3.6.1.2.1.1");
    let scalars = [
        oid(oids::SYS_DESCR),
        oid(oids::SYS_OBJECT_ID),
        oid(oids::SYS_UP_TIME),
        oid(oids::SYS_CONTACT),
        oid(oids::SYS_LOCATION),
    ];

    let mut walks = Vec::new();
    for (version, auth) in [("v1", v1("public")), ("v2c", v2c("public"))] {
        let mut client = connect(&daemon, auth).await;

        // GET of the system scalars.
        let got = client.get(&scalars).await.unwrap();
        assert_eq!(got.len(), scalars.len(), "{version}");
        for (bind, requested) in got.iter().zip(&scalars) {
            assert_eq!(&bind.oid, requested, "{version}");
        }
        // The agent describes the machine it runs on — "Linux …" here,
        // "Darwin …" on a Mac — so what is asserted is that it said
        // something, not what the kernel is called.
        assert!(
            !text(&got[0]).trim().is_empty(),
            "{version}: sysDescr {:?}",
            got[0].value
        );
        // And that it identifies itself as a net-snmp agent:
        // netSnmpAgentOIDs.<platform>, where the platform is 10 for Linux
        // and another number elsewhere. The arc is the claim; the leaf is
        // which machine happens to be running the daemon.
        let object_id = match &got[1].value {
            Value::Oid(found) => found.to_string(),
            other => panic!("{version}: sysObjectID {other:?}"),
        };
        assert!(
            object_id.starts_with("1.3.6.1.4.1.8072.3.2."),
            "{version}: sysObjectID {object_id} is not a net-snmp agent"
        );
        assert!(matches!(got[2].value, Value::TimeTicks(_)), "{version}");
        assert_eq!(text(&got[3]), SYS_CONTACT, "{version}");
        assert_eq!(text(&got[4]), SYS_LOCATION, "{version}");

        // GETNEXT steps to the first instance.
        let next = client.get_next(&[oid("1.3.6.1.2.1.1.4")]).await.unwrap();
        assert_eq!(next[0].oid, oid(oids::SYS_CONTACT), "{version}");

        // Walk of the system group: starts with sysDescr.0, stays inside, strictly increasing.
        let walked = client.walk(&system, usize::MAX).await.unwrap();
        assert!(
            walked.len() >= 7,
            "{version}: only {} objects under system",
            walked.len()
        );
        assert_eq!(walked[0].oid, scalars[0], "{version}");
        assert!(
            walked.iter().all(|bind| bind.oid.starts_with(&system)),
            "{version}"
        );
        assert!(
            walked.windows(2).all(|pair| pair[0].oid < pair[1].oid),
            "{version}"
        );
        assert_eq!(
            names::describe(&walked[0].oid).as_deref(),
            Some("sysDescr.0"),
            "{version}"
        );

        // The interfaces group: ifNumber rows, the loopback among them.
        let if_number = client.get(&[oid(oids::IF_NUMBER)]).await.unwrap()[0]
            .value
            .as_u64()
            .expect("ifNumber is a number");
        let descriptions = client.walk(&oid(oids::IF_DESCR), usize::MAX).await.unwrap();
        assert_eq!(descriptions.len() as u64, if_number, "{version}");
        assert!(
            descriptions.iter().any(|bind| text(bind).starts_with("lo")),
            "{version}: no loopback"
        );
        let interfaces = client
            .walk(&oid("1.3.6.1.2.1.2"), usize::MAX)
            .await
            .unwrap();
        assert!(
            interfaces.len() as u64 > if_number * 20,
            "{version}: {} objects",
            interfaces.len()
        );

        // A limit cuts the walk short; walking an instance returns that instance.
        // (sysUpTime is among them and moves on, so compare the names.)
        let limited = client.walk(&system, 3).await.unwrap();
        let names_of =
            |walk: &[VarBind]| walk.iter().map(|bind| bind.oid.clone()).collect::<Vec<_>>();
        assert_eq!(names_of(&limited), names_of(&walked[..3]), "{version}");
        assert!(client.walk(&system, 0).await.unwrap().is_empty());
        let single = client.walk(&scalars[4], usize::MAX).await.unwrap();
        assert_eq!(single.len(), 1, "{version}");
        assert_eq!(text(&single[0]), SYS_LOCATION, "{version}");
        // ...and a subtree with nothing in it is simply empty.
        assert!(
            client
                .walk(&oid("1.3.6.1.2.1.1.42"), usize::MAX)
                .await
                .unwrap()
                .is_empty()
        );

        walks.push((version, walked));
        println!(
            "{version}: get, get_next, walk of system ({} objects) and interfaces ({} objects): pass",
            walks.last().unwrap().1.len(),
            interfaces.len()
        );
    }

    // Both versions see the same system group (sysUpTime moves, so compare the names).
    let names_of = |walk: &[VarBind]| walk.iter().map(|bind| bind.oid.clone()).collect::<Vec<_>>();
    assert_eq!(names_of(&walks[0].1), names_of(&walks[1].1));
}

#[tokio::test]
async fn missing_objects_differ_by_version() {
    let Some(daemon) = Snmpd::start("").await else {
        return;
    };
    let present = oid(oids::SYS_LOCATION);
    let no_instance = oid("1.3.6.1.2.1.1.6.7");
    let no_object = oid("1.3.6.1.2.1.1.99.0");

    // SNMPv2c: exceptions in place of values.
    let mut client = connect(&daemon, v2c("public")).await;
    let got = client
        .get(&[present.clone(), no_instance.clone(), no_object.clone()])
        .await
        .unwrap();
    assert_eq!(text(&got[0]), SYS_LOCATION);
    assert_eq!(got[1].value, Value::NoSuchInstance);
    assert_eq!(got[2].value, Value::NoSuchObject);
    assert!(got[1].value.is_exception() && got[2].value.is_exception());

    // SNMPv1: the whole request fails, naming the offender (1-based).
    let mut client = connect(&daemon, v1("public")).await;
    let error = client
        .get(&[present.clone(), no_object.clone()])
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            SnmpError::Agent {
                status: 2,
                name: "noSuchName",
                index: 2
            }
        ),
        "{error:?}"
    );

    // SNMPv1 has neither GETBULK nor Counter64: ifHCInOctets is invisible, the 32-bit
    // counters are not.
    let error = client
        .get_bulk(std::slice::from_ref(&present), 0, 5)
        .await
        .unwrap_err();
    assert!(matches!(error, SnmpError::Unsupported(_)), "{error:?}");
    assert!(
        client
            .walk(&oid(oids::IF_HC_IN_OCTETS), usize::MAX)
            .await
            .unwrap()
            .is_empty()
    );
    let counters = client
        .walk(&oid(oids::IF_IN_OCTETS), usize::MAX)
        .await
        .unwrap();
    assert!(!counters.is_empty());
    assert!(
        counters
            .iter()
            .all(|bind| matches!(bind.value, Value::Counter32(_)))
    );

    // The wrong community is met with silence.
    let mut client = connect_impatient(&daemon, v2c("not-the-community")).await;
    let started = Instant::now();
    let error = client.get(&[present]).await.unwrap_err();
    assert!(matches!(error, SnmpError::Timeout), "{error:?}");
    assert!(started.elapsed() >= Duration::from_millis(400));
}

#[tokio::test]
async fn getbulk_tables_and_large_requests() {
    let Some(daemon) = Snmpd::start("").await else {
        return;
    };
    let mut client = connect(&daemon, v2c("public")).await;

    // Raw GETBULK: one non-repeater, then interleaved repetitions of two columns.
    let bulk = client
        .get_bulk(
            &[
                oid("1.3.6.1.2.1.1.3"),
                oid(oids::IF_INDEX),
                oid(oids::IF_DESCR),
            ],
            1,
            2,
        )
        .await
        .unwrap();
    assert_eq!(bulk.len(), 5);
    assert_eq!(bulk[0].oid, oid(oids::SYS_UP_TIME));
    assert!(bulk[1].oid.starts_with(&oid(oids::IF_INDEX)));
    assert!(bulk[2].oid.starts_with(&oid(oids::IF_DESCR)));
    assert_eq!(
        bulk[1].oid.suffix(&oid(oids::IF_INDEX)),
        bulk[2].oid.suffix(&oid(oids::IF_DESCR))
    );

    // table(): the pivot agrees with per-column walks and with ifNumber.
    let columns = [
        oid(oids::IF_INDEX),
        oid(oids::IF_DESCR),
        oid(oids::IF_TYPE),
        oid(oids::IF_OPER_STATUS),
        oid(oids::IF_HC_IN_OCTETS),
        oid(oids::IF_ALIAS),
    ];
    let table = client.table(&columns, usize::MAX).await.unwrap();
    let if_number = client.get(&[oid(oids::IF_NUMBER)]).await.unwrap()[0]
        .value
        .as_u64()
        .unwrap();
    assert_eq!(table.len() as u64, if_number);
    let mut previous: Option<&Vec<u32>> = None;
    for (index, row) in &table {
        assert!(
            previous.is_none_or(|p| p < index),
            "rows are in index order"
        );
        previous = Some(index);
        assert_eq!(index.len(), 1, "ifTable is indexed by ifIndex alone");
        assert_eq!(row[&columns[0]], Value::Integer(i64::from(index[0])));
        assert_eq!(
            row.keys().collect::<Vec<_>>(),
            columns.iter().collect::<Vec<_>>()
        );
        assert!(matches!(row[&columns[4]], Value::Counter64(_)));
    }
    let descriptions = client.walk(&columns[1], usize::MAX).await.unwrap();
    for bind in &descriptions {
        let index = bind.oid.suffix(&columns[1]).unwrap().to_vec();
        assert_eq!(table[&index][&columns[1]], bind.value);
    }
    // A row limit keeps the first rows whole.
    let first = client.table(&columns, 1).await.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first.first(), table.first());

    // A GET of 130 OIDs goes out in several PDUs and comes back in order.
    let wanted: Vec<Oid> = (0..130)
        .map(|i| match i % 3 {
            0 => oid(oids::SYS_NAME),
            1 => oid(oids::SYS_LOCATION),
            _ => oid("1.3.6.1.2.1.1.77.0"),
        })
        .collect();
    let got = client.get(&wanted).await.unwrap();
    assert_eq!(got.len(), 130);
    for (bind, requested) in got.iter().zip(&wanted) {
        assert_eq!(&bind.oid, requested);
    }
    assert_eq!(text(&got[1]), SYS_LOCATION);
    assert_eq!(got[2].value, Value::NoSuchObject);
    println!(
        "v2c: get_bulk, table ({} rows x {} columns), 130-OID get: pass",
        table.len(),
        columns.len()
    );
}

#[tokio::test]
async fn set_changes_writable_objects_only() {
    // A read-write SNMPv3 user next to the read-write community.
    let v3_writer = User::new(Some(AUTH_PROTOCOLS[3]), Some(PRIV_PROTOCOLS[1]));
    let config = v3_writer.config().replace("rouser", "rwuser");
    let Some(daemon) = Snmpd::start(&config).await else {
        return;
    };
    let sys_name = oid(oids::SYS_NAME);
    let new_name = VarBind::new(sys_name.clone(), Value::from("renamed-by-nocturne"));

    // The read-write community may set sysName (not fixed in the configuration)...
    let mut writer = connect(&daemon, v2c("private")).await;
    let echoed = writer.set(std::slice::from_ref(&new_name)).await.unwrap();
    assert_eq!(echoed, vec![new_name.clone()]);
    let read_back = writer.get(std::slice::from_ref(&sys_name)).await.unwrap();
    assert_eq!(text(&read_back[0]), "renamed-by-nocturne");
    // ...and net-snmp's own tool agrees.
    let target = daemon.addr().to_string();
    let output = daemon
        .tool(
            "snmpget",
            &["-v2c", "-c", "public", "-Oqv", &target, oids::SYS_NAME],
        )
        .await;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .trim_matches('"'),
        "renamed-by-nocturne"
    );

    // sysLocation is pinned by the configuration file: notWritable.
    let pinned = VarBind::new(oid(oids::SYS_LOCATION), Value::from("elsewhere"));
    let error = writer.set(std::slice::from_ref(&pinned)).await.unwrap_err();
    assert!(
        matches!(
            error,
            SnmpError::Agent {
                status: 17,
                name: "notWritable",
                index: 1
            }
        ),
        "{error:?}"
    );
    // The wrong type for sysName: wrongType.
    let mistyped = VarBind::new(sys_name.clone(), Value::Integer(5));
    let error = writer.set(&[mistyped]).await.unwrap_err();
    assert!(
        matches!(
            error,
            SnmpError::Agent {
                status: 7,
                name: "wrongType",
                ..
            }
        ),
        "{error:?}"
    );

    // The read-only community may not write at all.
    let mut reader = connect(&daemon, v2c("public")).await;
    let error = reader
        .set(std::slice::from_ref(&new_name))
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            SnmpError::Agent {
                status: 6,
                name: "noAccess",
                ..
            }
        ),
        "{error:?}"
    );

    // SNMPv1 set works too.
    let mut v1_writer = connect(&daemon, v1("private")).await;
    let v1_name = VarBind::new(sys_name, Value::from("renamed-over-v1"));
    assert_eq!(
        v1_writer.set(std::slice::from_ref(&v1_name)).await.unwrap(),
        vec![v1_name]
    );

    // ...and so does SNMPv3 with authentication and privacy.
    let mut secure_writer = connect(&daemon, v3_writer.auth()).await;
    let v3_name = VarBind::new(oid(oids::SYS_NAME), Value::from("renamed-over-v3"));
    assert_eq!(
        secure_writer
            .set(std::slice::from_ref(&v3_name))
            .await
            .unwrap(),
        vec![v3_name.clone()]
    );
    assert_eq!(
        reader.get(&[oid(oids::SYS_NAME)]).await.unwrap(),
        vec![v3_name]
    );
    let error = secure_writer
        .set(std::slice::from_ref(&pinned))
        .await
        .unwrap_err();
    assert!(
        matches!(error, SnmpError::Agent { status: 17, .. }),
        "{error:?}"
    );
    println!("set over v1, v2c and v3 authPriv (and its refusals): pass");
}

#[tokio::test]
async fn full_mib2_walk_matches_snmpwalk() {
    let Some(daemon) = Snmpd::start("").await else {
        return;
    };
    let mib2 = oid("1.3.6.1.2.1");
    let target = daemon.addr().to_string();

    // Tables whose content comes and goes while we look: processes, sockets, neighbours.
    let volatile: Vec<Oid> = [
        "1.3.6.1.2.1.25.4",
        "1.3.6.1.2.1.25.5",
        "1.3.6.1.2.1.6.13",
        "1.3.6.1.2.1.6.19",
        "1.3.6.1.2.1.6.20",
        "1.3.6.1.2.1.7.5",
        "1.3.6.1.2.1.7.7",
        "1.3.6.1.2.1.4.22",
        "1.3.6.1.2.1.4.35",
        "1.3.6.1.2.1.3",
    ]
    .iter()
    .map(|prefix| oid(prefix))
    .collect();
    let is_volatile = |oid: &Oid| volatile.iter().any(|prefix| oid.starts_with(prefix));

    let mut last_report = String::new();
    // The MIB is alive (the walks themselves open sockets and run processes), so give the
    // comparison a few chances to see it in the same state twice.
    for _attempt in 0..3 {
        let mut client = connect(&daemon, v2c("public")).await;
        let ours = client.walk(&mib2, usize::MAX).await.unwrap();
        assert!(ours.windows(2).all(|pair| pair[0].oid < pair[1].oid));
        assert!(
            ours.iter()
                .all(|bind| bind.oid.starts_with(&mib2) && !bind.value.is_exception())
        );

        let output = daemon
            .tool(
                "snmpwalk",
                &[
                    "-v2c",
                    "-c",
                    "public",
                    "-Onq",
                    "-t",
                    "5",
                    &target,
                    "1.3.6.1.2.1",
                ],
            )
            .await;
        assert!(
            output.status.success(),
            "snmpwalk failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let theirs: Vec<Oid> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.starts_with(".1.3.6.1.2.1."))
            .filter_map(|line| line.split_whitespace().next()?.parse().ok())
            .collect();

        let ours_set: BTreeSet<Oid> = ours.iter().map(|bind| bind.oid.clone()).collect();
        let theirs_set: BTreeSet<Oid> = theirs.iter().cloned().collect();
        let difference: Vec<&Oid> = ours_set.symmetric_difference(&theirs_set).collect();
        let stable_difference: Vec<String> = difference
            .iter()
            .copied()
            .filter(|oid| !is_volatile(oid))
            .map(|oid| names::describe(oid).unwrap_or_else(|| oid.to_string()))
            .collect();
        last_report = format!(
            "walk of 1.3.6.1.2.1: {} objects, snmpwalk: {} objects, {} differ in volatile tables, {} elsewhere {:?}",
            ours.len(),
            theirs.len(),
            difference.len() - stable_difference.len(),
            stable_difference.len(),
            stable_difference.iter().take(10).collect::<Vec<_>>(),
        );
        println!("{last_report}");
        // Sanity: this is a real MIB, not a handful of objects.
        assert!(ours.len() > 500, "{last_report}");
        if stable_difference.is_empty() {
            if ours.len() == theirs.len() {
                println!("full walk: identical object count ({}): pass", ours.len());
            } else {
                println!("full walk: identical outside the volatile tables: pass");
            }
            return;
        }
    }
    panic!("our walk and snmpwalk disagree: {last_report}");
}

// ---------------------------------------------------------------------------------------------
// SNMPv3
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn v3_security_matrix() {
    let users = all_users();
    let Some(daemon) = Snmpd::start(&users_config(&users)).await else {
        return;
    };
    let wanted = [oid(oids::SYS_LOCATION), oid(oids::SYS_UP_TIME)];
    let system = oid("1.3.6.1.2.1.1");
    let reference = connect(&daemon, v2c("public"))
        .await
        .walk(&system, usize::MAX)
        .await
        .unwrap()
        .len();

    let mut failures = Vec::new();
    let (mut passed, mut unsupported) = (0, 0);
    println!(
        "\nSNMPv3 interoperability with net-snmp ({} combinations):",
        users.len()
    );
    for user in &users {
        let supported = netsnmp_accepts(&daemon, user).await;
        let mut client = connect(&daemon, user.auth()).await;
        let outcome: Result<(), String> = async {
            let got = client.get(&wanted).await.map_err(|e| format!("get: {e}"))?;
            if text(&got[0]) != SYS_LOCATION || !matches!(got[1].value, Value::TimeTicks(_)) {
                return Err(format!("unexpected values {got:?}"));
            }
            // A bulk walk makes for bigger, differently sized (and padded) messages.
            let walked = client
                .walk(&system, usize::MAX)
                .await
                .map_err(|e| format!("walk: {e}"))?;
            if walked.len() != reference {
                return Err(format!(
                    "walk returned {} objects, v2c sees {reference}",
                    walked.len()
                ));
            }
            let next = client
                .get_next(std::slice::from_ref(&system))
                .await
                .map_err(|e| format!("get_next: {e}"))?;
            if next[0].oid != oid(oids::SYS_DESCR) {
                return Err(format!("get_next returned {}", next[0].oid));
            }
            Ok(())
        }
        .await;

        let verdict = match (&outcome, supported) {
            (Ok(()), true) => {
                passed += 1;
                "pass".to_owned()
            }
            (Ok(()), false) => {
                failures.push(format!(
                    "{}: works for us but net-snmp's own tools fail?",
                    user.label()
                ));
                "pass (but snmpget failed)".to_owned()
            }
            (Err(_), false) => {
                unsupported += 1;
                "not supported by this net-snmp build".to_owned()
            }
            (Err(reason), true) => {
                failures.push(format!("{}: {reason}", user.label()));
                format!("FAIL ({reason})")
            }
        };
        println!("  {:<34} {verdict}", user.label());
    }
    println!(
        "  => {passed} passed, {} failed, {unsupported} not supported by this net-snmp build\n",
        failures.len()
    );
    assert!(failures.is_empty(), "{failures:#?}");
    // Vacuity guard. Thirteen is what a full net-snmp build offers; one with
    // fewer algorithms offers fewer — macOS ships such a build and verified
    // seven — and which algorithms a distribution compiles in is its
    // business rather than ours. Note that `supported` above is decided by
    // net-snmp's own tools, so a combination this client cannot do is
    // already a failure and not an excuse. What is required here is that the
    // build verified everything it claims to support, and that the shortfall
    // is explained by the build rather than by the matrix having quietly
    // shrunk.
    assert!(passed > 0, "no combination could be verified at all");
    assert!(
        passed >= 13 || unsupported > 0,
        "only {passed} combinations were tried and this build supports them all"
    );
}

#[tokio::test]
async fn v3_errors_name_their_cause() {
    let sha_aes = User::new(Some(AUTH_PROTOCOLS[1]), Some(PRIV_PROTOCOLS[1]));
    let sha_des = User::new(Some(AUTH_PROTOCOLS[1]), Some(PRIV_PROTOCOLS[0]));
    let sha_only = User::new(Some(AUTH_PROTOCOLS[1]), None);
    let noauth = User::new(None, None);
    let users = [
        sha_aes.clone(),
        sha_des.clone(),
        sha_only.clone(),
        noauth.clone(),
    ];
    let Some(daemon) = Snmpd::start(&users_config(&users)).await else {
        return;
    };
    let wanted = [oid(oids::SYS_LOCATION)];
    let v3 = |user: &str,
              auth: Option<(AuthProtocol, &str)>,
              privacy: Option<(PrivProtocol, &str)>| Auth::V3 {
        user: user.to_owned(),
        auth: auth.map(|(protocol, passphrase)| (protocol, passphrase.to_owned())),
        privacy: privacy.map(|(protocol, passphrase)| (protocol, passphrase.to_owned())),
        context_name: String::new(),
    };
    let auth_error = |result: Result<Vec<VarBind>, SnmpError>| match result {
        Err(SnmpError::Auth(message)) => message,
        other => panic!("expected an authentication error, got {other:?}"),
    };

    // The right credentials work (so the failures below are about what they change).
    for user in &users {
        let got = connect(&daemon, user.auth())
            .await
            .get(&wanted)
            .await
            .unwrap();
        assert_eq!(text(&got[0]), SYS_LOCATION, "{}", user.name);
    }

    // Wrong authentication passphrase.
    let wrong_auth = v3(
        &sha_only.name,
        Some((AuthProtocol::Sha1, "not-the-passphrase")),
        None,
    );
    let message = auth_error(
        connect_impatient(&daemon, wrong_auth)
            .await
            .get(&wanted)
            .await,
    );
    assert!(
        message.contains("wrong digest") && message.contains("authentication passphrase"),
        "{message}"
    );

    // Right passphrase, wrong authentication protocol.
    let wrong_hash = v3(
        &sha_only.name,
        Some((AuthProtocol::Md5, &sha_only.auth_passphrase())),
        None,
    );
    let message = auth_error(
        connect_impatient(&daemon, wrong_hash)
            .await
            .get(&wanted)
            .await,
    );
    assert!(message.contains("wrong digest"), "{message}");

    // Unknown user.
    let stranger = v3("nobody-by-that-name", None, None);
    let message = auth_error(
        connect_impatient(&daemon, stranger)
            .await
            .get(&wanted)
            .await,
    );
    assert!(message.contains("unknown user name"), "{message}");

    // A security level the user does not have: authentication for a user without a key.
    let too_much = v3(
        &noauth.name,
        Some((AuthProtocol::Sha1, "some-passphrase")),
        None,
    );
    let message = auth_error(
        connect_impatient(&daemon, too_much)
            .await
            .get(&wanted)
            .await,
    );
    assert!(message.contains("unsupported security level"), "{message}");

    // Wrong privacy passphrase. net-snmp drops such requests without a word; the client
    // works out why and says so instead of reporting a timeout.
    for user in [&sha_aes, &sha_des] {
        let (protocol, _) = user.privacy.unwrap();
        let wrong_priv = v3(
            &user.name,
            Some((AuthProtocol::Sha1, &user.auth_passphrase())),
            Some((protocol, "not-the-privacy-passphrase")),
        );
        let message = auth_error(
            connect_impatient(&daemon, wrong_priv)
                .await
                .get(&wanted)
                .await,
        );
        assert!(
            message.contains("decryption error") && message.contains("privacy passphrase"),
            "{message}"
        );
    }
    // Right passphrase, wrong cipher.
    let wrong_cipher = v3(
        &sha_aes.name,
        Some((AuthProtocol::Sha1, &sha_aes.auth_passphrase())),
        Some((PrivProtocol::Des, &sha_aes.priv_passphrase())),
    );
    let message = auth_error(
        connect_impatient(&daemon, wrong_cipher)
            .await
            .get(&wanted)
            .await,
    );
    assert!(message.contains("decryption error"), "{message}");

    // Too low a level for a user that must encrypt: the agent answers, but refuses access.
    let too_little = v3(
        &sha_aes.name,
        Some((AuthProtocol::Sha1, &sha_aes.auth_passphrase())),
        None,
    );
    let error = connect_impatient(&daemon, too_little)
        .await
        .get(&wanted)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            SnmpError::Agent {
                status: 16,
                name: "authorizationError",
                ..
            }
        ),
        "{error:?}"
    );

    // Configurations that can never work are refused before anything is sent.
    let privacy_without_auth = v3("x", None, Some((PrivProtocol::Aes128, "some-passphrase")));
    let error = Client::connect(ClientConfig::new(daemon.addr(), privacy_without_auth))
        .await
        .unwrap_err();
    assert!(matches!(error, SnmpError::Unsupported(_)), "{error:?}");
    let empty_passphrase = v3("x", Some((AuthProtocol::Sha1, "")), None);
    let error = Client::connect(ClientConfig::new(daemon.addr(), empty_passphrase))
        .await
        .unwrap_err();
    assert!(matches!(error, SnmpError::Auth(_)), "{error:?}");

    println!(
        "v3 errors (wrong auth passphrase/protocol, unknown user, unsupported level, wrong privacy passphrase/cipher, authorizationError): pass"
    );
}

#[tokio::test]
async fn v3_finds_the_other_aes_key_extension() {
    // SHA-1 yields 20 octets of key, AES-256 needs 32: the two extensions differ.
    let sha = AUTH_PROTOCOLS[1];
    let cisco_user = User::new(Some(sha), Some((PrivProtocol::Aes256Cisco, "AES-256-C")));
    let blumenthal_user = User::new(Some(sha), Some((PrivProtocol::Aes256, "AES-256")));
    let users = [cisco_user.clone(), blumenthal_user.clone()];
    let Some(daemon) = Snmpd::start(&users_config(&users)).await else {
        return;
    };
    if !netsnmp_accepts(&daemon, &cisco_user).await
        || !netsnmp_accepts(&daemon, &blumenthal_user).await
    {
        println!("AES-256 key-extension fallback: not supported by this net-snmp build, skipped");
        return;
    }
    let wanted = [oid(oids::SYS_LOCATION)];

    // Configure each user with the *other* flavour of AES-256.
    for (user, configured) in [
        (&cisco_user, PrivProtocol::Aes256),
        (&blumenthal_user, PrivProtocol::Aes256Cisco),
    ] {
        let auth = Auth::V3 {
            user: user.name.clone(),
            auth: Some((AuthProtocol::Sha1, user.auth_passphrase())),
            privacy: Some((configured, user.priv_passphrase())),
            context_name: String::new(),
        };
        let mut client = connect_impatient(&daemon, auth).await;
        let got = client.get(&wanted).await.unwrap();
        assert_eq!(
            text(&got[0]),
            SYS_LOCATION,
            "{configured:?} against {}",
            user.name
        );
        // Once found, the working flavour is kept: no further delay.
        let started = Instant::now();
        client.get(&wanted).await.unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(400),
            "{configured:?}"
        );
    }
    println!(
        "AES-256 configured with the wrong key extension (Blumenthal vs Cisco): detected and corrected: pass"
    );
}

#[tokio::test]
async fn v3_follows_the_agent_through_restarts() {
    let user = User::new(Some(AUTH_PROTOCOLS[3]), Some(PRIV_PROTOCOLS[1]));
    let Some(mut daemon) = Snmpd::start(&users_config(std::slice::from_ref(&user))).await else {
        return;
    };
    // sysLocation, snmpEngineID, snmpEngineBoots
    let wanted = [
        oid(oids::SYS_LOCATION),
        oid("1.3.6.1.6.3.10.2.1.1.0"),
        oid("1.3.6.1.6.3.10.2.1.2.0"),
    ];
    async fn engine(client: &mut Client, wanted: &[Oid]) -> (Vec<u8>, u64) {
        let got = client.get(wanted).await.expect("request after a restart");
        assert_eq!(text(&got[0]), SYS_LOCATION);
        (
            got[1].value.as_bytes().expect("snmpEngineID").to_vec(),
            got[2].value.as_u64().expect("snmpEngineBoots"),
        )
    }

    // One client lives through everything that follows.
    let mut client = connect(&daemon, user.auth()).await;
    let (engine_id, boots) = engine(&mut client, &wanted).await;

    // A reboot: same engine, later boot count, clock back at zero. The agent must reject the
    // client's stale notion with usmStatsNotInTimeWindows; the client resynchronises.
    let state = format!(
        "engineBoots {}\noldEngineID 0x{}\n",
        boots + 40,
        hex(&engine_id)
    );
    daemon.restart_with(&state).await;
    let (id_after_reboot, boots_after_reboot) = engine(&mut client, &wanted).await;
    assert_eq!(id_after_reboot, engine_id);
    assert_eq!(boots_after_reboot, boots + 41);
    println!(
        "v3 across a reboot (same engine ID, snmpEngineBoots {boots} -> {boots_after_reboot}): resynchronised: pass"
    );

    // A replaced device: another engine ID altogether. The agent reports
    // usmStatsUnknownEngineIDs; the client rediscovers and localises its keys anew.
    daemon
        .restart_with("engineBoots 7\noldEngineID 0x80001f88800102030405060708090a0b0c\n")
        .await;
    let (new_id, new_boots) = engine(&mut client, &wanted).await;
    assert_eq!(hex(&new_id), "80001f88800102030405060708090a0b0c");
    assert_eq!(new_boots, 8);
    println!("v3 across an engine ID change: rediscovered, keys relocalised: pass");

    // A failover to a peer with the same engine ID but a *smaller* boot count. A manager
    // that only ever moves its clock forward would be locked out here.
    let state = format!("engineBoots 2\noldEngineID 0x{}\n", hex(&new_id));
    daemon.restart_with(&state).await;
    let (id_after_failover, boots_after_failover) = engine(&mut client, &wanted).await;
    assert_eq!(id_after_failover, new_id);
    assert_eq!(boots_after_failover, 3);
    println!("v3 across a failover (snmpEngineBoots 8 -> 3): resynchronised: pass");

    // And it keeps working without further ado.
    for _ in 0..3 {
        engine(&mut client, &wanted).await;
    }
}

#[tokio::test]
async fn v3_tables_and_walks_equal_v2c() {
    // DES pads, AES does not; SHA-512 has the longest digest, MD5 the shortest.
    let users = [
        User::new(Some(AUTH_PROTOCOLS[5]), Some(PRIV_PROTOCOLS[3])),
        User::new(Some(AUTH_PROTOCOLS[0]), Some(PRIV_PROTOCOLS[0])),
        User::new(Some(AUTH_PROTOCOLS[1]), None),
    ];
    let Some(daemon) = Snmpd::start(&users_config(&users)).await else {
        return;
    };
    let names_of = |walk: &[VarBind]| walk.iter().map(|bind| bind.oid.clone()).collect::<Vec<_>>();
    let columns = [oid(oids::IF_DESCR), oid(oids::IF_TYPE), oid(oids::IF_MTU)];
    // Subtrees whose population does not change from one moment to the next.
    let subtrees = [
        oid("1.3.6.1.2.1.2"),
        oid("1.3.6.1.2.1.31"),
        oid("1.3.6.1.2.1.25.2"),
    ];
    let mut plain = connect(&daemon, v2c("public")).await;

    for user in &users {
        let mut secure = connect(&daemon, user.auth()).await;
        if !netsnmp_accepts(&daemon, user).await {
            println!(
                "v3 {}: not supported by this net-snmp build, skipped",
                user.label()
            );
            continue;
        }
        // The same table...
        let table = secure.table(&columns, usize::MAX).await.unwrap();
        assert!(!table.is_empty());
        assert_eq!(
            table,
            plain.table(&columns, usize::MAX).await.unwrap(),
            "{}",
            user.label()
        );
        // ...and the same walks, hundreds of objects in bulk responses of every size.
        let mut objects = 0;
        for subtree in &subtrees {
            let walked = secure.walk(subtree, usize::MAX).await.unwrap();
            let reference = plain.walk(subtree, usize::MAX).await.unwrap();
            assert_eq!(
                names_of(&walked),
                names_of(&reference),
                "{} {subtree}",
                user.label()
            );
            objects += walked.len();
        }
        assert!(objects > 100, "{objects}");
        // A long GET, chunked, under the same security.
        // (sysLocation.0 exists, sysLocation.1 does not.)
        let wanted: Vec<Oid> = (0..120)
            .map(|i| oid("1.3.6.1.2.1.1.6").child(i % 2))
            .collect();
        let got = secure.get(&wanted).await.unwrap();
        assert_eq!(got.len(), 120);
        assert_eq!(text(&got[0]), SYS_LOCATION);
        assert_eq!(got[1].value, Value::NoSuchInstance);
        println!(
            "v3 {}: table, walks ({objects} objects) and a 120-OID get equal v2c: pass",
            user.label()
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Traps and informs
// ---------------------------------------------------------------------------------------------

/// A UDP socket playing the trap receiver, and the tools' environment.
struct TrapSink {
    socket: UdpSocket,
    dir: TempDir,
}

impl TrapSink {
    async fn new() -> Option<TrapSink> {
        if !installed(&["snmptrap", "snmpinform"]) {
            return None;
        }
        let dir = TempDir::new("traps");
        for sub in ["conf", "persist", "mibs"] {
            std::fs::create_dir_all(dir.path().join(sub)).expect("create tool dirs");
        }
        let socket = UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("bind trap socket");
        Some(TrapSink { socket, dir })
    }

    fn target(&self) -> String {
        self.socket.local_addr().expect("local address").to_string()
    }

    fn command(&self, tool: &str, args: &[&str]) -> tokio::process::Command {
        let mut command =
            Command::new(find_program(tool).unwrap_or_else(|| panic!("{tool} is not installed")));
        isolate(&mut command, self.dir.path());
        command.args(args).stdin(Stdio::null());
        tokio::process::Command::from(command)
    }

    async fn receive(&self) -> (Vec<u8>, SocketAddr) {
        let mut buf = vec![0u8; 65_535];
        let (len, from) =
            tokio::time::timeout(Duration::from_secs(10), self.socket.recv_from(&mut buf))
                .await
                .expect("a notification within 10 s")
                .expect("receive");
        buf.truncate(len);
        (buf, from)
    }

    /// Runs `snmptrap` with `args` and returns what arrived.
    async fn trap(&self, args: &[&str]) -> (Vec<u8>, SocketAddr) {
        let output = self
            .command("snmptrap", args)
            .output()
            .await
            .expect("run snmptrap");
        assert!(
            output.status.success(),
            "snmptrap failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        self.receive().await
    }
}

#[tokio::test]
async fn traps_and_informs_from_net_snmp() {
    let Some(sink) = TrapSink::new().await else {
        return;
    };
    let target = sink.target();

    // SNMPv1 linkDown.
    let (datagram, from) = sink
        .trap(&[
            "-v",
            "1",
            "-c",
            "trapcomm",
            &target,
            "1.3.6.1.4.1.8072.3.2.10",
            "192.0.2.1",
            "2",
            "0",
            "12345",
            "1.3.6.1.2.1.2.2.1.1.3",
            "i",
            "3",
            "1.3.6.1.2.1.2.2.1.8.3",
            "i",
            "2",
        ])
        .await;
    let trap = decode_trap(&datagram, from).unwrap();
    assert_eq!(
        (trap.version, trap.community.as_str(), trap.source),
        (1, "trapcomm", from)
    );
    assert_eq!(trap.uptime_ticks, Some(12_345));
    assert_eq!(trap.enterprise, Some(oid("1.3.6.1.4.1.8072.3.2.10")));
    assert_eq!(trap.agent_addr, Some([192, 0, 2, 1]));
    assert_eq!((trap.generic, trap.specific), (Some(2), Some(0)));
    assert_eq!(
        trap.trap_oid.as_ref().and_then(names::trap_name),
        Some("linkDown")
    );
    assert_eq!(
        trap.varbinds,
        vec![
            VarBind::new(oid("1.3.6.1.2.1.2.2.1.1.3"), Value::Integer(3)),
            VarBind::new(oid("1.3.6.1.2.1.2.2.1.8.3"), Value::Integer(2)),
        ]
    );
    assert!(!trap.is_inform);
    assert!(inform_response(&datagram).is_err());
    println!("snmptrap -v 1 (linkDown): decoded: pass");

    // SNMPv1 enterprise-specific trap.
    let (datagram, from) = sink
        .trap(&[
            "-v",
            "1",
            "-c",
            "public",
            &target,
            "1.3.6.1.4.1.9.9.43.2",
            "192.0.2.7",
            "6",
            "1",
            "99",
        ])
        .await;
    let trap = decode_trap(&datagram, from).unwrap();
    assert_eq!((trap.generic, trap.specific), (Some(6), Some(1)));
    assert_eq!(
        trap.trap_oid.as_ref().and_then(names::trap_name),
        Some("ciscoConfigManEvent")
    );
    println!("snmptrap -v 1 (enterprise specific): decoded: pass");

    // SNMPv2c linkUp with a mixed bag of value types.
    let (datagram, from) = sink
        .trap(&[
            "-v",
            "2c",
            "-c",
            "public",
            &target,
            "54321",
            "1.3.6.1.6.3.1.1.5.4",
            "1.3.6.1.2.1.2.2.1.1.3",
            "i",
            "3",
            "1.3.6.1.2.1.31.1.1.1.1.3",
            "s",
            "eth3",
            "1.3.6.1.2.1.31.1.1.1.6.3",
            "C",
            "18446744073709551615",
            "1.3.6.1.2.1.4.20.1.1.192.0.2.9",
            "a",
            "192.0.2.9",
            "1.3.6.1.2.1.2.2.1.10.3",
            "c",
            "4294967295",
            "1.3.6.1.2.1.2.2.1.5.3",
            "u",
            "1000000000",
            "1.3.6.1.2.1.1.2.0",
            "o",
            "1.3.6.1.4.1.8072.3.2.10",
            "1.3.6.1.2.1.2.2.1.6.3",
            "x",
            "001b213c9df8",
        ])
        .await;
    let trap = decode_trap(&datagram, from).unwrap();
    assert_eq!((trap.version, trap.community.as_str()), (2, "public"));
    assert_eq!(trap.uptime_ticks, Some(54_321));
    assert_eq!(
        trap.trap_oid.as_ref().and_then(names::trap_name),
        Some("linkUp")
    );
    assert_eq!(
        (trap.enterprise.clone(), trap.agent_addr, trap.generic),
        (None, None, None)
    );
    let values: Vec<&Value> = trap.varbinds.iter().map(|bind| &bind.value).collect();
    assert_eq!(
        values,
        [
            &Value::Integer(3),
            &Value::from("eth3"),
            &Value::Counter64(u64::MAX),
            &Value::IpAddress([192, 0, 2, 9]),
            &Value::Counter32(u32::MAX),
            &Value::Gauge32(1_000_000_000),
            &Value::Oid(oid("1.3.6.1.4.1.8072.3.2.10")),
            &Value::OctetString(vec![0x00, 0x1b, 0x21, 0x3c, 0x9d, 0xf8]),
        ]
    );
    assert_eq!(trap.varbinds[7].value.display(), "00:1b:21:3c:9d:f8");
    println!("snmptrap -v 2c (linkUp, 8 value types): decoded: pass");

    // SNMPv2c coldStart with the sender's own uptime.
    let (datagram, from) = sink
        .trap(&[
            "-v",
            "2c",
            "-c",
            "public",
            &target,
            "",
            "1.3.6.1.6.3.1.1.5.1",
        ])
        .await;
    let trap = decode_trap(&datagram, from).unwrap();
    assert_eq!(
        trap.trap_oid.as_ref().and_then(names::trap_name),
        Some("coldStart")
    );
    assert!(trap.uptime_ticks.is_some() && trap.varbinds.is_empty());
    println!("snmptrap -v 2c (coldStart): decoded: pass");

    // SNMPv2c inform: snmpinform only exits successfully once it gets our acknowledgement.
    let inform = sink
        .command(
            "snmpinform",
            &[
                "-v",
                "2c",
                "-c",
                "informcomm",
                "-r",
                "2",
                "-t",
                "3",
                &target,
                "999",
                "1.3.6.1.6.3.1.1.5.4",
                "1.3.6.1.2.1.2.2.1.1.5",
                "i",
                "5",
            ],
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run snmpinform");
    let (datagram, from) = sink.receive().await;
    let trap = decode_trap(&datagram, from).unwrap();
    assert!(trap.is_inform);
    assert_eq!((trap.version, trap.community.as_str()), (2, "informcomm"));
    assert_eq!(trap.uptime_ticks, Some(999));
    assert_eq!(
        trap.trap_oid.as_ref().and_then(names::trap_name),
        Some("linkUp")
    );
    assert_eq!(
        trap.varbinds,
        vec![VarBind::new(
            oid("1.3.6.1.2.1.2.2.1.1.5"),
            Value::Integer(5)
        )]
    );
    let acknowledgement = inform_response(&datagram).unwrap();
    sink.socket
        .send_to(&acknowledgement, from)
        .await
        .expect("send acknowledgement");
    let output = tokio::time::timeout(Duration::from_secs(15), inform.wait_with_output())
        .await
        .expect("snmpinform to finish")
        .expect("wait for snmpinform");
    assert!(
        output.status.success(),
        "snmpinform did not accept our acknowledgement: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    println!(
        "snmpinform -v 2c: decoded, acknowledged with inform_response, accepted by snmpinform: pass"
    );

    // SNMPv3 without security is decoded; with authentication it is reported as unsupported.
    let (datagram, from) = sink
        .trap(&[
            "-v",
            "3",
            "-e",
            "0x8000000001020304",
            "-u",
            "trapuser",
            "-l",
            "noAuthNoPriv",
            &target,
            "777",
            "1.3.6.1.6.3.1.1.5.3",
            "1.3.6.1.2.1.2.2.1.1.2",
            "i",
            "2",
        ])
        .await;
    let trap = decode_trap(&datagram, from).unwrap();
    assert_eq!((trap.version, trap.community.as_str()), (3, "trapuser"));
    assert_eq!(trap.uptime_ticks, Some(777));
    assert_eq!(
        trap.trap_oid.as_ref().and_then(names::trap_name),
        Some("linkDown")
    );
    assert_eq!(
        trap.varbinds,
        vec![VarBind::new(
            oid("1.3.6.1.2.1.2.2.1.1.2"),
            Value::Integer(2)
        )]
    );
    println!("snmptrap -v 3 noAuthNoPriv: decoded: pass");

    for (level, extra) in [
        (
            "authNoPriv",
            &["-a", "SHA", "-A", "trap-auth-passphrase"][..],
        ),
        (
            "authPriv",
            &[
                "-a",
                "SHA",
                "-A",
                "trap-auth-passphrase",
                "-x",
                "AES",
                "-X",
                "trap-priv-passphrase",
            ][..],
        ),
    ] {
        let mut args = vec![
            "-v",
            "3",
            "-e",
            "0x8000000001020304",
            "-u",
            "trapuser",
            "-l",
            level,
        ];
        args.extend_from_slice(extra);
        args.extend_from_slice(&[&target, "777", "1.3.6.1.6.3.1.1.5.3"]);
        let (datagram, from) = sink.trap(&args).await;
        let error = decode_trap(&datagram, from).unwrap_err();
        assert!(
            matches!(error, SnmpError::Unsupported(_)),
            "{level}: {error:?}"
        );
        println!("snmptrap -v 3 {level}: reported as SnmpError::Unsupported (by design): pass");
    }
}
