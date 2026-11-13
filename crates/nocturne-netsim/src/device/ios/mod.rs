//! The IOS-style command-line personality.
//!
//! This module owns the modes, prompts, login flow and command dispatch; the
//! command tables live in the sub-modules. Handlers only see one node and an
//! effect collector, never the world: everything a command does is either a
//! change to the node's configuration model, a read of its runtime state, or
//! an effect.

pub(crate) mod boot;
mod config;
mod config_if;
mod config_sub;
mod exec;
mod help;
pub(crate) mod runcfg;
pub(crate) mod show;

use std::net::Ipv4Addr;
use std::sync::OnceLock;

use crate::catalog::Role;
use crate::device::cli::grammar::{Args, Cmd, Grammar, Parsed};
use crate::device::cli::{LineId, Next, Reply, Tag};
use crate::device::config::{DeviceConfig, LineConfig, LoginMode};
use crate::ifname::IfId;
use crate::sim::fx::Fx;
use crate::sim::node::Node;
use crate::util::regex::Regex;

/// What a command handler works with.
pub(crate) struct Cx<'a> {
    pub(crate) n: &'a mut Node,
    pub(crate) fx: &'a mut Fx,
    pub(crate) sh: &'a mut IosShell,
    pub(crate) out: String,
    pub(crate) error: bool,
    pub(crate) touched: bool,
    pub(crate) next: Next,
    /// The command is not valid here after all: report it as invalid input.
    pub(crate) invalid: bool,
}

impl Cx<'_> {
    /// Reject the command the way an unknown command is rejected.
    pub(crate) fn invalid(&mut self) {
        self.invalid = true;
        self.error = true;
    }

    /// Print a line.
    pub(crate) fn say(&mut self, text: impl AsRef<str>) {
        self.out.push_str(text.as_ref());
        self.out.push('\n');
    }

    /// Print a rejection message and mark the command as failed.
    pub(crate) fn fail(&mut self, text: impl AsRef<str>) {
        self.say(text);
        self.error = true;
    }

    /// The configuration model, marking it as changed.
    pub(crate) fn cfg(&mut self) -> &mut DeviceConfig {
        self.touched = true;
        &mut self.n.cfg
    }

    pub(crate) fn role(&self) -> Role {
        self.n.model.role
    }
}

pub(crate) type Handler = for<'a, 'b> fn(&'b mut Cx<'a>, &Args);
pub(crate) type Table = Vec<Cmd<Handler>>;

/// Shorthand for command tables: a command without a `no` form.
pub(crate) fn c(pattern: &'static str, help: &'static str, run: Handler) -> Cmd<Handler> {
    Cmd {
        pattern,
        help,
        negatable: false,
        run,
    }
}

