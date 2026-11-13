//! The console line discipline and command dispatch, byte for byte.

mod common;

use common::*;
use nocturne_netsim::{ConsoleId, Lab, NodeId};

fn switch_console() -> (Lab, NodeId, ConsoleId) {
    let mut lab = Lab::new("cli");
    let sw = node(&mut lab, "switch", "SW1");
    let console = lab.console_open(sw).unwrap();
    assert_eq!(output(&mut lab, console), "\r\nSW1>");
    (lab, sw, console)
}

fn privileged() -> (Lab, NodeId, ConsoleId) {
    let (mut lab, sw, console) = switch_console();
    assert_eq!(typed(&mut lab, console, "enable"), "enable\r\nSW1#");
    assert_eq!(typed(&mut lab, console, "terminal length 0"), "terminal length 0\r\nSW1#");
    (lab, sw, console)
}

#[test]
fn echo_editing_and_history() {
    let (mut lab, _, c) = privileged();
    // Backspace (both codes) erases on screen and in the buffer.
    lab.console_input(c, b"shoq\x7fw clocj\x08k\r");
    let out = output(&mut lab, c);
    assert!(out.starts_with("shoq\x08 \x08w clocj\x08 \x08k\r\n*00:00:"), "{out:?}");
    assert!(out.ends_with("UTC Mon Mar 1 1993\r\nSW1#"), "{out:?}");

    // Ctrl-A, insert at the start, Ctrl-E, finish the line.
    lab.console_input(c, b"how cloc\x01s\x05k\r");
    let out = output(&mut lab, c);
    assert!(out.contains("UTC Mon Mar 1 1993"), "{out:?}");
    // Left arrow then insert in the middle; right arrow moves back.
    lab.console_input(c, b"show clck\x1b[D\x1b[Do\x1b[C\x1b[C\r");
    assert!(output(&mut lab, c).contains("UTC Mon Mar 1 1993"));
    // Ctrl-U discards the line, Ctrl-W the last word.
    lab.console_input(c, b"garbage here\x15show clock junk\x17\r");
    assert!(output(&mut lab, c).contains("UTC Mon Mar 1 1993"));

    // Up arrow recalls, twice goes further back, down comes forward again.
    typed(&mut lab, c, "show privilege");
    lab.console_input(c, b"\x1b[A");
    assert_eq!(output(&mut lab, c), "show privilege");
    lab.console_input(c, b"\x1b[A\x1b[B\r");
    assert!(output(&mut lab, c).contains("Current privilege level is 15"));
    let history = typed(&mut lab, c, "show history");
    assert!(history.contains("  show clock\r\n") && history.contains("  show privilege\r\n"), "{history:?}");

    // CR LF from a terminal counts as one Enter; a lone LF works too.
    lab.console_input(c, b"\r\n\n");
    assert_eq!(output(&mut lab, c), "\r\nSW1#\r\nSW1#");
}

#[test]
fn abbreviations_errors_and_help() {
    let (mut lab, _, c) = privileged();
    assert!(typed(&mut lab, c, "sh ip int br").contains("Interface                  IP-Address      OK? Method Status                Protocol"));
    assert_eq!(typed(&mut lab, c, "sh i"), "sh i\r\n% Ambiguous command:  \"sh i\"\r\nSW1#");
    assert_eq!(typed(&mut lab, c, "show"), "show\r\n% Incomplete command.\r\n\r\nSW1#");
    // The caret sits under the first character that matches nothing.
    assert_eq!(
        typed(&mut lab, c, "show ip routx"),
        "show ip routx\r\n                ^\r\n% Invalid input detected at '^' marker.\r\n\r\nSW1#"
    );
    assert_eq!(typed(&mut lab, c, "frobnicate"), "frobnicate\r\n    ^\r\n% Invalid input detected at '^' marker.\r\n\r\nSW1#");

    // `?` acts immediately: list, then prompt and partial line again.
    lab.console_input(c, b"show sp?");
    let out = output(&mut lab, c);
    assert_eq!(out, "show sp?\r\nspanning-tree\r\n\r\nSW1#show sp");
    lab.console_input(c, b"\x15show ip ?");
    let out = output(&mut lab, c);
    assert!(out.contains("  route         IP routing table\r\n"), "{out:?}");
    assert!(out.ends_with("\r\nSW1#show ip "), "{out:?}");
    lab.console_input(c, b"route ?");
    let out = output(&mut lab, c);
    assert!(out.contains("  A.B.C.D    Network to display information about\r\n") && out.contains("  <cr>\r\n"), "{out:?}");
    lab.console_input(c, b"\x15");
    output(&mut lab, c);

    // Tab completes a unique prefix in place; an ambiguous one just redraws.
    lab.console_input(c, b"sh\tver\t");
    assert_eq!(output(&mut lab, c), "show version ");
    lab.console_input(c, b"\r");
    assert!(output(&mut lab, c).contains("Nocturne Virtual Software"));
    lab.console_input(c, b"show i\t");
    assert_eq!(output(&mut lab, c), "show i\r\nSW1#show i");
    lab.console_input(c, b"\x15");
    output(&mut lab, c);

    // User EXEC cannot see the configuration.
    typed(&mut lab, c, "disable");
    assert!(typed(&mut lab, c, "show running-config").contains("% Invalid input detected"));
    assert!(typed(&mut lab, c, "configure terminal").contains("% Invalid input detected"));
}

