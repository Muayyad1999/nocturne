//! `show` commands about the device itself: identity, clock, sessions, logs,
//! health.

use std::fmt::Write as _;

use crate::api::SimTime;
use crate::catalog::Role;
use crate::device::cli::LineId;
use crate::device::config::Timestamps;
use crate::device::ios::{Cx, Mode, runcfg};
use crate::ifname::{IfId, IfKind};
use crate::sim::fx::Fx;
use crate::sim::node::Node;
use crate::util::clock::{DateTime, hms, uptime_long};

/// `flash:nvos-l3-universalk9.15.9-3.M.bin`
fn image_file(node: &Node) -> String {
    let version = node.model.version.replace('(', "-").replace(')', ".");
    format!("{}.{version}.bin", node.model.image.to_ascii_lowercase())
}

pub(crate) fn version(node: &Node, now: SimTime) -> String {
    let model = node.model;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Nocturne Virtual Software ({}), Version {}, RELEASE SOFTWARE (fc1)",
        model.image, model.version
    );
    out.push_str("Lab device simulated by Nocturne - not a vendor image\n");
    out.push_str("ROM: Bootstrap program is NVOS boot loader\n\n");
    let _ = writeln!(out, "{} uptime is {}", node.hostname(), uptime_long(node.uptime(now)));
    out.push_str("System returned to ROM by power-on\n");
    let _ = writeln!(out, "System image file is \"flash:{}\"\n", image_file(node));
    let (main_k, io_k) = if model.role == Role::Router { (524_288, 65_536) } else { (262_144, 32_768) };
    let _ = writeln!(out, "nocturne {} (revision 1.0) with {main_k}K/{io_k}K bytes of memory.", model.platform);
    let _ = writeln!(out, "Processor board ID {}", node.serial);
    let count = |kind: IfKind| node.cfg.interfaces.keys().filter(|id| id.kind == kind && !id.is_sub()).count();
    for (kind, label) in [
        (IfKind::Vlan, "Virtual Ethernet"),
        (IfKind::FastEthernet, "FastEthernet"),
        (IfKind::GigabitEthernet, "Gigabit Ethernet"),
        (IfKind::TenGigabitEthernet, "Ten Gigabit Ethernet"),
    ] {
        match count(kind) {
            0 => {}
            1 => {
                let _ = writeln!(out, "1 {label} interface");
            }
            n => {
                let _ = writeln!(out, "{n} {label} interfaces");
            }
        }
    }
    out.push_str("255K bytes of non-volatile configuration memory.\n");
    out.push_str("250880K bytes of flash memory at flash:.\n\n");
    if model.is_switch() {
        let _ = writeln!(out, "Base ethernet MAC Address       : {}", node.base_mac.dashed().replace('-', ":"));
        let _ = writeln!(out, "Model number                    : {}", model.platform);
        let _ = writeln!(out, "System serial number            : {}\n", node.serial);
    }
    out.push_str("Configuration register is 0x2102\n\n");
    out
}

pub(crate) fn inventory(node: &Node) -> String {
    let model = node.model;
    let mut out = String::new();
    let _ = writeln!(out, "NAME: \"Chassis\", DESCR: \"Nocturne {} chassis\"", model.platform);
    let _ = writeln!(out, "PID: {:<18}, VID: V01  , SN: {}\n", model.platform, node.serial);
    out
}

pub(crate) fn clock(node: &Node, now: SimTime) -> String {
    let (zone, offset_ms) = match &node.cfg.clock_timezone {
        Some(tz) => (tz.name.clone(), (i64::from(tz.hours) * 60 + i64::from(tz.hours.signum()) * i64::from(tz.minutes)) * 60_000),
        None => ("UTC".to_string(), 0),
    };
    // An asterisk marks a clock nobody has set.
    let mark = if node.clock_offset_ms.is_none() { "*" } else { "" };
    format!("{mark}{}\n", DateTime::from_unix_ms(node.clock_ms(now) + offset_ms).show_clock(&zone))
}

fn line_number(line: LineId) -> u32 {
    match line {
        LineId::Console => 0,
        LineId::Vty(n) => u32::from(n) + 1,
    }
}

