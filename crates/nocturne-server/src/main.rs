//! The `nocturne-server` binary: runs the server, and doubles as a small
//! client for scripts (`call`, `methods`, `token`).

#![forbid(unsafe_code)]

mod cli;

fn main() -> std::process::ExitCode {
    cli::run()
}
