//! The two editions as two programs on one machine.
//!
//! The ISP edition is the same source with one more feature compiled in and
//! `tauri.isp.conf.json` laid over `tauri.conf.json`. It is sold as a
//! separate application that installs beside the free one, and that is only
//! true while nothing an installer puts on a disk has the same name in both.
//! The product name and the identifier differ by themselves. The program's
//! file name does not: Cargo calls it `nocturne` whatever is compiled in,
//! and under one name the second Linux package is refused over the first
//! one's `/usr/bin/nocturne`, launcher and icons.
//!
//! What the packager names after what (tauri-bundler 2):
//!
//! | On disk | Named after |
//! |---|---|
//! | Debian/RPM package, `.desktop` file, install folder, installer files | `productName` |
//! | `/usr/bin/…`, `Exec=`, `Icon=`, icon files, the `.exe`, what the Windows installer looks for among running programs | the main binary: `mainBinaryName`, else Cargo's `[[bin]]` name |
//! | bundle id, WebView data, the single-instance name | `identifier` |

use serde_json::Value;

fn free() -> Value {
    serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json is JSON")
}

/// The ISP edition's configuration as the packager sees it: its file laid
/// over the base one, key by key.
fn isp() -> Value {
    let mut merged = free();
    let over: Value = serde_json::from_str(include_str!("../tauri.isp.conf.json"))
        .expect("tauri.isp.conf.json is JSON");
    for (key, value) in over.as_object().expect("an object") {
        merged[key] = value.clone();
    }
    merged
}

/// The name Cargo gives the program, which is what an edition is called
/// unless its configuration says otherwise.
fn cargo_binary() -> String {
    let manifest = include_str!("../Cargo.toml");
    let target = manifest
        .split("[[bin]]")
        .nth(1)
        .expect("Cargo.toml declares the program as a [[bin]]");
    let name = target
        .lines()
        .find_map(|line| line.trim().strip_prefix("name = "))
        .expect("the [[bin]] has a name");
    name.trim_matches('"').to_string()
}

fn main_binary(config: &Value) -> String {
    config["mainBinaryName"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(cargo_binary)
}

/// How the packager turns a product name into a package name.
fn package(config: &Value) -> String {
    config["productName"]
        .as_str()
        .expect("a product name")
        .to_lowercase()
        .replace(' ', "-")
}

#[test]
fn both_editions_can_be_installed_on_one_machine() {
    let (free, isp) = (free(), isp());

    assert_ne!(
        main_binary(&free),
        main_binary(&isp),
        "both editions would install the same program file (and the launcher and icons named \
         after it): the second package is refused, or replaces the first one's program"
    );
    assert_ne!(package(&free), package(&isp));
    assert_ne!(free["productName"], isp["productName"]);
    assert_ne!(
        free["identifier"], isp["identifier"],
        "one identifier would make the two editions each other's \"already running\" copy"
    );
}

#[test]
fn the_isp_edition_is_found_under_its_own_name() {
    // What somebody types, and what a launcher's `Exec=` says.
    assert_eq!(main_binary(&free()), "nocturne");
    assert_eq!(main_binary(&isp()), "nocturne-isp");
    assert_eq!(package(&isp()), "nocturne-isp");
}
