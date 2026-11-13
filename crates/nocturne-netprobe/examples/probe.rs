//! A small command-line front end to the probes, for trying them by hand:
//!
//! ```text
//! cargo run -p nocturne-netprobe --example probe -- ping 192.0.2.1
//! cargo run -p nocturne-netprobe --example probe -- trace example.com
//! ```

use std::time::Duration;

use nocturne_netprobe::ping::{PingOptions, Pinger};
use nocturne_netprobe::sweep::SweepOptions;
use nocturne_netprobe::tcp::ScanOptions;
use nocturne_netprobe::traceroute::TraceOptions;
use nocturne_netprobe::{ProbeError, dns, local, ping, sweep, tcp, traceroute};

const USAGE: &str = "usage: probe <command> [arguments]
  local                         interfaces, neighbours, default gateways
  ping <host>                   four echo requests
  sweep <targets>               host discovery (10.0.0.0/24, 10.0.0.5-40, ...)
  trace <host>                  traceroute
  scan <host> [ports]           TCP connect scan (default: common)
  dns <name> [type] [server]    DNS lookup
  wake <mac> [broadcast]        Wake-on-LAN";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    if let Err(error) = run(&args).await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run(args: &[&str]) -> Result<(), ProbeError> {
    match args {
        ["local"] => {
            print(&local::interfaces());
            print(&local::neighbours().await);
            print(&local::default_gateways().await);
        }
        ["ping", host] => {
            let pinger = Pinger::new();
            print(&ping::ping(&pinger, host, &PingOptions::default(), None).await?);
        }
        ["sweep", targets] => {
            let pinger = Pinger::new();
            let options = SweepOptions {
                resolve_names: true,
                ..SweepOptions::default()
            };
            print(&sweep::sweep(&pinger, targets, &options, None).await?);
        }
        ["trace", host] => {
            let options = TraceOptions {
                resolve_names: true,
                ..TraceOptions::default()
            };
            print(&traceroute::traceroute(host, &options, None).await?);
        }
        ["scan", host, rest @ ..] => {
            let ports = tcp::parse_ports(rest.first().unwrap_or(&"common"))?;
            let results = tcp::scan(host, &ports, &ScanOptions::default(), None).await?;
            let answered: Vec<_> = results
                .into_iter()
                .filter(|result| result.state == tcp::PortState::Open)
                .collect();
            print(&answered);
        }
        ["dns", name, rest @ ..] => {
            let kind = rest.first().unwrap_or(&"A");
            let server = rest.get(1).copied();
            print(&dns::lookup(name, kind, server, Duration::from_secs(5)).await?);
        }
        ["wake", mac, rest @ ..] => {
            print(&local::wake_on_lan(mac, rest.first().copied()).await?);
        }
        _ => eprintln!("{USAGE}"),
    }
    Ok(())
}

fn print<T: serde::Serialize>(value: &T) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("results serialize to JSON")
    );
}
