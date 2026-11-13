//! Remote sessions: telnet and SSH from a console into another device.

mod common;

use common::*;
use nocturne_netsim::{ConsoleId, Lab};

/// PC1 -- R1 -- R2; returns the lab, the routers and PC1.
fn chain() -> (Lab, [u32; 2], u32) {
    let mut lab = Lab::new("sessions");
    let r1 = node(&mut lab, "router", "R1");
    let r2 = node(&mut lab, "router", "R2");
    let pc1 = pc(&mut lab, "PC1", "10.1.0.10", Some("10.1.0.1"));
    link(&mut lab, pc1, "Ethernet0", r1, "g0/0");
    link(&mut lab, r1, "g0/1", r2, "g0/0");
    cfg(
        &mut lab,
        r1,
        "interface g0/0\n ip address 10.1.0.1 255.255.255.0\n no shutdown\ninterface g0/1\n ip address 10.0.12.1 255.255.255.252\n no shutdown",
    );
    cfg(
        &mut lab,
        r2,
        "interface g0/0\n ip address 10.0.12.2 255.255.255.252\n no shutdown\nip route 10.1.0.0 255.255.255.0 10.0.12.1",
    );
    lab.settle(60 * SECOND);
    // Warm the ARP caches so connections open on the first try.
    exec(&mut lab, pc1, "ping 10.0.12.2");
    (lab, [r1, r2], pc1)
}

/// Type a line and let simulated time pass so the remote end can answer.
fn say(lab: &mut Lab, console: ConsoleId, text: &str) -> String {
    lab.console_input(console, text.as_bytes());
    lab.console_input(console, b"\r");
    lab.advance(2 * SECOND);
    output(lab, console)
}

#[test]
fn telnet_with_line_password_and_enable_secret() {
    let (mut lab, [r1, r2], _) = chain();
    let c = lab.console_open(r1).unwrap();
    output(&mut lab, c);
    say(&mut lab, c, "enable");
    // A vty with `login` but no password lets nobody in.
    let out = say(&mut lab, c, "telnet 10.0.12.2");
    assert_eq!(
        out,
        "telnet 10.0.12.2\r\nTrying 10.0.12.2 ... Open\r\n\r\nPassword required, but none set\r\n\r\n[Connection to 10.0.12.2 closed by foreign host]\r\nR1#"
    );

    cfg(
        &mut lab,
        r2,
        "enable secret cisco\nline vty 0 4\n password vtypass\n login",
    );
    let out = say(&mut lab, c, "telnet 10.0.12.2");
    assert_eq!(
        out,
        "telnet 10.0.12.2\r\nTrying 10.0.12.2 ... Open\r\n\r\nUser Access Verification\r\n\r\nPassword: "
    );
    assert_eq!(
        say(&mut lab, c, "vtypass"),
        "\r\nR2>",
        "the password is not echoed"
    );
    assert_eq!(say(&mut lab, c, "enable"), "enable\r\nPassword: ");
    assert_eq!(say(&mut lab, c, "cisco"), "\r\nR2#");
    // The session shows up on the far device, with where it came from.
    let users = say(&mut lab, c, "show users");
    assert!(
        users.contains("*  1 vty 0                idle                 00:00:0")
            && users.contains("10.0.12.1\r\n"),
        "{users:?}"
    );
    assert!(
        exec(&mut lab, r2, "show users").contains("   1 vty 0 "),
        "seen from R2 itself"
    );
    // Configuration typed through the session lands on R2.
    say(&mut lab, c, "configure terminal");
    assert_eq!(
        say(&mut lab, c, "hostname Edge"),
        "hostname Edge\r\nEdge(config)#"
    );
    say(&mut lab, c, "end");
    assert!(lab.running_config(r2).unwrap().contains("hostname Edge\n"));
    assert_eq!(
        say(&mut lab, c, "exit"),
        "exit\r\n\r\n[Connection to 10.0.12.2 closed by foreign host]\r\nR1#"
    );
    assert!(!exec(&mut lab, r2, "show users").contains("vty 0"));

    // Three wrong passwords and the far end hangs up.
    say(&mut lab, c, "telnet 10.0.12.2");
    say(&mut lab, c, "a");
    say(&mut lab, c, "b");
    let out = say(&mut lab, c, "c");
    assert!(
        out.contains("% Bad passwords")
            && out.ends_with("[Connection to 10.0.12.2 closed by foreign host]\r\nR1#"),
        "{out:?}"
    );

    // Nobody answers on an address that does not exist; a closed port refuses.
    let out = say(&mut lab, c, "telnet 10.0.12.3");
    lab.advance(40 * SECOND);
    let out = out + &output(&mut lab, c);
    assert!(
        out.contains(
            "Trying 10.0.12.3 ... \r\n% Connection timed out; remote host not responding\r\nR1#"
        ),
        "{out:?}"
    );
    let out = say(&mut lab, c, "telnet 10.0.12.2 8080");
    assert!(
        out.contains("% Connection refused by remote host"),
        "{out:?}"
    );
}

