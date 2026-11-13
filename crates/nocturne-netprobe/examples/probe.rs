//! A small command-line front end to the probes, for trying them by hand:
//!
//! ```text
//! cargo run -p nocturne-netprobe --example probe -- local
//! ```

use nocturne_netprobe::local;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["local"] => {
            print(&local::interfaces());
            print(&local::neighbours().await);
            print(&local::default_gateways().await);
        }
        _ => eprintln!("usage: probe local"),
    }
}

fn print<T: serde::Serialize>(value: &T) {
    println!("{}", serde_json::to_string_pretty(value).expect("results serialize to JSON"));
}
