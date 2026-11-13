//! EXEC-mode commands (everything typed at `>` and `#` except `show`).

use std::net::Ipv4Addr;

use crate::catalog::Role;
use crate::device::cli::grammar::Args;
use crate::device::cli::{LineId, Next, Reply, Tag};
use crate::device::ios::{Cx, IosShell, Mode, Table, boot, c, runcfg, show};
use crate::ifname::IfId;
use crate::sim::fx::Fx;
use crate::sim::node::Node;
use crate::sim::procs::{PingSpec, Style};
use crate::util::clock;
use crate::util::net::parse_ip;

pub(crate) fn table(role: Role, privileged: bool) -> Table {
    let mut t: Table = vec![
        c("enable", "Turn on privileged commands", enable),
        c("exit", "Exit from the EXEC", logout),
        c("logout", "Exit from the EXEC", logout),
        c(
            "ping [ip] <word:target;Ping destination address or hostname> {repeat <1-2147483647:count;Repeat count> | timeout <0-3600:timeout;Timeout in seconds> | size <36-18024:size;Datagram size> | source (<ip:source_ip> | <if:source_if>)}",
            "Send echo messages",
            ping,
        ),
        c("traceroute [ip] <word:target;Trace route to destination address or hostname>", "Trace route to destination", traceroute),
        c("telnet <word:target;IP address or hostname of a remote system> [<1-65535:port>]", "Open a telnet connection", telnet),
        c("ssh [-l <word:user;Login name>] <word:target;IP address or hostname of a remote system>", "Open a secure shell client connection", ssh),
        c("terminal length <0-512:lines;Number of lines on screen (0 for no pausing)>", "Set number of lines on a screen", terminal_length),
        c("terminal width <0-512:columns;Number of characters on a screen line>", "Set width of the display terminal", terminal_width),
        c("terminal monitor", "Copy debug output to the current terminal line", |cx, _| cx.sh.monitor = true),
        c("terminal no monitor", "Stop copying debug output to this line", |cx, _| cx.sh.monitor = false),
    ];
    t.extend(show::table(role, privileged));
    if !privileged {
        return t;
    }
    t.extend(vec![
        c("disable", "Turn off privileged commands", |cx, _| cx.sh.mode = Mode::User),
        c("configure [terminal]", "Enter configuration mode", configure),
        c("copy running-config startup-config", "Copy from current system configuration", |cx, _| {
            cx.next = Next::Ask {
                prompt: "Destination filename [startup-config]? ".to_string(),
                hidden: false,
                tag: Tag::CopyDestination,
            };
        }),
        c("copy startup-config running-config", "Copy from startup configuration", copy_startup_running),
        c("write [memory]", "Write running configuration to memory, network, or terminal", |cx, _| save(cx)),
        c("write erase", "Erase NV memory", erase_prompt),
        c("write terminal", "Write to terminal", |cx, _| {
            let text = runcfg::show_running(cx.n);
            cx.out.push_str(&text);
        }),
        c("erase startup-config", "Erase contents of configuration memory", erase_prompt),
        c("reload", "Halt and perform a cold restart", reload),
        c("clear arp-cache", "Clear the entire ARP cache", |cx, _| {
            cx.n.arp.clear();
            cx.fx.changed = true;
        }),
        c("clear mac address-table dynamic [vlan <1-4094:vlan> | interface <if>]", "dynamic entry type", clear_mac),
        c("clear ip nat translation *", "Delete all dynamic translations", |cx, _| {
            cx.n.nat_clear();
            cx.fx.changed = true;
        }),
        c("clear counters [<if>]", "Clear counters on one or all interfaces", clear_counters),
        c("clear ip ospf process", "Reset OSPF process", |cx, _| {
            cx.next = Next::Ask { prompt: "Reset ALL OSPF processes? [no]: ".to_string(), hidden: false, tag: Tag::ClearOspf };
        }),
        c("clear ip dhcp binding (* | <ip:addr>)", "DHCP address bindings", |cx, a| {
            cx.n.dhcp_clear_bindings(a.ip("addr"));
            cx.fx.changed = true;
        }),
        c("clear access-list counters [<word:name>]", "Clear access list counters", |cx, a| match a.text("name") {
            Some(name) => cx.n.acl_hits.retain(|(list, _), _| list != name),
            None => cx.n.acl_hits.clear(),
        }),
        c("clear ip access-list counters [<word:name>]", "Clear access list counters", |cx, a| match a.text("name") {
            Some(name) => cx.n.acl_hits.retain(|(list, _), _| list != name),
            None => cx.n.acl_hits.clear(),
        }),
        c("clear logging", "Clear logging buffer", |cx, _| cx.n.log.clear()),
        c("clock set <word:time;hh:mm:ss  Current Time> <word:a;Day of the month or month> <word:b;Month of the year or day> <1993-2035:year;Year>", "Set the time and date", clock_set),
        c("debug <line:what>", "Debugging functions", |_, _| {}),
        c("undebug all", "Disable all debugging", |cx, _| cx.say("All possible debugging has been turned off")),
        c("no debug all", "Disable all debugging", |cx, _| cx.say("All possible debugging has been turned off")),
        c("dir [flash:]", "List files on a filesystem", |cx, _| {
            let text = show::system::flash(cx.n, cx.fx.now);
            cx.out.push_str(&text);
        }),
        c("delete <word:file;File to be deleted>", "Delete a file", delete),
    ]);
    if role != Role::Router {
        t.push(c(
            "clear port-security (all | sticky | dynamic) [interface <if>]",
            "Clear secure MAC addresses",
            clear_port_security,
        ));
    }
    t
}