#[test]
fn paging_and_output_modifiers() {
    let (mut lab, _, c) = privileged();
    typed(&mut lab, c, "terminal length 6");
    let first = typed(&mut lab, c, "show vlan");
    assert_eq!(first.matches("\r\n").count(), 6, "{first:?}");
    assert!(first.ends_with("\r\n --More-- "), "{first:?}");
    // Space: the marker is erased and the next page follows.
    lab.console_input(c, b" ");
    let page = output(&mut lab, c);
    assert!(page.starts_with("\x08\x08\x08\x08\x08\x08\x08\x08\x08        \x08\x08\x08\x08\x08\x08\x08\x08\x08"), "{page:?}");
    assert_eq!(page.matches("\r\n").count(), 5, "{page:?}");
    assert!(page.ends_with(" --More-- "));
    // Enter: one more line. q: stop.
    lab.console_input(c, b"\r");
    let line = output(&mut lab, c);
    assert_eq!(line.matches("\r\n").count(), 1, "{line:?}");
    lab.console_input(c, b"q");
    let end = output(&mut lab, c);
    assert!(end.ends_with("SW1#") && !end.contains("More"), "{end:?}");

    typed(&mut lab, c, "terminal length 0");
    let all = typed(&mut lab, c, "show vlan");
    assert!(!all.contains("More") && all.contains("trnet-default"));

    let inc = typed(&mut lab, c, "show running-config | include ^interface Gig");
    assert_eq!(inc, "show running-config | include ^interface Gig\r\ninterface GigabitEthernet0/1\r\ninterface GigabitEthernet0/2\r\nSW1#");
    let count = typed(&mut lab, c, "show running-config | count ^interface");
    assert!(count.contains("Number of lines which match regexp = 27"), "{count:?}");
    let begin = typed(&mut lab, c, "show running-config | begin ^line vty 5");
    assert!(begin.starts_with("show running-config | begin ^line vty 5\r\nline vty 5 15\r\n login\r\n"), "{begin:?}");
    let section = typed(&mut lab, c, "show running-config | section Vlan1");
    assert!(section.contains("interface Vlan1\r\n no ip address\r\n shutdown\r\nSW1#"), "{section:?}");
    let exclude = typed(&mut lab, c, "show vlan brief | exclude default|^$|---|Fa0/|Gi0/");
    assert_eq!(exclude, "show vlan brief | exclude default|^$|---|Fa0/|Gi0/\r\nVLAN Name                             Status    Ports\r\nSW1#");
    assert!(typed(&mut lab, c, "show vlan | frobnicate x").contains("% Invalid input detected"));
}