pub(crate) fn users(cx: &Cx) -> String {
    let mut out = String::from("    Line       User       Host(s)              Idle       Location\n");
    let mut rows: Vec<(LineId, String, u64, String)> = cx
        .n
        .sessions
        .iter()
        .map(|s| {
            let idle = cx.fx.now.saturating_sub(s.last_input) / 1_000_000;
            (s.line, s.user.clone(), idle, s.peer.map(|p| p.to_string()).unwrap_or_default())
        })
        .collect();
    if !rows.iter().any(|(line, ..)| *line == cx.sh.line) {
        rows.push((cx.sh.line, cx.sh.user().to_string(), 0, cx.sh.peer.map(|p| p.to_string()).unwrap_or_default()));
    }
    rows.sort_by_key(|(line, ..)| line_number(*line));
    for (line, user, idle, location) in rows {
        let mark = if line == cx.sh.line { "*" } else { " " };
        let (kind, index) = match line {
            LineId::Console => ("con", 0),
            LineId::Vty(n) => ("vty", n),
        };
        let row = format!(
            "{mark}{:>3} {kind} {:<6}{:<11}{:<21}{:<11}{location}",
            line_number(line),
            index,
            user,
            "idle",
            hms(idle)
        );
        out.push_str(row.trim_end());
        out.push('\n');
    }
    out.push_str("\n  Interface    User               Mode         Idle     Peer Address\n\n");
    out
}

pub(crate) fn history(cx: &Cx) -> String {
    let mut out = String::new();
    for line in &cx.sh.history {
        let _ = writeln!(out, "  {line}");
    }
    out
}

pub(crate) fn flash(node: &Node, now: SimTime) -> String {
    let stamp = |micros: SimTime| {
        let t = DateTime::from_unix_ms(node.clock_ms(micros));
        format!("{} {} {} {:02}:{:02}:{:02} +00:00", t.month_name(), t.day, t.year, t.hour, t.min, t.sec)
    };
    let mut files: Vec<(u64, String, String)> = vec![(52_428_800, stamp(node.boot_time), image_file(node))];
    if let Some(vlans) = &node.vlan_dat {
        files.push((616 + 36 * vlans.len() as u64, stamp(node.boot_time), "vlan.dat".to_string()));
    }
    if let Some(startup) = &node.startup {
        files.push((startup.len() as u64, stamp(now), "config.text".to_string()));
    }
    let used: u64 = files.iter().map(|(size, _, _)| size).sum();
    let mut out = String::from("Directory of flash:/\n\n");
    for (index, (size, date, name)) in files.iter().enumerate() {
        let _ = writeln!(out, "{:>5}  -rw-  {size:>11}  {date:>26}  {name}", index + 1);
    }
    let total: u64 = 256_901_120;
    let _ = writeln!(out, "\n{total} bytes total ({} bytes free)", total.saturating_sub(used));
    out
}

pub(crate) fn logging(node: &Node) -> String {
    let cfg = &node.cfg.logging;
    let count = node.log.next_seq;
    let console = if cfg.console { format!("level debugging, {count} messages logged") } else { "disabled".to_string() };
    let mut out = String::new();
    out.push_str("Syslog logging: enabled (0 messages dropped, 0 messages rate-limited, 0 flushes, 0 overruns, xml disabled, filtering disabled)\n\n");
    out.push_str("No Active Message Discriminator.\n\n\n\nNo Inactive Message Discriminator.\n\n\n");
    let _ = writeln!(out, "    Console logging: {console}, xml disabled,\n                     filtering disabled");
    out.push_str("    Monitor logging: level debugging, 0 messages logged, xml disabled,\n                     filtering disabled\n");
    let _ = writeln!(
        out,
        "    Buffer logging:  level debugging, {count} messages logged, xml disabled,\n                    filtering disabled"
    );
    out.push_str("    Exception Logging: size (8192 bytes)\n");
    out.push_str("    Count and timestamp logging messages: disabled\n");
    out.push_str("    Persistent logging: disabled\n\nNo active filter modules.\n\n");
    let _ = writeln!(
        out,
        "    Trap logging: level {}, {count} message lines logged",
        cfg.trap.as_deref().unwrap_or("informational")
    );
    for host in &cfg.hosts {
        let _ = writeln!(out, "        Logging to {host}  (udp port 514, audit disabled,\n              link up),");
        let _ = writeln!(out, "              {count} message lines logged,\n              0 message lines rate-limited,");
        out.push_str("              0 message lines dropped-by-MD,\n              xml disabled, sequence number disabled\n");
        out.push_str("              filtering disabled\n");
    }
    let _ = writeln!(out, "\nLog Buffer ({} bytes):\n", cfg.buffered.unwrap_or(8192));
    for entry in &node.log.entries {
        out.push_str(&entry.line);
        out.push('\n');
    }
    out
}