/// A command that can be negated with `no`.
pub(crate) fn n(pattern: &'static str, help: &'static str, run: Handler) -> Cmd<Handler> {
    Cmd {
        pattern,
        help,
        negatable: true,
        run,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineSel {
    Console,
    /// Inclusive range of vty line numbers.
    Vty(u8, u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    User,
    Priv,
    Config,
    /// One interface, or several with `interface range`.
    If(Vec<IfId>),
    Vlan(Vec<u16>),
    Line(LineSel),
    Ospf,
    Rip,
    DhcpPool(String),
    AclStd(String),
    AclExt(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GrammarKind {
    User,
    Priv,
    Config,
    If,
    Vlan,
    Line,
    Ospf,
    Rip,
    DhcpPool,
    AclStd,
    AclExt,
}

const KINDS: [GrammarKind; 11] = [
    GrammarKind::User,
    GrammarKind::Priv,
    GrammarKind::Config,
    GrammarKind::If,
    GrammarKind::Vlan,
    GrammarKind::Line,
    GrammarKind::Ospf,
    GrammarKind::Rip,
    GrammarKind::DhcpPool,
    GrammarKind::AclStd,
    GrammarKind::AclExt,
];
const ROLES: [Role; 3] = [Role::Router, Role::L2Switch, Role::L3Switch];

fn build(kind: GrammarKind, role: Role) -> Grammar<Handler> {
    let table = match kind {
        GrammarKind::User => exec::table(role, false),
        GrammarKind::Priv => exec::table(role, true),
        GrammarKind::Config => config::table(role),
        GrammarKind::If => config_if::table(role),
        GrammarKind::Vlan => config_sub::vlan_table(),
        GrammarKind::Line => config_sub::line_table(),
        GrammarKind::Ospf => config_sub::ospf_table(),
        GrammarKind::Rip => config_sub::rip_table(),
        GrammarKind::DhcpPool => config_sub::dhcp_table(),
        GrammarKind::AclStd => config_sub::acl_table(false),
        GrammarKind::AclExt => config_sub::acl_table(true),
    };
    Grammar::new(table, help::DICTIONARY)
}

/// The compiled grammar of a mode on a platform (built once, on first use).
fn grammar(kind: GrammarKind, role: Role) -> &'static Grammar<Handler> {
    static CACHE: OnceLock<Vec<Grammar<Handler>>> = OnceLock::new();
    let all = CACHE.get_or_init(|| {
        ROLES
            .iter()
            .flat_map(|role| KINDS.iter().map(|kind| build(*kind, *role)))
            .collect()
    });
    let role_index = ROLES.iter().position(|r| *r == role).unwrap_or(0);
    let kind_index = KINDS.iter().position(|k| *k == kind).unwrap_or(0);
    &all[role_index * KINDS.len() + kind_index]
}

/// Multi-line banner text being typed.
#[derive(Clone, Debug)]
struct BannerInput {
    login: bool,
    delimiter: String,
    text: String,
}

#[derive(Clone, Debug)]
pub(crate) struct IosShell {
    pub(crate) mode: Mode,
    pub(crate) line: LineId,
    pub(crate) peer: Option<Ipv4Addr>,
    user: String,
    pub(crate) term_len: u32,
    pub(crate) term_width: u32,
    pub(crate) monitor: bool,
    banner: Option<BannerInput>,
    /// Copy of the session's history for `show history`.
    pub(crate) history: Vec<String>,
    /// Still at the login prompt.
    logging_in: bool,
}

impl IosShell {
    pub(crate) fn new(line: LineId, peer: Option<Ipv4Addr>) -> IosShell {
        IosShell {
            mode: Mode::User,
            line,
            peer,
            user: String::new(),
            term_len: 24,
            term_width: 80,
            monitor: false,
            banner: None,
            history: Vec::new(),
            logging_in: false,
        }
    }

    /// A pre-authenticated shell for the automation helpers.
    pub(crate) fn automation(mode: Mode) -> IosShell {
        IosShell {
            mode,
            term_len: 0,
            ..IosShell::new(LineId::Console, None)
        }
    }

    pub(crate) fn user(&self) -> &str {
        &self.user
    }

    pub(crate) fn terminal_length(&self) -> u32 {
        self.term_len
    }

    pub(crate) fn in_config(&self) -> bool {
        !matches!(self.mode, Mode::User | Mode::Priv)
    }

    /// False while free text (a banner) is being typed.
    pub(crate) fn reads_commands(&self) -> bool {
        self.banner.is_none() && !self.logging_in
    }

    pub(crate) fn set_history(&mut self, history: &[String]) {
        self.history = history.to_vec();
    }

    fn line_config<'n>(&self, node: &'n Node) -> Option<&'n LineConfig> {
        match self.line {
            LineId::Console => Some(&node.cfg.console),
            LineId::Vty(n) => node.cfg.vty.get(usize::from(n)),
        }
    }

    pub(crate) fn shows_logs(&self, node: &Node) -> bool {
        node.cfg.logging.console && (self.line == LineId::Console || self.monitor)
    }

    pub(crate) fn logging_synchronous(&self, node: &Node) -> bool {
        self.line_config(node)
            .is_some_and(|l| l.logging_synchronous)
    }

    pub(crate) fn exec_timeout(&self, node: &Node) -> Option<u64> {
        self.line_config(node).and_then(LineConfig::timeout_micros)
    }

    pub(crate) fn prompt(&self, node: &Node) -> String {
        if self.banner.is_some() || self.logging_in {
            return String::new();
        }
        let host = node.hostname();
        let suffix = match &self.mode {
            Mode::User => return format!("{host}>"),
            Mode::Priv => return format!("{host}#"),
            Mode::Config => "config",
            Mode::If(ids) if ids.len() > 1 => "config-if-range",
            Mode::If(ids) if ids.first().is_some_and(|i| i.is_sub()) => "config-subif",
            Mode::If(_) => "config-if",
            Mode::Vlan(_) => "config-vlan",
            Mode::Line(_) => "config-line",
            Mode::Ospf | Mode::Rip => "config-router",
            Mode::DhcpPool(_) => "dhcp-config",
            Mode::AclStd(_) => "config-std-nacl",
            Mode::AclExt(_) => "config-ext-nacl",
        };
        format!("{host}({suffix})#")
    }

    fn grammar_kind(&self) -> GrammarKind {
        match &self.mode {
            Mode::User => GrammarKind::User,
            Mode::Priv => GrammarKind::Priv,
            Mode::Config => GrammarKind::Config,
            Mode::If(_) => GrammarKind::If,
            Mode::Vlan(_) => GrammarKind::Vlan,
            Mode::Line(_) => GrammarKind::Line,
            Mode::Ospf => GrammarKind::Ospf,
            Mode::Rip => GrammarKind::Rip,
            Mode::DhcpPool(_) => GrammarKind::DhcpPool,
            Mode::AclStd(_) => GrammarKind::AclStd,
            Mode::AclExt(_) => GrammarKind::AclExt,
        }
    }

    // ------------------------------------------------------------- login

    /// What a new session sees first: banner, then a login or the prompt.
    pub(crate) fn greet(&mut self, node: &mut Node, _fx: &mut Fx) -> Reply {
        let mut out = String::new();
        if let Some(banner) = &node.cfg.banner_motd {
            out.push_str(banner);
            out.push('\n');
        }
        let Some(line) = self.line_config(node).cloned() else {
            return Reply {
                out,
                next: Next::Close,
                ..Reply::default()
            };
        };
        if let Some(level) = line.privilege
            && level >= 15
        {
            self.mode = Mode::Priv;
        }
        let ask = |out: String, prompt: &str, hidden: bool, tag: Tag| Reply {
            out,
            next: Next::Ask {
                prompt: prompt.to_string(),
                hidden,
                tag,
            },
            ..Reply::default()
        };
        match line.login {
            LoginMode::None => Reply::text(out),
            LoginMode::Password if line.password.is_none() => {
                if self.line == LineId::Console {
                    // A console without a password cannot lock anyone out.
                    Reply::text(out)
                } else {
                    out.push_str("\nPassword required, but none set\n");
                    Reply {
                        out,
                        next: Next::Close,
                        ..Reply::default()
                    }
                }
            }
            LoginMode::Password => {
                self.logging_in = true;
                if let Some(banner) = &node.cfg.banner_login {
                    out.push_str(banner);
                    out.push('\n');
                }
                out.push_str("\nUser Access Verification\n\n");
                ask(out, "Password: ", true, Tag::LinePassword { tries: 0 })
            }
            LoginMode::Local => {
                self.logging_in = true;
                if let Some(banner) = &node.cfg.banner_login {
                    out.push_str(banner);
                    out.push('\n');
                }
                out.push_str("\nUser Access Verification\n\n");
                ask(out, "Username: ", false, Tag::Username { tries: 0 })
            }
        }
    }

    /// SSH sends the user name up front: skip straight to the password.
    pub(crate) fn ssh_login(&mut self, node: &mut Node, fx: &mut Fx, user: &str) -> Reply {
        let greeting = self.greet(node, fx);
        match greeting.next {
            Next::Ask {
                tag: Tag::Username { tries },
                ..
            } => Reply {
                out: String::new(),
                next: Next::Ask {
                    prompt: "Password: ".to_string(),
                    hidden: true,
                    tag: Tag::UserPassword {
                        user: user.to_string(),
                        tries,
                    },
                },
                ..Reply::default()
            },
            _ => greeting,
        }
    }

    /// A wrong password or unknown user. Three strikes drop a vty connection;
    /// the console simply starts over.
    fn login_failed(&mut self, tries: u8, username: bool) -> Reply {
        let last = tries >= 2;
        if last && self.line != LineId::Console {
            self.logging_in = false;
            return Reply {
                out: "% Bad passwords\n".to_string(),
                next: Next::Close,
                ..Reply::default()
            };
        }
        let out = match (last, username) {
            (true, _) => "% Bad passwords\n\n",
            (false, true) => "% Login invalid\n\n",
            (false, false) => "",
        };
        let tries = if last { 0 } else { tries + 1 };
        let next = if username {
            Next::Ask {
                prompt: "Username: ".to_string(),
                hidden: false,
                tag: Tag::Username { tries },
            }
        } else {
            Next::Ask {
                prompt: "Password: ".to_string(),
                hidden: true,
                tag: Tag::LinePassword { tries },
            }
        };
        Reply {
            out: out.to_string(),
            next,
            ..Reply::default()
        }
    }

    /// The user answered a question the shell asked.
    pub(crate) fn answer(&mut self, node: &mut Node, fx: &mut Fx, tag: Tag, text: &str) -> Reply {
        match tag {
            Tag::LinePassword { tries } => {
                let ok = self
                    .line_config(node)
                    .and_then(|l| l.password.as_ref())
                    .is_some_and(|p| p.verify(text));
                if ok {
                    self.logging_in = false;
                    Reply::default()
                } else {
                    self.login_failed(tries, false)
                }
            }
            Tag::Username { tries } => Reply {
                next: Next::Ask {
                    prompt: "Password: ".to_string(),
                    hidden: true,
                    tag: Tag::UserPassword {
                        user: text.trim().to_string(),
                        tries,
                    },
                },
                ..Reply::default()
            },
            Tag::UserPassword { user, tries } => {
                let account = node.cfg.user(&user).cloned();
                match account {
                    Some(account) if account.secret.as_ref().is_none_or(|s| s.verify(text)) => {
                        self.logging_in = false;
                        self.user = user;
                        if account.privilege >= 15 {
                            self.mode = Mode::Priv;
                        }
                        Reply::default()
                    }
                    _ => self.login_failed(tries, true),
                }
            }
            Tag::Enable { tries } => {
                let secret = node
                    .cfg
                    .enable_secret
                    .as_ref()
                    .or(node.cfg.enable_password.as_ref());
                if secret.is_some_and(|s| s.verify(text)) {
                    self.mode = Mode::Priv;
                    Reply::default()
                } else if tries >= 2 {
                    Reply {
                        out: "% Bad secrets\n\n".to_string(),
                        error: true,
                        ..Reply::default()
                    }
                } else {
                    Reply {
                        next: Next::Ask {
                            prompt: "Password: ".to_string(),
                            hidden: true,
                            tag: Tag::Enable { tries: tries + 1 },
                        },
                        ..Reply::default()
                    }
                }
            }
            other => exec::answer(self, node, fx, other, text),
        }
    }

    /// Ctrl-C while a question was pending.
    pub(crate) fn abort_question(&mut self, _node: &mut Node, _fx: &mut Fx, tag: Tag) -> Reply {
        match tag {
            Tag::LinePassword { .. } | Tag::Username { .. } | Tag::UserPassword { .. }
                if self.logging_in =>
            {
                if self.line == LineId::Console {
                    self.login_failed(
                        0,
                        matches!(tag, Tag::Username { .. } | Tag::UserPassword { .. }),
                    )
                } else {
                    Reply {
                        next: Next::Close,
                        ..Reply::default()
                    }
                }
            }
            _ => Reply::default(),
        }
    }

    /// Ctrl-C at the prompt: in configuration mode it leaves, like Ctrl-Z.
    pub(crate) fn interrupt(&mut self, node: &mut Node, fx: &mut Fx) -> Reply {
        if self.banner.take().is_some() {
            return Reply::default();
        }
        if self.in_config() {
            self.leave_config(node, fx)
        } else {
            Reply::default()
        }
    }

    /// Ctrl-Z: `end`.
    pub(crate) fn end_key(&mut self, node: &mut Node, fx: &mut Fx) -> Reply {
        self.banner = None;
        if self.in_config() {
            self.leave_config(node, fx)
        } else {
            Reply::default()
        }
    }

    /// Return to privileged EXEC and log the configuration event.
    pub(crate) fn leave_config(&mut self, node: &mut Node, fx: &mut Fx) -> Reply {
        self.mode = Mode::Priv;
        let message = match (self.line, self.peer, self.user.is_empty()) {
            (LineId::Vty(n), Some(peer), true) => {
                format!("Configured from console by vty{n} ({peer})")
            }
            (LineId::Vty(n), Some(peer), false) => format!(
                "Configured from console by {} on vty{n} ({peer})",
                self.user
            ),
            (_, _, true) => "Configured from console by console".to_string(),
            (_, _, false) => format!("Configured from console by {} on console", self.user),
        };
        node.log(fx, 5, "SYS", "CONFIG_I", message);
        Reply::default()
    }

    // ---------------------------------------------------------- dispatch

    /// Run one input line in the current mode.
    pub(crate) fn execute(&mut self, node: &mut Node, fx: &mut Fx, line: &str) -> Reply {
        if self.banner.is_some() {
            return self.banner_line(node, line);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('!') {
            return Reply::default();
        }
        let prompt_len = self.prompt(node).len();
        let indent = line.len() - line.trim_start().len();
        if self.in_config() {
            self.run_config(node, fx, trimmed, prompt_len + indent)
        } else {
            let privileged = self.mode == Mode::Priv;
            self.run_exec(node, fx, trimmed, prompt_len + indent, privileged)
        }
    }

    /// Run a handler. `shown` is the command as typed and `column` where it
    /// started on screen, for the caret of a handler-level rejection.
    fn run(
        &mut self,
        node: &mut Node,
        fx: &mut Fx,
        run: Handler,
        args: &Args,
        shown: &str,
        column: usize,
    ) -> Reply {
        let mut cx = Cx {
            n: node,
            fx,
            sh: self,
            out: String::new(),
            error: false,
            touched: false,
            next: Next::Prompt,
            invalid: false,
        };
        run(&mut cx, args);
        let (invalid, touched) = (cx.invalid, cx.touched);
        let reply = Reply {
            out: cx.out,
            error: cx.error,
            touched,
            next: cx.next,
        };
        if touched {
            node.last_config_change = Some(fx.now);
            node.unsaved = true;
        }
        if invalid {
            return Reply {
                touched,
                ..error_reply(Parsed::Invalid { at: 0 }, shown, column)
            };
        }
        reply
    }

    fn run_exec(
        &mut self,
        node: &mut Node,
        fx: &mut Fx,
        line: &str,
        column: usize,
        privileged: bool,
    ) -> Reply {
        let (command, modifier) = split_modifier(line);
        let kind = if privileged {
            GrammarKind::Priv
        } else {
            GrammarKind::User
        };
        match grammar(kind, node.model.role).parse(command, false) {
            Parsed::Ok { run, args } => {
                let filter = match modifier.map(OutputFilter::parse) {
                    Some(Ok(filter)) => Some(filter),
                    Some(Err(message)) => {
                        return Reply {
                            out: message,
                            error: true,
                            ..Reply::default()
                        };
                    }
                    None => None,
                };
                let mut reply = self.run(node, fx, run, &args, command, column);
                if let Some(filter) = filter {
                    reply.out = filter.apply(&reply.out);
                }
                reply
            }
            other => error_reply(other, line, column),
        }
    }

    fn run_config(&mut self, node: &mut Node, fx: &mut Fx, line: &str, column: usize) -> Reply {
        let first = line.split_whitespace().next().unwrap_or("");
        if first.eq_ignore_ascii_case("do") {
            let rest = line[first.len()..].trim_start();
            if rest.is_empty() {
                return Reply {
                    out: "% Incomplete command.\n\n".to_string(),
                    error: true,
                    ..Reply::default()
                };
            }
            let offset = line.len() - rest.len();
            let saved = self.mode.clone();
            let reply = self.run_exec(node, fx, rest, column + offset, true);
            // `do` never changes the configuration mode.
            if self.in_config() || matches!(saved, Mode::User | Mode::Priv) {
                self.mode = saved;
            }
            return reply;
        }
        let (negate, body, offset) = if first.eq_ignore_ascii_case("no") {
            let rest = line[first.len()..].trim_start();
            (true, rest, line.len() - rest.len())
        } else {
            (false, line, 0)
        };
        if body.is_empty() {
            return Reply {
                out: "% Incomplete command.\n\n".to_string(),
                error: true,
                ..Reply::default()
            };
        }
        let role = node.model.role;
        let local = grammar(self.grammar_kind(), role).parse(body, negate);
        if let Parsed::Ok { run, args } = local {
            return self.run(node, fx, run, &args, body, column + offset);
        }
        if self.mode != Mode::Config {
            // Commands of the parent mode are accepted from a sub-mode and leave it.
            if let Parsed::Ok { run, args } = grammar(GrammarKind::Config, role).parse(body, negate)
            {
                self.mode = Mode::Config;
                return self.run(node, fx, run, &args, body, column + offset);
            }
            // Report whichever grammar understood more of the line.
            let global = grammar(GrammarKind::Config, role).parse(body, negate);
            if progress(&global) > progress(&local) {
                return error_reply(global, body, column + offset);
            }
        }
        error_reply(local, body, column + offset)
    }

    // ------------------------------------------------------------ banners

    /// Start collecting a banner. `rest` is the text after the banner keyword:
    /// a delimiter character followed by text, possibly ending on the same line.
    pub(crate) fn begin_banner(&mut self, node: &mut Node, login: bool, rest: &str) -> Reply {
        // A running configuration writes the delimiter as the two characters "^C".
        let delimiter = if rest.starts_with("^C") {
            "^C".to_string()
        } else {
            rest.chars().take(1).collect()
        };
        let after = &rest[delimiter.len()..];
        let mut input = BannerInput {
            login,
            delimiter,
            text: String::new(),
        };
        if let Some(end) = after.find(&input.delimiter) {
            input.text = after[..end].to_string();
            store_banner(node, &input);
            return Reply {
                touched: true,
                ..Reply::default()
            };
        }
        input.text = after.to_string();
        let out = format!(
            "Enter TEXT message.  End with the character '{}'.\n",
            input.delimiter
        );
        self.banner = Some(input);
        Reply::text(out)
    }

    fn banner_line(&mut self, node: &mut Node, line: &str) -> Reply {
        let Some(mut input) = self.banner.take() else {
            return Reply::default();
        };
        match line.find(&input.delimiter) {
            Some(end) => {
                if !input.text.is_empty() || end > 0 {
                    input.text.push('\n');
                }
                input.text.push_str(&line[..end]);
                store_banner(node, &input);
                Reply {
                    touched: true,
                    ..Reply::default()
                }
            }
            None => {
                input.text.push('\n');
                input.text.push_str(line);
                if input.text.len() < 4096 {
                    self.banner = Some(input);
                }
                Reply::default()
            }
        }
    }

    // --------------------------------------------------------------- help

    /// The `?` listing for a partial line.
    pub(crate) fn help(&self, node: &Node, line: &str) -> String {
        let role = node.model.role;
        let trimmed = line.trim_start();
        let first = trimmed.split_whitespace().next().unwrap_or("");
        let after_first = trimmed.len() > first.len();
        let (kind, body, negate) =
            if self.in_config() && first.eq_ignore_ascii_case("do") && after_first {
                (
                    GrammarKind::Priv,
                    trimmed[first.len()..].trim_start(),
                    false,
                )
            } else if self.in_config() && first.eq_ignore_ascii_case("no") && after_first {
                (
                    self.grammar_kind(),
                    trimmed[first.len()..].trim_start(),
                    true,
                )
            } else {
                (self.grammar_kind(), trimmed, false)
            };
        let Some((mut entries, names_only)) = grammar(kind, role).help(body, negate) else {
            return "% Unrecognized command\n".to_string();
        };
        if self.in_config() && body.is_empty() && !negate {
            entries.push((
                "do".to_string(),
                "To run exec commands in config mode".to_string(),
            ));
            entries.push((
                "no".to_string(),
                "Negate a command or set its defaults".to_string(),
            ));
            entries.sort();
        }
        if entries.is_empty() {
            return "% Unrecognized command\n".to_string();
        }
        if names_only {
            return format!(
                "{}\n\n",
                entries
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join("  ")
            );
        }
        let width = entries
            .iter()
            .map(|(name, _)| name.len())
            .max()
            .unwrap_or(0);
        let mut out = String::new();
        for (name, description) in entries {
            out.push_str(format!("  {name:<width$}  {description}").trim_end());
            out.push('\n');
        }
        out.push('\n');
        out
    }

    /// Tab completion of the last word.
    pub(crate) fn complete(&self, line: &str) -> Option<String> {
        // Completion does not depend on the platform beyond which commands exist;
        // the router grammar is a superset for the common commands.
        let trimmed = line.trim_start();
        let first = trimmed.split_whitespace().next().unwrap_or("");
        let after_first = trimmed.len() > first.len();
        let (kind, body, negate) =
            if self.in_config() && first.eq_ignore_ascii_case("do") && after_first {
                (
                    GrammarKind::Priv,
                    trimmed[first.len()..].trim_start(),
                    false,
                )
            } else if self.in_config() && first.eq_ignore_ascii_case("no") && after_first {
                (
                    self.grammar_kind(),
                    trimmed[first.len()..].trim_start(),
                    true,
                )
            } else {
                (self.grammar_kind(), trimmed, false)
            };
        ROLES
            .iter()
            .find_map(|role| grammar(kind, *role).complete(body, negate))
    }
}

/// Run one access-list entry (`permit ...`, `deny ...`, `remark ...`) against
/// the list `name`, as typed after a numbered `access-list N`.
pub(crate) fn run_acl_entry(cx: &mut Cx, extended: bool, name: &str, entry: &str, negate: bool) {
    let kind = if extended {
        GrammarKind::AclExt
    } else {
        GrammarKind::AclStd
    };
    let saved = std::mem::replace(
        &mut cx.sh.mode,
        if extended {
            Mode::AclExt(name.to_string())
        } else {
            Mode::AclStd(name.to_string())
        },
    );
    match grammar(kind, cx.n.model.role).parse(entry, negate) {
        Parsed::Ok { run, args } => run(cx, &args),
        Parsed::Incomplete => cx.fail("% Incomplete command.\n"),
        _ => cx.invalid(),
    }
    cx.sh.mode = saved;
}

fn store_banner(node: &mut Node, input: &BannerInput) {
    let text = Some(input.text.clone());
    if input.login {
        node.cfg.banner_login = text;
    } else {
        node.cfg.banner_motd = text;
    }
}

/// How far into the line a failed parse got (for choosing which error to show).
fn progress(parsed: &Parsed<Handler>) -> usize {
    match parsed {
        Parsed::Invalid { at } => *at,
        Parsed::Incomplete | Parsed::Ambiguous => usize::MAX - 1,
        Parsed::Ok { .. } => usize::MAX,
        Parsed::Empty => 0,
    }
}

/// The classic error texts. `column` is where the line started on screen.
fn error_reply(parsed: Parsed<Handler>, line: &str, column: usize) -> Reply {
    let out = match parsed {
        Parsed::Ambiguous => format!("% Ambiguous command:  \"{line}\"\n"),
        Parsed::Incomplete => "% Incomplete command.\n\n".to_string(),
        Parsed::Invalid { at } => {
            format!(
                "{}^\n% Invalid input detected at '^' marker.\n\n",
                " ".repeat(column + at)
            )
        }
        Parsed::Empty | Parsed::Ok { .. } => String::new(),
    };
    Reply {
        out,
        error: true,
        ..Reply::default()
    }
}

/// Split `show ... | include x` into the command and the modifier text.
fn split_modifier(line: &str) -> (&str, Option<&str>) {
    match line.split_once('|') {
        Some((command, modifier)) if !command.trim().is_empty() => {
            (command.trim_end(), Some(modifier.trim()))
        }
        _ => (line, None),
    }
}

enum OutputFilter {
    Include(Regex),
    Exclude(Regex),
    Begin(Regex),
    Section(Regex),
    Count(Regex),
}

impl OutputFilter {
    fn parse(text: &str) -> Result<OutputFilter, String> {
        let (word, pattern) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
        let pattern = pattern.trim();
        let invalid = || "% Invalid input detected at '^' marker.\n\n".to_string();
        if word.is_empty() || pattern.is_empty() {
            return Err("% Incomplete command.\n\n".to_string());
        }
        let regex =
            Regex::new(pattern).ok_or_else(|| "% Invalid regular expression\n".to_string())?;
        let is = |full: &str| crate::util::text::is_abbrev(word, full);
        Ok(if is("include") {
            OutputFilter::Include(regex)
        } else if is("exclude") {
            OutputFilter::Exclude(regex)
        } else if is("begin") {
            OutputFilter::Begin(regex)
        } else if is("section") {
            OutputFilter::Section(regex)
        } else if is("count") {
            OutputFilter::Count(regex)
        } else {
            return Err(invalid());
        })
    }

    fn apply(&self, text: &str) -> String {
        let lines = text.lines();
        let kept: Vec<&str> = match self {
            OutputFilter::Include(re) => lines.filter(|l| re.is_match(l)).collect(),
            OutputFilter::Exclude(re) => lines.filter(|l| !re.is_match(l)).collect(),
            OutputFilter::Begin(re) => lines.skip_while(|l| !re.is_match(l)).collect(),
            OutputFilter::Section(re) => {
                // A matching line plus the indented lines that belong to it.
                let mut out = Vec::new();
                let mut inside = false;
                for line in lines {
                    if re.is_match(line) {
                        inside = true;
                        out.push(line);
                    } else if inside && line.starts_with(' ') {
                        out.push(line);
                    } else {
                        inside = false;
                    }
                }
                out
            }
            OutputFilter::Count(re) => {
                return format!(
                    "Number of lines which match regexp = {}\n",
                    lines.filter(|l| re.is_match(l)).count()
                );
            }
        };
        let mut out = kept.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_grammar_compiles() {
        for role in ROLES {
            for kind in KINDS {
                // Building panics on a malformed pattern.
                let _ = grammar(kind, role);
            }
        }
    }

    #[test]
    fn output_filters() {
        let text = "interface Vlan1\n ip address 10.0.0.1 255.0.0.0\n!\nrouter ospf 1\n network 10.0.0.0 0.0.0.255 area 0\n!\n";
        let apply = |m: &str| {
            OutputFilter::parse(m)
                .map(|f| f.apply(text))
                .unwrap_or_else(|e| e)
        };
        assert_eq!(apply("include ospf"), "router ospf 1\n");
        assert_eq!(apply("i ^router"), "router ospf 1\n");
        assert_eq!(apply("exclude 10|!"), "interface Vlan1\nrouter ospf 1\n");
        assert_eq!(
            apply("begin router"),
            "router ospf 1\n network 10.0.0.0 0.0.0.255 area 0\n!\n"
        );
        assert_eq!(
            apply("section ospf"),
            "router ospf 1\n network 10.0.0.0 0.0.0.255 area 0\n"
        );
        assert_eq!(
            apply("count 10\\.0"),
            "Number of lines which match regexp = 2\n"
        );
        assert!(apply("include").starts_with("% Incomplete"));
        assert!(apply("frobnicate x").starts_with("% Invalid input"));
        assert!(apply("include (x").starts_with("% Invalid regular"));
        assert_eq!(
            split_modifier("show run | include foo"),
            ("show run", Some("include foo"))
        );
        assert_eq!(split_modifier("show run"), ("show run", None));
    }
}