#[test]
fn configuration_modes_and_prompts() {
    let (mut lab, sw, c) = privileged();
    assert_eq!(
        typed(&mut lab, c, "conf t"),
        "conf t\r\nEnter configuration commands, one per line.  End with CNTL/Z.\r\nSW1(config)#"
    );
    assert!(typed(&mut lab, c, "hostname Access1").ends_with("Access1(config)#"));
    assert!(typed(&mut lab, c, "int g0/1").ends_with("Access1(config-if)#"));
    assert!(typed(&mut lab, c, "interface range fa0/1 - 4 , gi0/2").ends_with("Access1(config-if-range)#"));
    assert!(typed(&mut lab, c, "switchport mode access").ends_with("(config-if-range)#"));
    assert!(typed(&mut lab, c, "vlan 10,20,30-32").ends_with("Access1(config-vlan)#"));
    // A global command typed in a sub-mode leaves the sub-mode.
    assert!(typed(&mut lab, c, "line vty 0 4").ends_with("Access1(config-line)#"));
    assert!(typed(&mut lab, c, "ip access-list standard MGMT").ends_with("Access1(config-std-nacl)#"));
    assert!(typed(&mut lab, c, "ip access-list extended WEB").ends_with("Access1(config-ext-nacl)#"));
    assert!(typed(&mut lab, c, "exit").ends_with("Access1(config)#"));
    // `do` runs EXEC commands without leaving configuration mode.
    let out = typed(&mut lab, c, "do show vlan brief | include VLAN0030");
    assert!(out.contains("30   VLAN0030                         active") && out.ends_with("Access1(config)#"), "{out:?}");
    // `no sh`, and every spelling of an interface name.
    for name in ["g0/1", "gi0/1", "gig 0/1", "GigabitEthernet0/1", "gigabitethernet 0/1"] {
        assert!(typed(&mut lab, c, &format!("interface {name}")).ends_with("(config-if)#"), "{name}");
    }
    typed(&mut lab, c, "shut");
    assert!(lab.running_config(sw).unwrap().contains("interface GigabitEthernet0/1\n shutdown\n"));
    typed(&mut lab, c, "no sh");
    assert!(!lab.running_config(sw).unwrap().contains(" shutdown\n!\ninterface GigabitEthernet0/2"));
    for name in ["vl10", "lo0", "po1"] {
        typed(&mut lab, c, &format!("interface {name}"));
    }
    let run = lab.running_config(sw).unwrap();
    assert!(run.contains("interface Vlan10\n") && run.contains("interface Port-channel1\n"), "{run}");
    assert!(run.contains("interface Loopback0\n no ip address\n!\ninterface Port-channel1\n!\ninterface FastEthernet0/1\n"), "{run}");

    // Ctrl-Z ends configuration mode and logs the event on the console.
    lab.console_input(c, b"\x1a");
    let out = output(&mut lab, c);
    assert!(out.starts_with("^Z\r\nAccess1#"), "{out:?}");
    assert!(out.contains("%SYS-5-CONFIG_I: Configured from console by console"), "{out:?}");
    assert_eq!(lab.snapshot().nodes[0].hostname, "Access1");
    assert_eq!(lab.snapshot().nodes[0].name, "SW1");

    // Unknown but plausible configuration is rejected, not swallowed.
    let lines: Vec<String> = [
        "spanning-tree mode mst",
        "router eigrp 1",
        "interface g9/9",
        "ip address 10.0.0.1",
        "vlan 5000",
        "hostname",
        "switchport mode trunk",
        "ip routing",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let outcome = lab.configure(sw, &lines).unwrap();
    assert!(!outcome.ok);
    assert!(outcome.lines.iter().all(|l| l.error), "{outcome:#?}");
    assert!(outcome.lines[0].output.contains("% Invalid input detected at '^' marker."));
    assert!(outcome.lines[3].output.contains("% Invalid input detected") || outcome.lines[3].output.contains("% Incomplete"));
    assert!(outcome.lines[5].output.contains("% Incomplete command."));
}

#[test]
fn passwords_and_login() {
    let (mut lab, sw, c) = privileged();
    cfg(
        &mut lab,
        sw,
        "enable secret Sup3r\nusername admin privilege 15 secret nocturne\nusername oper password oper123\nline con 0\n password letmein\n login",
    );
    let run = lab.running_config(sw).unwrap();
    assert!(run.contains("enable secret 5 $1$"), "{run}");
    assert!(run.contains("username admin privilege 15 secret 5 $1$"));
    assert!(run.contains("username oper password oper123"));
    assert!(run.contains("line con 0\n password letmein\n login\n"));

    // Logging out of the console brings up the password prompt; input is not echoed.
    let out = typed(&mut lab, c, "exit");
    assert!(out.ends_with("User Access Verification\r\n\r\nPassword: "), "{out:?}");
    lab.console_input(c, b"wrong\r");
    assert_eq!(output(&mut lab, c), "\r\nPassword: ");
    lab.console_input(c, b"letmein\r");
    assert_eq!(output(&mut lab, c), "\r\nSW1>");
    // Three bad enable passwords.
    lab.console_input(c, b"enable\r");
    assert_eq!(output(&mut lab, c), "enable\r\nPassword: ");
    lab.console_input(c, b"a\rb\rc\r");
    let out = output(&mut lab, c);
    assert_eq!(out, "\r\nPassword: \r\nPassword: \r\n% Bad secrets\r\n\r\nSW1>");
    lab.console_input(c, b"enable\rSup3r\r");
    assert!(output(&mut lab, c).ends_with("SW1#"));

    // `login local`: a privilege-15 user lands in privileged EXEC.
    cfg(&mut lab, sw, "line con 0\n login local");
    typed(&mut lab, c, "exit");
    lab.console_input(c, b"admin\rnope\r");
    let out = output(&mut lab, c);
    assert!(out.ends_with("% Login invalid\r\n\r\nUsername: "), "{out:?}");
    lab.console_input(c, b"admin\rnocturne\r");
    let out = output(&mut lab, c);
    assert!(out.ends_with("\r\nSW1#"), "{out:?}");
    typed(&mut lab, c, "exit");
    lab.console_input(c, b"oper\roper123\r");
    assert!(output(&mut lab, c).ends_with("\r\nSW1>"));

    // service password-encryption obfuscates what is already there (type 7).
    cfg(&mut lab, sw, "service password-encryption");
    let run = lab.running_config(sw).unwrap();
    assert!(run.contains("service password-encryption\n") && !run.contains("letmein") && !run.contains("oper123"), "{run}");
    assert!(run.contains("username oper password 7 ") && run.contains(" password 7 "));
    // ...and the obfuscated password still works.
    cfg(&mut lab, sw, "line con 0\n login");
    lab.console_input(c, b"exit\rletmein\r");
    assert!(output(&mut lab, c).ends_with("\r\nSW1>"));
}

#[test]
fn banners_and_log_messages_on_the_console() {
    let (mut lab, sw, c) = privileged();
    typed(&mut lab, c, "conf t");
    let out = typed(&mut lab, c, "banner motd #");
    assert!(out.contains("Enter TEXT message.  End with the character '#'."), "{out:?}");
    lab.console_input(c, b"Authorised access only\rSecond line\r#\r");
    let out = output(&mut lab, c);
    assert!(out.ends_with("SW1(config)#"), "{out:?}");
    typed(&mut lab, c, "banner login ^Keep out^");
    let run = lab.running_config(sw).unwrap();
    assert!(run.contains("banner login ^CKeep out^C\nbanner motd ^C\nAuthorised access only\nSecond line\n^C\n"), "{run}");
    typed(&mut lab, c, "end");
    output(&mut lab, c);
    // A new session sees the banner first.
    let out = typed(&mut lab, c, "exit");
    assert!(out.contains("Authorised access only\r\nSecond line\r\n"), "{out:?}");
    typed(&mut lab, c, "enable");
    typed(&mut lab, c, "terminal length 0");

    // An unsolicited message interrupts the line being typed...
    lab.console_input(c, b"show clo");
    output(&mut lab, c);
    let pc1 = pc(&mut lab, "PC1", "10.0.0.1", None);
    link(&mut lab, pc1, "Ethernet0", sw, "fa0/5");
    let out = output(&mut lab, c);
    assert!(out.starts_with("\r\n*Mar  1 00:00:00.000: %LINK-3-UPDOWN: Interface FastEthernet0/5, changed state to up\r\n"), "{out:?}");
    assert!(out.contains("%LINEPROTO-5-UPDOWN: Line protocol on Interface FastEthernet0/5, changed state to up\r\n"));
    assert!(!out.ends_with("show clo"), "without logging synchronous the line is not redrawn");
    // ...but the buffer is intact.
    lab.console_input(c, b"ck\r");
    assert!(output(&mut lab, c).contains("UTC Mon Mar 1 1993"));

    // With logging synchronous the prompt and partial line are printed again.
    cfg(&mut lab, sw, "line con 0\n logging synchronous");
    output(&mut lab, c);
    lab.console_input(c, b"show ver");
    output(&mut lab, c);
    lab.set_power(pc1, false);
    let out = output(&mut lab, c);
    assert!(out.contains("changed state to down\r\n") && out.ends_with("SW1#show ver"), "{out:?}");
    lab.console_input(c, b"\x15");
    output(&mut lab, c);

    // `no logging console` silences the console but not the buffer or the events.
    cfg(&mut lab, sw, "no logging console");
    output(&mut lab, c);
    lab.set_power(pc1, true);
    assert_eq!(output(&mut lab, c), "");
    assert!(typed(&mut lab, c, "show logging").contains("changed state to up"));
}

#[test]
fn long_running_commands_can_be_interrupted() {
    let (mut lab, sw, c) = privileged();
    cfg(&mut lab, sw, "interface vlan 1\n ip address 10.0.0.2 255.255.255.0\n no shutdown");
    let pc1 = pc(&mut lab, "PC1", "10.0.0.1", None);
    link(&mut lab, pc1, "Ethernet0", sw, "fa0/1");
    lab.settle(30 * SECOND);
    output(&mut lab, c);
    // Nothing answers at 10.0.0.9: dots appear as timeouts expire in simulated time.
    lab.console_input(c, b"ping 10.0.0.9 repeat 50\r");
    let out = output(&mut lab, c);
    assert!(out.ends_with("timeout is 2 seconds:\r\n"), "{out:?}");
    lab.advance(5 * SECOND);
    assert_eq!(output(&mut lab, c), "..");
    // Ctrl-Shift-6 stops it and the summary counts what was sent so far.
    lab.console_input(c, b"\x1e");
    let out = output(&mut lab, c);
    assert_eq!(out, "\r\nSuccess rate is 0 percent (0/2)\r\nSW1#", "{out:?}");
    // The prompt is back and usable; a real ping completes on its own.
    lab.console_input(c, b"ping 10.0.0.1\r");
    lab.advance(10 * SECOND);
    let out = output(&mut lab, c);
    assert!(out.contains("\r\n.!!!!\r\nSuccess rate is 80 percent (4/5)") && out.ends_with("SW1#"), "{out:?}");
}
