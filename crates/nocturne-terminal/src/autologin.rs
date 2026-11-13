//! Answering the login prompts of a session from a stored credential.
//!
//! Telnet servers, serial consoles and simulated devices ask for a user name
//! and a password inside the data stream. [`AutoLogin`] watches the first
//! moments of a session and answers those two questions, once.
//!
//! It is deliberately timid. A password typed at anything but a password
//! prompt is echoed, logged and kept in a shell history, and a second attempt
//! with a wrong password locks accounts. So an answer is only ever given
//!
//! * to the line the device is waiting on, after it has been quiet for a
//!   moment (text that merely scrolls past is not a prompt),
//! * before the first command prompt has been seen and before the person at
//!   the keyboard has typed anything,
//! * within the first minute, and never twice.
//!
//! Anything else ends the watch and leaves the session to its user.

use std::time::Duration;

use nocturne_drivers::profile::Cli;
use nocturne_drivers::term::Screen;
use regex::Regex;
use tokio::time::Instant;
use zeroize::Zeroizing;

/// How long after a session opens a login prompt is still answered.
const WINDOW: Duration = Duration::from_secs(60);
/// How long a prompt must have been the last thing printed before it counts
/// as the device waiting for input.
const SETTLE: Duration = Duration::from_millis(250);
/// More output than this before a login prompt is no login banner.
const MAX_PREAMBLE: usize = 64 * 1024;

/// What a login prompt is answered with.
pub(crate) struct Credentials {
    pub username: Option<String>,
    pub password: Option<Zeroizing<String>>,
}

/// How a platform's login looks, from its profile.
struct Dialect {
    username_prompt: Regex,
    password_prompt: Regex,
    cli_prompt: Regex,
    /// A password prompt that is not asking for the login password: a
    /// first-boot wizard, an expired password, `enable`, `sudo`.
    not_login: Regex,
    username_suffix: String,
    newline: String,
}

impl Dialect {
    fn from_profile(cli: &Cli) -> Result<Dialect, regex::Error> {
        Ok(Dialect {
            username_prompt: Regex::new(&cli.login.username_prompt)?,
            password_prompt: Regex::new(&cli.login.password_prompt)?,
            cli_prompt: Regex::new(&cli.prompt)?,
            not_login: Regex::new(
                r"(?i)\b(new|old|current|confirm|retype|re-?enter|again|verify|change|expired|enable|secret|sudo|passphrase)\b",
            )?,
            username_suffix: cli.login.username_suffix.clone(),
            newline: cli.newline.clone(),
        })
    }
}

/// What the line the device is waiting on looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Showing {
    Username,
    Password,
    /// A command prompt: the login is over, or there never was one.
    Cli,
    /// A password prompt that must not be answered.
    Foreign,
}

pub(crate) struct AutoLogin {
    dialect: Dialect,
    credentials: Credentials,
    screen: Screen,
    deadline: Instant,
    /// What the current line looks like, and since when nothing else was printed.
    showing: Option<(Showing, Instant)>,
    username_sent: bool,
    observed: usize,
    finished: bool,
}

