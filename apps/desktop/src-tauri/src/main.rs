// The interface is the window, not a terminal: no console on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    nocturne_desktop::run();
}