pub(crate) fn privilege(cx: &Cx) -> String {
    format!("Current privilege level is {}\n", if cx.sh.mode == Mode::User { 1 } else { 15 })
}

pub(crate) fn protocols(node: &Node) -> String {
    let mut out = String::from("Global values:\n");
    let _ = writeln!(out, "  Internet Protocol routing is {}", if node.routes_packets() { "enabled" } else { "disabled" });
    for id in node.cfg.interfaces.keys() {
        let (status, proto) = super::interfaces::status_words(node, *id);
        let _ = writeln!(out, "{} is {status}, line protocol is {proto}", id.name());
        if let Some(addr) = node.addr(*id).filter(|_| node.is_l3(*id)) {
            let _ = writeln!(out, "  Internet address is {addr}");
        }
    }
    out
}

pub(crate) fn ntp_status(node: &Node) -> String {
    if node.cfg.ntp_servers.is_empty() {
        return "%NTP is not enabled.\n".to_string();
    }
    // Time itself is not exchanged in the lab, so the clock never synchronises.
    "Clock is unsynchronized, stratum 16, no reference clock\n\
     nominal freq is 250.0000 Hz, actual freq is 250.0000 Hz, precision is 2**10\n\
     ntp uptime is 0 (1/100 of seconds), resolution is 4000\n\
     reference time is 00000000.00000000 (00:00:00.000 UTC Mon Jan 1 1900)\n\
     clock offset is 0.0000 msec, root delay is 0.00 msec\n\
     root dispersion is 0.00 msec, peer dispersion is 0.00 msec\n\
     loopfilter state is 'FSET' (Drift set from file), drift is 0.000000000 s/s\n\
     system poll interval is 64, never updated.\n"
        .to_string()
}

pub(crate) fn ntp_associations(node: &Node) -> String {
    if node.cfg.ntp_servers.is_empty() {
        return "%NTP is not enabled.\n".to_string();
    }
    let mut out = String::from("\n  address         ref clock       st   when   poll reach  delay  offset   disp\n");
    for server in &node.cfg.ntp_servers {
        let _ = writeln!(out, " ~{:<15} .INIT.          16      -     64     0  0.000   0.000 15937.");
        out = out.replace("{:<15}", &format!("{:<15}", server.to_string()));
    }
    out.push_str(" * sys.peer, # selected, + candidate, - outlyer, x falseticker, ~ configured\n");
    out
}

pub(crate) fn snmp(node: &Node) -> String {
    if node.cfg.snmp.communities.is_empty() {
        return "%SNMP agent not enabled\n".to_string();
    }
    let mut out = String::new();
    let _ = writeln!(out, "Chassis: {}", node.serial);
    if let Some(contact) = &node.cfg.snmp.contact {
        let _ = writeln!(out, "Contact: {contact}");
    }
    if let Some(location) = &node.cfg.snmp.location {
        let _ = writeln!(out, "Location: {location}");
    }
    out.push_str("0 SNMP packets input\n    0 Bad SNMP version errors\n    0 Unknown community name\n");
    out.push_str("    0 Illegal operation for community name supplied\n    0 Encoding errors\n");
    out.push_str("    0 Number of requested variables\n    0 Number of altered variables\n");
    out.push_str("    0 Get-request PDUs\n    0 Get-next PDUs\n    0 Set-request PDUs\n    0 Input queue packet drops (Maximum queue size 1000)\n");
    out.push_str("0 SNMP packets output\n    0 Too big errors (Maximum packet size 1500)\n    0 No such name errors\n");
    out.push_str("    0 Bad values errors\n    0 General errors\n    0 Response PDUs\n    0 Trap PDUs\n");
    out.push_str("SNMP global trap: disabled\n\nSNMP logging: disabled\n");
    out
}

pub(crate) fn snmp_community(node: &Node) -> String {
    let mut out = String::new();
    for community in &node.cfg.snmp.communities {
        let _ = writeln!(out, "\nCommunity name: {}", community.name);
        let _ = writeln!(out, "Community Index: {}", community.name);
        out.push_str("Community SecurityName: ");
        let _ = writeln!(out, "{}", community.name);
        let _ = writeln!(out, "storage-type: nonvolatile\t active{}", community.acl.as_ref().map(|a| format!("\taccess-list: {a}")).unwrap_or_default());
    }
    out.push('\n');
    out
}