fn enable(cx: &mut Cx, _: &Args) {
    if cx.sh.mode == Mode::Priv {
        return;
    }
    if cx.n.cfg.enable_secret.is_some() || cx.n.cfg.enable_password.is_some() {
        cx.next = Next::Ask { prompt: "Password: ".to_string(), hidden: true, tag: Tag::Enable { tries: 0 } };
    } else if cx.sh.line == LineId::Console {
        cx.sh.mode = Mode::Priv;
    } else {
        // Without an enable password only the console may become privileged.
        cx.fail("% No password set");
    }
}

fn logout(cx: &mut Cx, _: &Args) {
    cx.next = Next::Close;
}

fn configure(cx: &mut Cx, _: &Args) {
    cx.say("Enter configuration commands, one per line.  End with CNTL/Z.");
    cx.sh.mode = Mode::Config;
}

/// Resolve a ping/traceroute/telnet target typed as an address.
fn target_ip(cx: &mut Cx, a: &Args) -> Option<Ipv4Addr> {
    let text = a.text("target").unwrap_or("");
    let ip = parse_ip(text);
    if ip.is_none() {
        cx.fail("% Unrecognized host or address, or protocol not running.");
    }
    ip
}

fn ping(cx: &mut Cx, a: &Args) {
    let size = a.num("size").unwrap_or(100) as u16;
    let count = a.num("count").unwrap_or(5) as u32;
    let timeout_micros = a.num("timeout").unwrap_or(2).max(1) * 1_000_000;
    let source = match (a.ip("source_ip"), a.iface("source_if")) {
        (Some(ip), _) if cx.n.owns_ip(ip) => Some(ip),
        (None, Some(iface)) => match cx.n.addr(iface).filter(|_| cx.n.is_up(iface)) {
            Some(addr) => Some(addr.ip),
            None => return cx.fail("% Invalid source interface - Interface is down or has no IP address"),
        },
        (Some(_), _) => return cx.fail("% Invalid source address- IP address not on any of our up interfaces"),
        (None, None) => None,
    };
    let text = a.text("target").unwrap_or("").to_string();
    let spec = |dst| PingSpec { dst, src: source, count, timeout_micros, size, style: Style::Ios, label: None };
    match parse_ip(&text) {
        Some(dst) => cx.next = Next::Job(cx.n.start_ping(cx.fx, spec(dst), None)),
        None => match cx.n.cfg.name_servers.first().copied().filter(|_| cx.n.cfg.domain_lookup) {
            Some(server) => {
                cx.out.push_str(&format!("Translating \"{text}\"...domain server ({server})\n"));
                let then = spec(Ipv4Addr::UNSPECIFIED);
                cx.next = Next::Job(cx.n.start_lookup(cx.fx, &text, server, Some(then), true));
            }
            None => cx.fail("% Unrecognized host or address, or protocol not running."),
        },
    }
}

