//! Scratch driver: build a tiny lab and replay console input given as arguments.
//!
//! `cargo run -p nocturne-netsim --example console -- "show version" "conf t" ...`
//! Arguments are typed on SW1's console, each followed by Enter. An argument
//! starting with `+` advances simulated time by that many seconds instead, and
//! one starting with `~` is typed without the trailing Enter (e.g. `~show ?`).

use nocturne_netsim::{CableKind, HostConfig, Lab};

fn main() {
    let mut lab = Lab::new("scratch");
    let sw = lab.add_node("switch", Some("SW1"), 0.0, 0.0).unwrap();
    let r1 = lab.add_node("router", Some("R1"), 0.0, 0.0).unwrap();
    let pc = lab.add_node("pc", Some("PC1"), 0.0, 0.0).unwrap();
    lab.set_host_config(
        pc,
        HostConfig {
            ip: Some("192.168.1.10".into()),
            mask: Some("255.255.255.0".into()),
            gateway: Some("192.168.1.1".into()),
            ..HostConfig::default()
        },
    )
    .unwrap();
    lab.add_link(pc, "Ethernet0", sw, "fa0/1", CableKind::Auto)
        .unwrap();
    lab.add_link(r1, "g0/0", sw, "gi0/1", CableKind::Auto)
        .unwrap();
    lab.configure(
        r1,
        &[
            "interface g0/0".to_string(),
            "ip address 192.168.1.1 255.255.255.0".to_string(),
            "no shutdown".to_string(),
        ],
    )
    .unwrap();
    lab.settle(60_000_000);
    let target = match std::env::var("NODE").as_deref() {
        Ok("R1") => r1,
        Ok("PC1") => pc,
        _ => sw,
    };
    let console = lab.console_open(target).unwrap();
    let mut transcript = lab.console_output(console);
    for arg in std::env::args().skip(1) {
        if let Some(seconds) = arg.strip_prefix('+') {
            lab.advance(seconds.parse::<u64>().unwrap_or(1) * 1_000_000);
        } else if let Some(text) = arg.strip_prefix('~') {
            lab.console_input(console, text.as_bytes());
        } else {
            lab.console_input(console, arg.as_bytes());
            lab.console_input(console, b"\r");
        }
        transcript.extend(lab.console_output(console));
    }
    print!("{}", String::from_utf8_lossy(&transcript).replace('\r', ""));
    println!();
}