impl AutoLogin {
    /// `None` when there is nothing to answer with, or the profile's prompt
    /// patterns do not compile.
    pub fn new(cli: &Cli, credentials: Credentials, now: Instant) -> Option<AutoLogin> {
        if credentials.username.is_none() && credentials.password.is_none() {
            return None;
        }
        let dialect = match Dialect::from_profile(cli) {
            Ok(dialect) => dialect,
            Err(err) => {
                tracing::debug!(%err, "login prompts of the profile do not compile; not logging in automatically");
                return None;
            }
        };
        Some(AutoLogin {
            dialect,
            credentials,
            screen: Screen::new(),
            deadline: now + WINDOW,
            showing: None,
            username_sent: false,
            observed: 0,
            finished: false,
        })
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Take note of output from the device.
    pub fn observe(&mut self, data: &[u8], now: Instant) {
        if self.finished {
            return;
        }
        self.observed += data.len();
        if now >= self.deadline || self.observed > MAX_PREAMBLE {
            self.finish();
            return;
        }
        self.screen.feed(data);
        // Only the line being drawn can be a prompt; completed lines are history.
        self.screen.take_lines();
        let line = self.screen.current_line();
        self.showing = self.classify(&line).map(|showing| (showing, now));
    }

    fn classify(&self, line: &str) -> Option<Showing> {
        let dialect = &self.dialect;
        if dialect.password_prompt.is_match(line) {
            Some(if dialect.not_login.is_match(line) {
                Showing::Foreign
            } else {
                Showing::Password
            })
        } else if dialect.username_prompt.is_match(line) {
            Some(Showing::Username)
        } else if dialect.cli_prompt.is_match(line) {
            Some(Showing::Cli)
        } else {
            None
        }
    }

    /// When [`act`](Self::act) wants to be called next.
    pub fn due(&self) -> Option<Instant> {
        if self.finished {
            return None;
        }
        Some(match self.showing {
            Some((_, since)) => since + SETTLE,
            None => self.deadline,
        })
    }

    /// What to send now, if the device has settled on a login prompt.
    pub fn act(&mut self, now: Instant) -> Option<Zeroizing<Vec<u8>>> {
        if self.finished {
            return None;
        }
        if now >= self.deadline {
            self.finish();
            return None;
        }
        let (showing, since) = self.showing?;
        if now < since + SETTLE {
            return None;
        }
        self.showing = None;
        match showing {
            Showing::Cli | Showing::Foreign => {
                self.finish();
                None
            }
            Showing::Username => {
                // Asked again: the first answer was not accepted, and guessing on would lock the account.
                let username = self
                    .credentials
                    .username
                    .as_deref()
                    .filter(|_| !self.username_sent);
                let Some(username) = username else {
                    self.finish();
                    return None;
                };
                let reply = self.line(&format!("{username}{}", self.dialect.username_suffix));
                self.username_sent = true;
                // The answered prompt must not be mistaken for a new one while its echo arrives.
                self.screen.clear_current_line();
                Some(reply)
            }
            Showing::Password => {
                let reply = self
                    .credentials
                    .password
                    .as_deref()
                    .map(|password| self.line(password));
                self.finish();
                reply
            }
        }
    }

    fn line(&self, text: &str) -> Zeroizing<Vec<u8>> {
        let mut line = Zeroizing::new(Vec::with_capacity(text.len() + self.dialect.newline.len()));
        line.extend_from_slice(text.as_bytes());
        line.extend_from_slice(self.dialect.newline.as_bytes());
        line
    }

    fn finish(&mut self) {
        self.finished = true;
        self.showing = None;
        // Dropping the secret wipes it.
        self.credentials.password = None;
    }
}

/// Whether what the user typed is more than a keypress to wake a console up.
pub(crate) fn is_real_input(data: &[u8]) -> bool {
    data.iter().any(|byte| !matches!(byte, b'\r' | b'\n'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const AFTER_SETTLE: Duration = Duration::from_millis(300);

    fn credentials() -> Credentials {
        Credentials {
            username: Some("admin".into()),
            password: Some(Zeroizing::new("s3cr3t".into())),
        }
    }

    struct Dialogue {
        login: AutoLogin,
        now: Instant,
        sent: Vec<String>,
    }

    impl Dialogue {
        fn new(credentials: Credentials) -> Dialogue {
            let now = Instant::now();
            Dialogue {
                login: AutoLogin::new(&Cli::default(), credentials, now).expect("auto-login"),
                now,
                sent: Vec::new(),
            }
        }

        /// The device prints `text`, then stays quiet for `quiet`.
        fn device(&mut self, text: &str, quiet: Duration) -> &mut Dialogue {
            self.login.observe(text.as_bytes(), self.now);
            self.now += quiet;
            if let Some(reply) = self.login.act(self.now) {
                self.sent
                    .push(String::from_utf8(reply.to_vec()).expect("text"));
            }
            self
        }
    }

    #[test]
    fn answers_username_then_password_once_each() {
        let mut dialogue = Dialogue::new(credentials());
        dialogue
            .device("\r\nUser Access Verification\r\n\r\nUsername: ", AFTER_SETTLE)
            .device("admin\r\nPassword: ", AFTER_SETTLE);
        assert_eq!(dialogue.sent, ["admin\n", "s3cr3t\n"]);
        assert!(dialogue.login.is_finished());
        // Whatever follows — a rejection, another prompt — is the user's business.
        dialogue.device("\r\n% Login invalid\r\n\r\nUsername: ", AFTER_SETTLE);
        dialogue.device("Password: ", AFTER_SETTLE);
        assert_eq!(dialogue.sent.len(), 2);
    }

    #[test]
    fn a_line_password_without_user_name_is_answered() {
        let mut dialogue = Dialogue::new(credentials());
        dialogue.device("User Access Verification\r\n\r\nPassword: ", AFTER_SETTLE);
        assert_eq!(dialogue.sent, ["s3cr3t\n"]);
    }

    #[test]
    fn a_command_prompt_ends_the_watch_before_any_password_prompt() {
        let mut dialogue = Dialogue::new(credentials());
        dialogue.device("\r\nSW1>", AFTER_SETTLE);
        assert!(dialogue.login.is_finished());
        // `enable` asks for a password; it is not the login password's to answer.
        dialogue.device("enable\r\nPassword: ", AFTER_SETTLE);
        assert_eq!(dialogue.sent, Vec::<String>::new());
    }

    #[test]
    fn text_scrolling_past_is_not_a_prompt() {
        let mut dialogue = Dialogue::new(credentials());
        // The banner mentions a password, then goes on within the settle time.
        dialogue
            .device("Unauthorised use is an offence. Password: ", Duration::from_millis(50))
            .device("s are rotated monthly.\r\n", Duration::from_millis(50));
        assert_eq!(dialogue.sent, Vec::<String>::new());
        assert!(!dialogue.login.is_finished());
        dialogue.device("login: ", AFTER_SETTLE);
        assert_eq!(dialogue.sent, ["admin\n"]);
    }

    #[test]
    fn a_repeated_username_prompt_is_not_answered_again() {
        let mut dialogue = Dialogue::new(credentials());
        dialogue
            .device("Username: ", AFTER_SETTLE)
            .device("admin\r\n% Unknown user\r\nUsername: ", AFTER_SETTLE);
        assert_eq!(dialogue.sent, ["admin\n"]);
        assert!(dialogue.login.is_finished());
    }

    #[test]
    fn password_prompts_that_are_not_a_login_are_left_alone() {
        for prompt in [
            "Enter new password: ",
            "Your password has expired.\r\nNew Password: ",
            "Retype password: ",
            "Enter enable password: ",
            "Current password: ",
        ] {
            let mut dialogue = Dialogue::new(credentials());
            dialogue.device(prompt, AFTER_SETTLE);
            assert_eq!(dialogue.sent, Vec::<String>::new(), "{prompt}");
            assert!(dialogue.login.is_finished(), "{prompt}");
        }
    }

    #[test]
    fn nothing_is_answered_after_the_first_minute_or_a_long_preamble() {
        let mut late = Dialogue::new(credentials());
        late.device("\r\n", WINDOW);
        late.device("Username: ", AFTER_SETTLE);
        assert_eq!(late.sent, Vec::<String>::new());
        assert!(late.login.is_finished());

        let mut chatty = Dialogue::new(credentials());
        chatty.device(&"x".repeat(MAX_PREAMBLE + 1), Duration::ZERO);
        chatty.device("\r\nPassword: ", AFTER_SETTLE);
        assert_eq!(chatty.sent, Vec::<String>::new());
    }

    #[test]
    fn missing_parts_of_the_credential_are_never_guessed() {
        let mut no_user = Dialogue::new(Credentials {
            username: None,
            password: Some(Zeroizing::new("s3cr3t".into())),
        });
        no_user.device("Username: ", AFTER_SETTLE);
        assert_eq!(no_user.sent, Vec::<String>::new());
        assert!(no_user.login.is_finished());

        let mut no_password = Dialogue::new(Credentials {
            username: Some("admin".into()),
            password: None,
        });
        no_password
            .device("Username: ", AFTER_SETTLE)
            .device("admin\r\nPassword: ", AFTER_SETTLE);
        assert_eq!(no_password.sent, ["admin\n"]);

        assert!(
            AutoLogin::new(
                &Cli::default(),
                Credentials {
                    username: None,
                    password: None
                },
                Instant::now()
            )
            .is_none()
        );
    }

    #[test]
    fn the_profile_decides_suffix_and_line_ending() {
        let mut cli = Cli::default();
        cli.login.username_suffix = "+ct".into();
        cli.newline = "\r".into();
        let now = Instant::now();
        let mut login = AutoLogin::new(&cli, credentials(), now).unwrap();
        login.observe(b"MikroTik Login: ", now);
        assert_eq!(login.due(), Some(now + SETTLE));
        assert_eq!(login.act(now + Duration::from_millis(100)), None);
        let reply = login.act(now + SETTLE).unwrap();
        assert_eq!(reply.as_slice(), b"admin+ct\r");
    }

    #[test]
    fn only_a_bare_return_counts_as_waking_the_console() {
        assert!(!is_real_input(b"\r"));
        assert!(!is_real_input(b"\r\n\r\n"));
        assert!(is_real_input(b"a"));
        assert!(is_real_input(b"\x03"));
        assert!(is_real_input(b"\radmin"));
    }
}