fn traceroute(cx: &mut Cx, a: &Args) {
    if let Some(dst) = target_ip(cx, a) {
        cx.next = Next::Job(cx.n.start_traceroute(cx.fx, dst, Style::Ios));
    }
}

fn telnet(cx: &mut Cx, a: &Args) {
    if let Some(dst) = target_ip(cx, a) {
        let port = a.num("port").unwrap_or(23) as u16;
        let proc = cx.n.start_connect(cx.fx, dst, port, Style::Ios, None);
        cx.next = Next::Connect { proc, ssh_user: None };
    }
}

fn ssh(cx: &mut Cx, a: &Args) {
    if let Some(dst) = target_ip(cx, a) {
        let user = a.text("user").map(str::to_string).unwrap_or_else(|| cx.sh.user().to_string());
        let proc = cx.n.start_connect(cx.fx, dst, 22, Style::Ios, None);
        cx.next = Next::Connect { proc, ssh_user: Some(user) };
    }
}

fn terminal_length(cx: &mut Cx, a: &Args) {
    cx.sh.term_len = a.num("lines").unwrap_or(24) as u32;
}

fn terminal_width(cx: &mut Cx, a: &Args) {
    cx.sh.term_width = a.num("columns").unwrap_or(80) as u32;
}

/// Store the running configuration as the startup configuration.
fn save(cx: &mut Cx) {
    cx.say("Building configuration...");
    cx.n.startup = Some(runcfg::render(cx.n));
    if cx.n.model.is_switch() {
        cx.n.vlan_dat = Some(cx.n.cfg.vlans.clone());
    }
    cx.n.unsaved = false;
    cx.say("[OK]");
}

fn copy_startup_running(cx: &mut Cx, _: &Args) {
    let Some(text) = cx.n.startup.clone() else {
        return cx.fail("%% Non-volatile configuration memory invalid or not present");
    };
    let mut shell = IosShell::automation(Mode::Config);
    boot::apply_lines(&mut shell, cx.n, cx.fx, &text);
    cx.touched = true;
    cx.say(format!("{} bytes copied in 0.416 secs", text.len()));
}

fn erase_prompt(cx: &mut Cx, _: &Args) {
    cx.next = Next::Key {
        prompt: "Erasing the nvram filesystem will remove all configuration files! Continue? [confirm]".to_string(),
        tag: Tag::EraseConfirm,
    };
}

fn reload(cx: &mut Cx, _: &Args) {
    if cx.n.unsaved {
        cx.next = Next::Ask {
            prompt: "\nSystem configuration has been modified. Save? [yes/no]: ".to_string(),
            hidden: false,
            tag: Tag::ReloadSave,
        };
    } else {
        cx.next = Next::Key { prompt: "Proceed with reload? [confirm]".to_string(), tag: Tag::ReloadConfirm };
    }
}

fn clear_mac(cx: &mut Cx, a: &Args) {
    match (a.num("vlan"), a.iface("if")) {
        (Some(vlan), _) => cx.n.macs.flush_vlan(vlan as u16, None),
        (None, Some(port)) => cx.n.macs.flush_port(port),
        (None, None) => cx.n.macs.clear_dynamic(),
    }
    cx.fx.changed = true;
}

