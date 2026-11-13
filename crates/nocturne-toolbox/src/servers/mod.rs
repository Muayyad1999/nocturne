//! The two servers: TFTP, and a syslog listener for a device being staged.
//!
//! These are the one part of the toolbox that *listens*, so they are the part
//! to be careful with:
//!
//! 1. **Off unless asked.** Nothing here binds at start-up, and no other code
//!    path in this crate starts them.
//! 2. **Bound where the settings say** — an unprivileged port by default,
//!    because binding below 1024 needs rights the application does not have.
//! 3. **A server is a job.** It appears in the job list like everything else,
//!    `tftp.stop` cancels that job, and the job ends when the application
//!    shuts down, which is what closes the socket. A server cannot be left
//!    running by accident because there is nowhere for it to hide.
//! 4. **A port that cannot be bound is an error with the reason in it** — in
//!    use, or needing privileges, with what to do about it — never a server
//!    that looks as though it started.
//! 5. **Local access only.** Both are refused in server mode: they serve files
//!    from, and listen on, the machine Nocturne runs on, and in a browser
//!    that machine is not the user's.

use nocturne_app::Router;

pub mod syslog;
pub mod tftp;

pub fn register(router: &mut Router) {
    tftp::register(router);
    syslog::register(router);
}