pub(crate) fn ssh(node: &Node) -> String {
    if node.rsa_bits.is_none() {
        return "%No SSHv2 server connections running.\n%No SSHv1 server connections running.\n".to_string();
    }
    let mut out = String::new();
    let ssh_sessions: Vec<_> = node.sessions.iter().filter(|s| matches!(s.line, LineId::Vty(_)) && !s.user.is_empty()).collect();
    if ssh_sessions.is_empty() {
        return "%No SSHv2 server connections running.\n%No SSHv1 server connections running.\n".to_string();
    }
    out.push_str("Connection Version Mode Encryption  Hmac         State                 Username\n");
    for (index, session) in ssh_sessions.iter().enumerate() {
        for direction in ["IN ", "OUT"] {
            let _ = writeln!(out, "{index:<10} 2.0     {direction}  aes128-ctr  hmac-sha1    Session started       {}", session.user);
        }
    }
    out.push_str("%No SSHv1 server connections running.\n");
    out
}

pub(crate) fn ip_ssh(node: &Node) -> String {
    match node.rsa_bits {
        None => "SSH Disabled - version 1.99\n%Please create RSA keys to enable SSH (and of atleast 768 bits for SSH v2).\nAuthentication timeout: 120 secs; Authentication retries: 3\n".to_string(),
        Some(bits) => {
            let version = if node.cfg.ssh_version == Some(2) { "2.0" } else { "1.99" };
            format!(
                "SSH Enabled - version {version}\nAuthentication methods:publickey,keyboard-interactive,password\nAuthentication timeout: 120 secs; Authentication retries: 3\nMinimum expected Diffie Hellman key size : 1024 bits\nIOS Keys in SECSH format(ssh-rsa, base64 encoded): {}.{} ({bits} bits)\n",
                node.hostname(),
                node.cfg.domain_name.as_deref().unwrap_or("lab")
            )
        }
    }
}

pub(crate) fn line(cx: &Cx) -> String {
    let mut out = String::from("   Tty Typ     Tx/Rx    A Modem  Roty AccO AccI   Uses   Noise  Overruns   Int\n");
    let busy = |line: LineId| line == cx.sh.line || cx.n.sessions.iter().any(|s| s.line == line);
    let mut row = |line: LineId, kind: &str, acl: Option<&String>| {
        let mark = if busy(line) { "*" } else { " " };
        let _ = writeln!(
            out,
            "{mark}{:>5} {kind}              - -      -    -    {:<4}   {:>4}       0     0/0       -",
            line_number(line),
            acl.map(String::as_str).unwrap_or("-"),
            u8::from(busy(line))
        );
    };
    row(LineId::Console, "CTY", cx.n.cfg.console.access_class_in.as_ref());
    for (index, vty) in cx.n.cfg.vty.iter().enumerate() {
        row(LineId::Vty(index as u8), "VTY", vty.access_class_in.as_ref());
    }
    out
}

pub(crate) fn processes_cpu(node: &Node, fx: &Fx) -> String {
    let now_pct = node.cpu_percent(fx.now, fx.seed).round() as u32;
    let minute = node.cpu_percent(fx.now.saturating_sub(30_000_000), fx.seed).round() as u32;
    let five = node.cpu_percent(fx.now.saturating_sub(150_000_000), fx.seed).round() as u32;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "CPU utilization for five seconds: {now_pct}%/{}%; one minute: {minute}%; five minutes: {five}%",
        now_pct / 4
    );
    out.push_str(" PID Runtime(ms)     Invoked      uSecs   5Sec   1Min   5Min TTY Process \n");
    let uptime_s = node.uptime(fx.now) / 1_000_000;
    let procs: [(&str, u32); 8] = [
        ("Chunk Manager", 0),
        ("Load Meter", 0),
        ("Check heaps", 5),
        ("Pool Manager", 0),
        ("ARP Input", 2),
        ("IP Input", 70),
        ("Spanning Tree", 8),
        ("Exec", 15),
    ];
    for (pid, (name, share)) in procs.iter().enumerate() {
        let pct = f64::from(now_pct) * f64::from(*share) / 100.0;
        let _ = writeln!(
            out,
            "{:>4} {:>11} {:>11} {:>10} {:>5.2}% {:>5.2}% {:>5.2}% {:>3} {name:<16}",
            pid + 1,
            uptime_s * u64::from(*share) / 10,
            uptime_s * 5 + pid as u64,
            100 + pid * 7,
            pct,
            pct,
            pct,
            0
        );
    }
    out
}