fn clear_counters(cx: &mut Cx, a: &Args) {
    let target = a.iface("if");
    if let Some(iface) = target
        && !cx.n.ifs.contains_key(&iface)
    {
        return cx.fail("% Invalid interface");
    }
    let prompt = match target {
        Some(iface) => format!("Clear \"show interface\" counters on this interface [confirm]{}", iface_marker(iface)),
        None => "Clear \"show interface\" counters on all interfaces [confirm]".to_string(),
    };
    cx.next = Next::Key { prompt, tag: Tag::ClearCounters { iface: target.map(|i| i.name()) } };
}

/// The interface name is not shown in the real prompt; kept for symmetry.
fn iface_marker(_iface: IfId) -> &'static str {
    ""
}

fn clear_port_security(cx: &mut Cx, a: &Args) {
    let only = a.iface("if");
    let sticky = a.has("sticky") || a.has("all");
    let dynamic = a.has("dynamic") || a.has("all");
    let ports: Vec<IfId> = cx.n.ifs.keys().copied().filter(|id| only.is_none_or(|o| o == *id)).collect();
    for port in ports {
        if dynamic && let Some(state) = cx.n.ifs.get_mut(&port) {
            state.secure_macs.clear();
        }
        if sticky && let Some(sw) = cx.n.cfg.interfaces.get_mut(&port).and_then(|c| c.l2.as_mut()) {
            sw.port_security.macs.retain(|(_, sticky)| !sticky);
            cx.touched = true;
        }
        cx.n.macs.flush_port(port);
    }
}

fn clock_set(cx: &mut Cx, a: &Args) {
    let time: Vec<u32> = a.text("time").unwrap_or("").split(':').filter_map(|p| p.parse().ok()).collect();
    let (first, second) = (a.text("a").unwrap_or(""), a.text("b").unwrap_or(""));
    // Both "1 March" and "March 1" are accepted.
    let (day, month) = match (first.parse::<u32>(), clock::month_number(second)) {
        (Ok(day), Some(month)) => (day, month),
        _ => match (second.parse::<u32>(), clock::month_number(first)) {
            (Ok(day), Some(month)) => (day, month),
            _ => return cx.fail("% Invalid date"),
        },
    };
    let year = a.num("year").unwrap_or(1993) as i64;
    let valid_time = time.len() == 3 && time[0] < 24 && time[1] < 60 && time[2] < 60;
    let Some(days) = clock::days_from_civil(year, month, day).filter(|_| valid_time) else {
        return cx.fail("% Invalid time or date");
    };
    let wanted_ms = (days * 86_400 + i64::from(time[0] * 3600 + time[1] * 60 + time[2])) * 1000;
    let unset_ms = clock::EPOCH_1993 * 1000 + (cx.n.uptime(cx.fx.now) / 1000) as i64;
    cx.n.clock_offset_ms = Some(wanted_ms - unset_ms);
    let stamp = clock::DateTime::from_unix_ms(wanted_ms);
    cx.n.log(
        cx.fx,
        6,
        "SYS",
        "CLOCKUPDATE",
        format!(
            "System clock has been updated to {:02}:{:02}:{:02} UTC {} {} {} {}, configured from console by console.",
            stamp.hour,
            stamp.min,
            stamp.sec,
            stamp.weekday_name(),
            stamp.month_name(),
            stamp.day,
            stamp.year
        ),
    );
}

fn delete(cx: &mut Cx, a: &Args) {
    let file = a.text("file").unwrap_or("").trim_start_matches("flash:").trim_start_matches('/').to_string();
    cx.next = Next::Ask {
        prompt: format!("Delete filename [{file}]? "),
        hidden: false,
        tag: Tag::DeleteName { file },
    };
}

