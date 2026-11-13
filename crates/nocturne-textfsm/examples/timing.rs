//! Measures compiling a bundled template and parsing a large output.
//!
//! ```text
//! cargo run --release -p nocturne-textfsm --example timing
//! ```
//!
//! The input is the `show interfaces` fixture repeated until it is at least
//! 5,000 lines long.

use std::time::Instant;

use nocturne_textfsm::{Template, ntc};

const TEMPLATE: &str = "cisco_ios_show_interfaces";
const FIXTURE: &str = "tests/fixtures/cisco_ios/show_interfaces/cisco_ios_show_interfaces_01.raw";
const LINES: usize = 5_000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sample =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE))?;
    let repeats = LINES.div_ceil(sample.lines().count());
    let output = sample.repeat(repeats);

    let started = Instant::now();
    let source = ntc::source(TEMPLATE).ok_or("template not bundled")?;
    let unpack = started.elapsed();

    let started = Instant::now();
    let template = Template::parse(source)?;
    let compile = started.elapsed();

    let started = Instant::now();
    let rows = template.run(&output)?;
    let run = started.elapsed();

    let started = Instant::now();
    let again = template.run(&output)?;
    let rerun = started.elapsed();
    assert_eq!(rows, again);

    println!("bundle unpack (first use):     {unpack:?}");
    println!("compile {TEMPLATE}: {compile:?}");
    println!(
        "parse {} lines -> {} rows:   {run:?} (second run {rerun:?})",
        output.lines().count(),
        rows.len()
    );
    println!("compile + parse:               {:?}", compile + run);
    Ok(())
}