pub(crate) fn cpu_history(node: &Node, fx: &Fx) -> String {
    // One column per five seconds over the last five minutes, newest first.
    let samples: Vec<u32> =
        (0..60).map(|i| node.cpu_percent(fx.now.saturating_sub(i * 5_000_000), fx.seed).round() as u32).collect();
    let mut out = String::from("\n");
    for level in (1..=10).rev().map(|l| l * 10) {
        let row: String = samples.iter().map(|s| if *s >= level { '*' } else { ' ' }).collect();
        let _ = writeln!(out, "{level:>3} {}", row.trim_end());
    }
    out.push_str("    0....5....1....1....2....2....3....3....4....4....5....5....\n");
    out.push_str("              0    5    0    5    0    5    0    5    0    5\n");
    out.push_str("               CPU% per second (last 60 samples, 5 seconds apart)\n\n");
    out
}

pub(crate) fn memory(node: &Node) -> String {
    let (total, used) = node.memory();
    let mut out = String::from("                Head    Total(b)     Used(b)     Free(b)   Lowest(b)  Largest(b)\n");
    let _ = writeln!(
        out,
        "Processor   {:>8X}  {total:>10}  {used:>10}  {:>10}  {:>10}  {:>10}",
        0x0260_0000u32,
        total - used,
        (total - used) * 9 / 10,
        (total - used) * 8 / 10
    );
    let io = total / 8;
    let _ = writeln!(out, "      I/O   {:>8X}  {io:>10}  {:>10}  {:>10}  {:>10}  {:>10}", 0x0E00_0000u32, io / 5, io * 4 / 5, io * 4 / 5, io * 3 / 5);
    out
}

pub(crate) fn environment(node: &Node, fx: &Fx) -> String {
    let temperature = node.temperature(fx.now, fx.seed);
    let mut out = String::new();
    out.push_str("FAN is OK\n");
    let _ = writeln!(out, "SYSTEM TEMPERATURE is OK");
    let _ = writeln!(out, "System Temperature Value: {:.0} Degree Celsius", temperature);
    out.push_str("System Temperature State: GREEN\nYellow Threshold : 46 Degree Celsius\nRed Threshold    : 60 Degree Celsius\n");
    out.push_str("SW  PID                 Serial#     Status           Sys Pwr  PoE Pwr  Watts\n");
    out.push_str("--  ------------------  ----------  ---------------  -------  -------  -----\n");
    let _ = writeln!(out, " 1  Built-in                                 Good\n");
    out
}

pub(crate) fn controllers(node: &Node, only: Option<IfId>) -> String {
    let mut out = String::new();
    for (id, state) in node.ifs.iter().filter(|(id, _)| id.is_physical() && only.is_none_or(|o| o == **id)) {
        let _ = writeln!(out, "Interface {}", id.name());
        let _ = writeln!(out, "  Hardware is {}", super::interfaces::hardware(*id));
        let _ = writeln!(out, "  address is {} (bia {})", state.mac, state.mac);
        let _ = writeln!(out, "  link is {}, {} Mb/s, {}-duplex\n", if state.carrier { "up" } else { "down" }, state.speed, if state.full_duplex { "full" } else { "half" });
    }
    out
}

/// The main show commands, one after another.
pub(crate) fn tech_support(cx: &mut Cx) -> String {
    let now = cx.fx.now;
    let mut out = String::new();
    let mut section = |title: &str, body: String| {
        let _ = writeln!(out, "\n------------------ {title} ------------------\n\n{body}");
    };
    section("show version", version(cx.n, now));
    section("show running-config", runcfg::show_running(cx.n));
    section("show interfaces", super::interfaces::interfaces(cx.n, now, None).unwrap_or_default());
    section("show ip interface brief", super::interfaces::ip_brief(cx.n));
    section("show ip route", super::routing::ip_route(cx.n, now, &Default::default()));
    section("show arp", super::routing::arp(cx.n, now));
    if cx.n.model.is_switch() {
        section("show vlan", super::switching::vlan(cx.n, false, None));
        section("show spanning-tree", super::switching::spanning_tree(cx.n, cx.fx, None, None));
    }
    section("show logging", logging(cx.n));
    out
}

/// The timestamp format in effect, for callers that describe it.
#[allow(dead_code)]
pub(crate) fn timestamp_mode(node: &Node) -> Timestamps {
    node.cfg.services.timestamps_log
}