/// Answers to the questions EXEC commands ask.
pub(crate) fn answer(shell: &mut IosShell, node: &mut Node, fx: &mut Fx, tag: Tag, text: &str) -> Reply {
    let yes = |t: &str| t == "\n" || t.eq_ignore_ascii_case("y");
    match tag {
        Tag::CopyDestination => {
            let name = text.trim();
            if !name.is_empty() && name != "startup-config" {
                return Reply { out: "%Error opening destination (only startup-config is supported)\n".to_string(), error: true, ..Reply::default() };
            }
            let mut cx = Cx { n: node, fx, sh: shell, out: String::new(), error: false, touched: false, next: Next::Prompt };
            save(&mut cx);
            Reply::text(cx.out)
        }
        Tag::EraseConfirm => {
            if !yes(text) {
                return Reply::default();
            }
            node.startup = None;
            node.log(fx, 7, "SYS", "NV_BLOCK_INIT", "Initialized the geometry of nvram".to_string());
            Reply::text("[OK]\nErase of nvram: complete\n")
        }
        Tag::ReloadSave => {
            let answer = text.trim().to_ascii_lowercase();
            let mut out = String::new();
            if !answer.is_empty() && "yes".starts_with(&answer) {
                let mut cx = Cx { n: node, fx, sh: shell, out: String::new(), error: false, touched: false, next: Next::Prompt };
                save(&mut cx);
                out = cx.out;
            } else if answer.is_empty() || !"no".starts_with(&answer) {
                return Reply {
                    out: "% Please answer 'yes' or 'no'.\n".to_string(),
                    next: Next::Ask {
                        prompt: "System configuration has been modified. Save? [yes/no]: ".to_string(),
                        hidden: false,
                        tag: Tag::ReloadSave,
                    },
                    ..Reply::default()
                };
            }
            Reply { out, next: Next::Key { prompt: "Proceed with reload? [confirm]".to_string(), tag: Tag::ReloadConfirm }, ..Reply::default() }
        }
        Tag::ReloadConfirm => {
            if !yes(text) {
                return Reply::default();
            }
            node.log(fx, 5, "SYS", "RELOAD", "Reload requested by console. Reload Reason: Reload Command.".to_string());
            Reply { next: Next::Reload, ..Reply::default() }
        }
        Tag::ClearCounters { iface } => {
            if yes(text) {
                let only = iface.as_deref().and_then(IfId::parse);
                for (id, state) in &mut node.ifs {
                    if only.is_none_or(|o| o == *id) {
                        state.counters = Default::default();
                    }
                }
                let by = if shell.user().is_empty() { "console" } else { shell.user() };
                let what = iface.map_or("all interfaces".to_string(), |name| format!("interface {name}"));
                node.log(fx, 5, "CLEAR", "COUNTERS", format!("Clear counter on {what} by {by}"));
            }
            Reply::default()
        }
        Tag::ClearOspf => {
            let answer = text.trim().to_ascii_lowercase();
            if !answer.is_empty() && "yes".starts_with(&answer) {
                node.ospf_restart(fx);
            }
            Reply::default()
        }
        Tag::DeleteName { file } => {
            let name = if text.trim().is_empty() { file } else { text.trim().to_string() };
            Reply {
                next: Next::Key { prompt: format!("Delete flash:/{name}? [confirm]"), tag: Tag::DeleteConfirm { file: name } },
                ..Reply::default()
            }
        }
        Tag::DeleteConfirm { file } => {
            if !yes(text) {
                return Reply::default();
            }
            match file.as_str() {
                "vlan.dat" if node.vlan_dat.is_some() => {
                    node.vlan_dat = None;
                    Reply::default()
                }
                "config.text" if node.startup.is_some() => {
                    node.startup = None;
                    Reply::default()
                }
                _ => Reply {
                    out: format!("%Error deleting flash:/{file} (No such file or directory)\n"),
                    error: true,
                    ..Reply::default()
                },
            }
        }
        Tag::LinePassword { .. } | Tag::Username { .. } | Tag::UserPassword { .. } | Tag::Enable { .. } => Reply::default(),
    }
}