#[test]
fn ssh_local_users_and_access_classes() {
    let (mut lab, [r1, r2], pc1) = chain();
    let c = lab.console_open(r1).unwrap();
    output(&mut lab, c);
    say(&mut lab, c, "enable");
    cfg(
        &mut lab,
        r2,
        "username admin privilege 15 secret nocturne\nusername oper secret oper123\nline vty 0 4\n login local\n transport input ssh",
    );
    // Telnet is no longer accepted, and SSH needs a host key first.
    assert!(say(&mut lab, c, "telnet 10.0.12.2").contains("% Connection refused by remote host"));
    assert!(
        say(&mut lab, c, "ssh -l admin 10.0.12.2").contains("% Connection refused by remote host")
    );
    let outcome = lab
        .configure(r2, &["crypto key generate rsa modulus 1024".into()])
        .unwrap();
    assert!(
        outcome.lines[0]
            .output
            .contains("% Please define a domain-name first."),
        "{outcome:?}"
    );
    cfg(
        &mut lab,
        r2,
        "ip domain-name lab.local\ncrypto key generate rsa modulus 1024",
    );

    assert_eq!(
        say(&mut lab, c, "ssh -l admin 10.0.12.2"),
        "ssh -l admin 10.0.12.2\r\nTrying 10.0.12.2 ... Open\r\nPassword: "
    );
    assert_eq!(
        say(&mut lab, c, "nocturne"),
        "\r\nR2#",
        "privilege 15 lands in privileged EXEC"
    );
    let ssh = say(&mut lab, c, "show ssh");
    assert!(ssh.contains("Session started       admin"), "{ssh:?}");
    assert!(say(&mut lab, c, "show users").contains("vty 0     admin      idle"));
    say(&mut lab, c, "exit");
    // An ordinary user starts in user EXEC.
    say(&mut lab, c, "ssh -l oper 10.0.12.2");
    assert_eq!(say(&mut lab, c, "oper123"), "\r\nR2>");
    say(&mut lab, c, "exit");

    // An access class limits who may open a session at all.
    cfg(
        &mut lab,
        r2,
        "access-list 10 permit 10.1.0.0 0.0.0.255\nline vty 0 4\n access-class 10 in",
    );
    assert!(
        say(&mut lab, c, "ssh -l admin 10.0.12.2").contains("% Connection refused by remote host")
    );
    assert!(!lab.probe_tcp(r1, "10.0.12.2".parse().unwrap(), 22).ok);
    assert!(lab.probe_tcp(pc1, "10.0.12.2".parse().unwrap(), 22).ok);

    // From the host: through R1 into R2, two sessions deep.
    cfg(
        &mut lab,
        r1,
        "line vty 0 4\n password r1pass\n login\n transport input telnet",
    );
    let h = lab.console_open(pc1).unwrap();
    assert_eq!(output(&mut lab, h), "\r\nC:\\>");
    let out = say(&mut lab, h, "telnet 10.1.0.1");
    assert!(
        out.ends_with("User Access Verification\r\n\r\nPassword: "),
        "{out:?}"
    );
    assert_eq!(say(&mut lab, h, "r1pass"), "\r\nR1>");
    // R1 itself is not in the permitted range, so the second hop is refused...
    assert!(
        say(&mut lab, h, "ssh -l admin 10.0.12.2").contains("% Connection refused by remote host")
    );
    cfg(&mut lab, r2, "access-list 10 permit host 10.0.12.1");
    // ...until it is.
    say(&mut lab, h, "ssh -l admin 10.0.12.2");
    assert_eq!(say(&mut lab, h, "nocturne"), "\r\nR2#");
    assert!(say(&mut lab, h, "show users").contains("10.0.12.1\r\n"));
    assert_eq!(
        say(&mut lab, h, "exit"),
        "exit\r\n\r\n[Connection to 10.0.12.2 closed by foreign host]\r\nR1>"
    );
    assert_eq!(
        say(&mut lab, h, "exit"),
        "exit\r\n\r\n[Connection to 10.1.0.1 closed by foreign host]\r\nC:\\>"
    );
    assert!(!exec(&mut lab, r1, "show users").contains("vty"));

    // An idle remote session is closed by the far end's exec-timeout.
    cfg(&mut lab, r1, "line vty 0 4\n exec-timeout 1 0");
    say(&mut lab, h, "telnet 10.1.0.1");
    assert_eq!(say(&mut lab, h, "r1pass"), "\r\nR1>");
    lab.advance(70 * SECOND);
    let out = output(&mut lab, h);
    assert!(
        out.ends_with("[Connection to 10.1.0.1 closed by foreign host]\r\nC:\\>"),
        "{out:?}"
    );

    // Powering the far device off ends the session too.
    say(&mut lab, h, "telnet 10.1.0.1");
    say(&mut lab, h, "r1pass");
    lab.set_power(r1, false);
    lab.advance(SECOND);
    let out = output(&mut lab, h);
    assert!(
        out.ends_with("[Connection to 10.1.0.1 closed by foreign host]\r\nC:\\>"),
        "{out:?}"
    );
}
