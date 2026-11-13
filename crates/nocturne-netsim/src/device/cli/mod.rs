//! Consoles: the raw byte stream between the host and a device's command line.
//!
//! A [`Console`] owns a stack of [`Session`]s (a telnet from one device to
//! another nests). The session implements the line discipline — echo, editing,
//! history, paging, hidden password entry — and delegates meaning to a shell:
//! the IOS-style personality or the end-host command prompt.

pub(crate) mod grammar;

use std::collections::VecDeque;
use std::net::Ipv4Addr;

use crate::api::{CliKind, LabError, NodeId, SimTime};
use crate::device::host::HostShell;
use crate::device::ios::IosShell;
use crate::sim::World;
use crate::sim::fx::Fx;
use crate::sim::node::Node;
use crate::sim::procs::Outcome;

/// Pending output is capped so an undrained console cannot grow without bound.
const MAX_PENDING_OUTPUT: usize = 2 << 20;
const HISTORY_SIZE: usize = 20;
const MAX_LINE: usize = 1024;

/// Why a shell asked the user a question; handed back with the answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Tag {
    LinePassword { tries: u8 },
    Username { tries: u8 },
    UserPassword { user: String, tries: u8 },
    Enable { tries: u8 },
    ReloadSave,
    ReloadConfirm,
    EraseConfirm,
    CopyDestination,
    ClearCounters { iface: Option<String> },
    ClearOspf,
    DeleteName { file: String },
    DeleteConfirm { file: String },
}

/// What the session should do after a shell handled some input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Next {
    /// Show the prompt and read the next line.
    #[default]
    Prompt,
    /// Follow a process until it finishes.
    Job(u32),
    /// Follow a connection attempt; on success open a nested session.
    Connect { proc: u32, ssh_user: Option<String> },
    /// Ask for a line of text (hidden for passwords).
    Ask {
        prompt: String,
        hidden: bool,
        tag: Tag,
    },
    /// Wait for a single keypress.
    Key { prompt: String, tag: Tag },
    /// The session is over (logout).
    Close,
    /// Restart the device.
    Reload,
}

/// A shell's response to input.
#[derive(Clone, Debug, Default)]
pub(crate) struct Reply {
    /// Text to print (`\n` line ends).
    pub(crate) out: String,
    /// The input was rejected.
    pub(crate) error: bool,
    /// The configuration changed: derived state must be recomputed.
    pub(crate) touched: bool,
    pub(crate) next: Next,
}

impl Reply {
    pub(crate) fn text(out: impl Into<String>) -> Reply {
        Reply {
            out: out.into(),
            ..Reply::default()
        }
    }
}

/// Which line of the device a session came in on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineId {
    Console,
    Vty(u8),
}

/// A session as `show users` lists it.
#[derive(Clone, Debug)]
pub(crate) struct SessionInfo {
    pub(crate) key: u64,
    pub(crate) line: LineId,
    pub(crate) user: String,
    pub(crate) peer: Option<Ipv4Addr>,
    pub(crate) last_input: SimTime,
}

#[derive(Clone, Debug)]
pub(crate) enum Shell {
    Ios(IosShell),
    Host(HostShell),
}

impl Shell {
    fn prompt(&self, node: &Node) -> String {
        match self {
            Shell::Ios(shell) => shell.prompt(node),
            Shell::Host(shell) => shell.prompt(),
        }
    }

    pub(crate) fn execute(&mut self, node: &mut Node, fx: &mut Fx, line: &str) -> Reply {
        match self {
            Shell::Ios(shell) => shell.execute(node, fx, line),
            Shell::Host(shell) => shell.execute(node, fx, line),
        }
    }

    pub(crate) fn answer(&mut self, node: &mut Node, fx: &mut Fx, tag: Tag, text: &str) -> Reply {
        match self {
            Shell::Ios(shell) => shell.answer(node, fx, tag, text),
            Shell::Host(_) => Reply::default(),
        }
    }

    /// Editing keys with meaning to the shell (`?`, Tab) are only active while
    /// it reads commands, not while it collects free text.
    fn interactive_help(&self) -> bool {
        match self {
            Shell::Ios(shell) => shell.reads_commands(),
            Shell::Host(_) => false,
        }
    }

    fn history_bucket(&self) -> usize {
        match self {
            Shell::Ios(shell) => usize::from(shell.in_config()),
            Shell::Host(_) => 0,
        }
    }
}

#[derive(Clone, Debug)]
enum Stage {
    Line,
    Ask {
        tag: Tag,
        hidden: bool,
        buffer: String,
    },
    Key {
        tag: Tag,
    },
    More {
        lines: VecDeque<String>,
    },
    Job {
        proc: u32,
        shown: usize,
        connect: Option<Option<String>>,
    },
    /// The device is powered off.
    Dead,
}

#[derive(Clone, Debug)]
pub(crate) struct Session {
    pub(crate) node: NodeId,
    epoch: u32,
    pub(crate) shell: Shell,
    line: String,
    cursor: usize,
    stage: Stage,
    history: [Vec<String>; 2],
    history_pos: Option<usize>,
    log_cursor: u64,
    last_input: SimTime,
    /// Key under which the session is registered on its node.
    key: u64,
    /// The address a remote session was opened to (`None` on the console).
    reached: Option<Ipv4Addr>,
}

/// What happened to the session stack while handling input.
enum Turn {
    Stay,
    Close,
    Reload,
    Push(Box<Session>, String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Esc {
    None,
    Escape,
    Bracket,
}

#[derive(Clone, Debug)]
pub(crate) struct Console {
    stack: Vec<Session>,
    out: Vec<u8>,
    esc: Esc,
    last_was_cr: bool,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
}

fn emit(out: &mut Vec<u8>, text: &str) {
    if out.len() > MAX_PENDING_OUTPUT {
        return;
    }
    let mut previous = b' ';
    for byte in text.bytes() {
        if byte == b'\n' && previous != b'\r' {
            out.push(b'\r');
        }
        out.push(byte);
        previous = byte;
    }
}

impl Session {
    /// Open a session on `node`; returns the session and its greeting text.
    /// How the far end of a remote session is named when the connection ends.
    fn reached_name(&self) -> String {
        self.reached
            .map(|address| address.to_string())
            .unwrap_or_default()
    }

    fn open(
        world: &mut World,
        node_id: NodeId,
        line: Option<(LineId, Ipv4Addr)>,
        key: u64,
    ) -> Result<(Session, Reply), LabError> {
        let node = world
            .nodes
            .get(&node_id)
            .ok_or(LabError::UnknownNode(node_id))?;
        let (epoch, log_cursor) = (node.epoch, node.log.next_seq);
        if !node.powered {
            return Err(LabError::PoweredOff(node.name.clone()));
        }
        let (line_id, peer) = match line {
            Some((line_id, peer)) => (line_id, Some(peer)),
            None => (LineId::Console, None),
        };
        let mut shell = match node.model.cli {
            CliKind::Ios => Shell::Ios(IosShell::new(line_id, peer)),
            CliKind::Host => Shell::Host(HostShell::new()),
            CliKind::None => return Err(LabError::NoCli(node.name.clone())),
        };
        let now = world.now;
        let greeting = world
            .with_node(node_id, |node, fx| {
                node.sessions.push(SessionInfo {
                    key,
                    line: line_id,
                    user: String::new(),
                    peer,
                    last_input: now,
                });
                match &mut shell {
                    Shell::Ios(ios) => ios.greet(node, fx),
                    Shell::Host(_) => Reply::default(),
                }
            })
            .unwrap_or_default();
        let session = Session {
            node: node_id,
            epoch,
            shell,
            line: String::new(),
            cursor: 0,
            stage: Stage::Line,
            history: [Vec::new(), Vec::new()],
            history_pos: None,
            log_cursor,
            last_input: now,
            key,
            reached: None,
        };
        Ok((session, greeting))
    }

    fn prompt(&self, world: &World) -> String {
        world
            .nodes
            .get(&self.node)
            .map(|n| self.shell.prompt(n))
            .unwrap_or_default()
    }

    fn close(&self, world: &mut World) {
        if let Some(node) = world.nodes.get_mut(&self.node) {
            node.sessions.retain(|s| s.key != self.key);
        }
    }

    /// Apply a shell reply: print its text and move to the stage it asks for.
    fn apply(&mut self, world: &mut World, out: &mut Vec<u8>, reply: Reply, paged: bool) -> Turn {
        if reply.touched {
            world.mark_dirty(self.node);
            world.touch();
            world.reconcile();
        }
        let page = if paged { self.page_len() } else { 0 };
        let lines: Vec<&str> = reply.out.split_inclusive('\n').collect();
        if page > 0 && lines.len() > page && reply.next == Next::Prompt {
            for line in &lines[..page] {
                emit(out, line);
            }
            emit(out, " --More-- ");
            self.stage = Stage::More {
                lines: lines[page..].iter().map(|l| (*l).to_string()).collect(),
            };
            return Turn::Stay;
        }
        emit(out, &reply.out);
        match reply.next {
            Next::Prompt => {
                self.stage = Stage::Line;
                self.show_prompt(world, out);
            }
            Next::Job(proc) => {
                self.stage = Stage::Job {
                    proc,
                    shown: 0,
                    connect: None,
                }
            }
            Next::Connect { proc, ssh_user } => {
                self.stage = Stage::Job {
                    proc,
                    shown: 0,
                    connect: Some(ssh_user),
                }
            }
            Next::Ask {
                prompt,
                hidden,
                tag,
            } => {
                emit(out, &prompt);
                self.stage = Stage::Ask {
                    tag,
                    hidden,
                    buffer: String::new(),
                };
            }
            Next::Key { prompt, tag } => {
                emit(out, &prompt);
                self.stage = Stage::Key { tag };
            }
            Next::Close => return Turn::Close,
            Next::Reload => return Turn::Reload,
        }
        Turn::Stay
    }

    fn page_len(&self) -> usize {
        match &self.shell {
            Shell::Ios(shell) => (shell.terminal_length() as usize).saturating_sub(1),
            Shell::Host(_) => 0,
        }
    }

    fn show_prompt(&mut self, world: &World, out: &mut Vec<u8>) {
        self.line.clear();
        self.cursor = 0;
        self.history_pos = None;
        emit(out, &self.prompt(world));
    }

    /// Redraw prompt and the line being edited on a fresh row.
    fn redraw(&self, world: &World, out: &mut Vec<u8>) {
        emit(out, &self.prompt(world));
        emit(out, &self.line);
        emit(out, &"\x08".repeat(self.line.len() - self.cursor));
    }

    fn run_shell(
        &mut self,
        world: &mut World,
        f: impl FnOnce(&mut Shell, &mut Node, &mut Fx) -> Reply,
    ) -> Reply {
        let shell = &mut self.shell;
        world
            .with_node(self.node, |node, fx| f(shell, node, fx))
            .unwrap_or_default()
    }

    // ------------------------------------------------------------ line editing

    fn replace_line(&mut self, out: &mut Vec<u8>, text: &str) {
        emit(out, &"\x08".repeat(self.cursor));
        emit(out, text);
        let extra = self.line.len().saturating_sub(text.len());
        emit(out, &" ".repeat(extra));
        emit(out, &"\x08".repeat(extra));
        self.line = text.to_string();
        self.cursor = self.line.len();
    }

    fn insert(&mut self, out: &mut Vec<u8>, c: char) {
        if self.line.len() >= MAX_LINE {
            return;
        }
        self.line.insert(self.cursor, c);
        self.cursor += 1;
        let tail = &self.line[self.cursor - 1..];
        emit(out, tail);
        emit(out, &"\x08".repeat(tail.len() - 1));
    }

    fn delete_before(&mut self, out: &mut Vec<u8>, count: usize) {
        let count = count.min(self.cursor);
        if count == 0 {
            return;
        }
        self.line
            .replace_range(self.cursor - count..self.cursor, "");
        self.cursor -= count;
        let tail = &self.line[self.cursor..];
        emit(out, &"\x08".repeat(count));
        emit(out, tail);
        emit(out, &" ".repeat(count));
        emit(out, &"\x08".repeat(tail.len() + count));
    }

    fn history_step(&mut self, out: &mut Vec<u8>, back: bool) {
        let bucket = self.shell.history_bucket();
        let entries = &self.history[bucket];
        let pos = match (self.history_pos, back) {
            (None, true) if !entries.is_empty() => Some(entries.len() - 1),
            (Some(p), true) => Some(p.saturating_sub(1)),
            (Some(p), false) if p + 1 < entries.len() => Some(p + 1),
            (Some(_), false) => None,
            (None, _) => return,
        };
        let text = pos
            .and_then(|p| entries.get(p))
            .cloned()
            .unwrap_or_default();
        self.history_pos = pos;
        self.replace_line(out, &text);
    }

    fn remember(&mut self, line: &str) {
        let bucket = self.shell.history_bucket();
        let entries = &mut self.history[bucket];
        if entries.last().is_none_or(|last| last != line) {
            entries.push(line.to_string());
            if entries.len() > HISTORY_SIZE {
                entries.remove(0);
            }
        }
    }

    // -------------------------------------------------------------- input

    fn key(&mut self, world: &mut World, out: &mut Vec<u8>, key: Key) -> Turn {
        self.last_input = world.now;
        match std::mem::replace(&mut self.stage, Stage::Line) {
            Stage::Line => self.key_line(world, out, key),
            Stage::Ask {
                tag,
                hidden,
                mut buffer,
            } => match key {
                Key::Enter => {
                    emit(out, "\n");
                    let reply = self.run_shell(world, |shell, node, fx| {
                        shell.answer(node, fx, tag, &buffer)
                    });
                    self.apply(world, out, reply, false)
                }
                Key::Char(c) => {
                    if buffer.len() < MAX_LINE {
                        buffer.push(c);
                        if !hidden {
                            emit(out, c.encode_utf8(&mut [0; 4]));
                        }
                    }
                    self.stage = Stage::Ask {
                        tag,
                        hidden,
                        buffer,
                    };
                    Turn::Stay
                }
                Key::Backspace => {
                    if buffer.pop().is_some() && !hidden {
                        emit(out, "\x08 \x08");
                    }
                    self.stage = Stage::Ask {
                        tag,
                        hidden,
                        buffer,
                    };
                    Turn::Stay
                }
                Key::Interrupt | Key::End => {
                    emit(out, "\n");
                    let reply = self.run_shell(world, |shell, node, fx| match shell {
                        Shell::Ios(ios) => ios.abort_question(node, fx, tag),
                        Shell::Host(_) => Reply::default(),
                    });
                    self.apply(world, out, reply, false)
                }
                _ => {
                    self.stage = Stage::Ask {
                        tag,
                        hidden,
                        buffer,
                    };
                    Turn::Stay
                }
            },
            Stage::Key { tag } => {
                let text = match key {
                    Key::Enter => "\n".to_string(),
                    Key::Char(c) => c.to_string(),
                    Key::Interrupt | Key::End => "\x03".to_string(),
                    _ => {
                        self.stage = Stage::Key { tag };
                        return Turn::Stay;
                    }
                };
                emit(out, "\n");
                let reply =
                    self.run_shell(world, |shell, node, fx| shell.answer(node, fx, tag, &text));
                self.apply(world, out, reply, false)
            }
            Stage::More { mut lines } => {
                // Erase the "--More--" marker the way a real terminal server does.
                emit(
                    out,
                    "\x08\x08\x08\x08\x08\x08\x08\x08\x08        \x08\x08\x08\x08\x08\x08\x08\x08\x08",
                );
                let count = match key {
                    Key::Char(' ') => self.page_len().max(1),
                    Key::Enter => 1,
                    _ => 0,
                };
                for _ in 0..count {
                    match lines.pop_front() {
                        Some(line) => emit(out, &line),
                        None => break,
                    }
                }
                if count == 0 || lines.is_empty() {
                    self.stage = Stage::Line;
                    self.show_prompt(world, out);
                } else {
                    emit(out, " --More-- ");
                    self.stage = Stage::More { lines };
                }
                Turn::Stay
            }
            Stage::Job {
                proc,
                shown,
                connect,
            } => {
                if matches!(key, Key::Interrupt | Key::Abort)
                    && let Some(node) = world.nodes.get_mut(&self.node)
                {
                    node.proc_abort(proc);
                }
                self.stage = Stage::Job {
                    proc,
                    shown,
                    connect,
                };
                Turn::Stay
            }
            Stage::Dead => {
                self.stage = Stage::Dead;
                Turn::Stay
            }
        }
    }

    fn key_line(&mut self, world: &mut World, out: &mut Vec<u8>, key: Key) -> Turn {
        match key {
            Key::Char('?') if self.shell.interactive_help() => {
                emit(out, "?\n");
                let line = self.line[..self.cursor].to_string();
                if let (Shell::Ios(ios), Some(node)) = (&self.shell, world.nodes.get(&self.node)) {
                    emit(out, &ios.help(node, &line));
                }
                self.redraw(world, out);
            }
            Key::Char(c) => self.insert(out, c),
            Key::Tab => {
                if !self.shell.interactive_help() {
                    return Turn::Stay;
                }
                let head = self.line[..self.cursor].to_string();
                let completion = match &self.shell {
                    Shell::Ios(ios) if self.cursor == self.line.len() => ios.complete(&head),
                    _ => None,
                };
                match completion {
                    Some(word) => {
                        let typed = head.rsplit(' ').next().unwrap_or("").len();
                        for c in word.chars().skip(typed).chain(std::iter::once(' ')) {
                            self.insert(out, c);
                        }
                    }
                    None => {
                        emit(out, "\n");
                        self.redraw(world, out);
                    }
                }
            }
            Key::Backspace => self.delete_before(out, 1),
            Key::Home => {
                emit(out, &"\x08".repeat(self.cursor));
                self.cursor = 0;
            }
            Key::LineEnd => {
                emit(out, &self.line[self.cursor..]);
                self.cursor = self.line.len();
            }
            Key::KillLine => self.delete_before(out, self.cursor),
            Key::KillWord => {
                let head = &self.line[..self.cursor];
                let keep = head.trim_end().rfind(' ').map_or(0, |i| i + 1);
                self.delete_before(out, self.cursor - keep);
            }
            Key::Left => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    emit(out, "\x08");
                }
            }
            Key::Right => {
                if self.cursor < self.line.len() {
                    emit(out, &self.line[self.cursor..self.cursor + 1]);
                    self.cursor += 1;
                }
            }
            Key::Up => self.history_step(out, true),
            Key::Down => self.history_step(out, false),
            Key::Redraw => {
                emit(out, "\n");
                self.redraw(world, out);
            }
            Key::Interrupt | Key::Abort => {
                emit(out, "\n");
                let reply = self.run_shell(world, |shell, node, fx| match shell {
                    Shell::Ios(ios) => ios.interrupt(node, fx),
                    Shell::Host(_) => Reply::default(),
                });
                return self.apply(world, out, reply, false);
            }
            Key::End => {
                let in_config = matches!(&self.shell, Shell::Ios(ios) if ios.in_config());
                emit(out, if in_config { "^Z\n" } else { "\n" });
                let reply = self.run_shell(world, |shell, node, fx| match shell {
                    Shell::Ios(ios) => ios.end_key(node, fx),
                    Shell::Host(_) => Reply::default(),
                });
                return self.apply(world, out, reply, false);
            }
            Key::Enter => {
                emit(out, "\n");
                let line = std::mem::take(&mut self.line);
                self.cursor = 0;
                if !line.trim().is_empty() && self.shell.interactive_help() {
                    self.remember(line.trim());
                }
                if let Shell::Ios(ios) = &mut self.shell {
                    ios.set_history(&self.history[usize::from(ios.in_config())]);
                }
                let reply = self.run_shell(world, |shell, node, fx| shell.execute(node, fx, &line));
                return self.apply(world, out, reply, true);
            }
        }
        Turn::Stay
    }

    /// Print what happened since the last call: process output and log messages.
    fn pump(&mut self, world: &mut World, out: &mut Vec<u8>) -> Turn {
        let Some(node) = world.nodes.get_mut(&self.node) else {
            return Turn::Close;
        };
        if !node.powered {
            self.stage = Stage::Dead;
            self.epoch = node.epoch;
            return Turn::Stay;
        }
        if matches!(self.stage, Stage::Dead) || node.epoch != self.epoch {
            // The device was restarted under us.
            return Turn::Reload;
        }
        if let Some(info) = node.sessions.iter_mut().find(|s| s.key == self.key) {
            info.last_input = self.last_input;
            if let Shell::Ios(ios) = &self.shell {
                info.user = ios.user().to_string();
            }
        }
        let mut result = Turn::Stay;
        if let Stage::Job {
            proc,
            shown,
            connect,
        } = &mut self.stage
        {
            let (proc_id, connect) = (*proc, connect.clone());
            match node.procs.get(proc_id) {
                Some(p) => {
                    emit(out, &p.out[(*shown).min(p.out.len())..]);
                    *shown = p.out.len();
                    if p.done {
                        let (outcome, address) = (p.outcome, p.address);
                        node.procs.remove(proc_id);
                        self.stage = Stage::Line;
                        result = self.job_done(world, out, outcome, address, connect);
                    }
                }
                None => {
                    self.stage = Stage::Line;
                    self.show_prompt(world, out);
                }
            }
        }
        self.pump_logs(world, out);
        if matches!(result, Turn::Stay) && self.timed_out(world) {
            return Turn::Close;
        }
        result
    }

    fn job_done(
        &mut self,
        world: &mut World,
        out: &mut Vec<u8>,
        outcome: Outcome,
        address: Option<Ipv4Addr>,
        connect: Option<Option<String>>,
    ) -> Turn {
        let (Some(ssh_user), Outcome::Ok { .. }, Some(address)) = (connect, outcome, address)
        else {
            self.show_prompt(world, out);
            return Turn::Stay;
        };
        // The connection opened: start a session on the far device's vty.
        let source = world
            .nodes
            .get(&self.node)
            .and_then(|n| n.source_for(address))
            .unwrap_or(Ipv4Addr::UNSPECIFIED);
        let target = world.node_by_ip(address);
        let line = target.and_then(|t| world.nodes.get(&t)).map(|n| {
            (0..n.cfg.vty.len() as u8)
                .find(|i| !n.sessions.iter().any(|s| s.line == LineId::Vty(*i)))
        });
        let opened = match (target, line) {
            (Some(target), Some(Some(vty))) => {
                world.session_keys += 1;
                Session::open(
                    world,
                    target,
                    Some((LineId::Vty(vty), source)),
                    world.session_keys,
                )
                .ok()
            }
            (Some(target), None) => {
                world.session_keys += 1;
                Session::open(
                    world,
                    target,
                    Some((LineId::Vty(0), source)),
                    world.session_keys,
                )
                .ok()
            }
            _ => None,
        };
        match opened {
            Some((mut session, mut greeting)) => {
                session.reached = Some(address);
                if let (Shell::Ios(ios), Some(user)) = (&mut session.shell, ssh_user) {
                    // SSH carries the username; only the password is asked for.
                    let reply =
                        world.with_node(session.node, |node, fx| ios.ssh_login(node, fx, &user));
                    greeting = reply.unwrap_or(greeting);
                }
                let mut text = Vec::new();
                match session.apply(world, &mut text, greeting, false) {
                    Turn::Stay => Turn::Push(
                        Box::new(session),
                        String::from_utf8_lossy(&text).into_owned(),
                    ),
                    _ => {
                        session.close(world);
                        emit(out, &String::from_utf8_lossy(&text));
                        emit(
                            out,
                            &format!("\n[Connection to {address} closed by foreign host]\n"),
                        );
                        self.show_prompt(world, out);
                        Turn::Stay
                    }
                }
            }
            None => {
                emit(
                    out,
                    &format!("\n[Connection to {address} closed by foreign host]\n"),
                );
                self.show_prompt(world, out);
                Turn::Stay
            }
        }
    }

    /// Unsolicited log messages appear on the console line (and on vty lines
    /// that asked with `terminal monitor`).
    fn pump_logs(&mut self, world: &World, out: &mut Vec<u8>) {
        let Some(node) = world.nodes.get(&self.node) else {
            return;
        };
        let Shell::Ios(ios) = &self.shell else { return };
        let from = self.log_cursor;
        self.log_cursor = node.log.next_seq;
        if !ios.shows_logs(node) || !matches!(self.stage, Stage::Line | Stage::Job { .. }) {
            return;
        }
        let lines: Vec<&str> = node.log.since(from).map(|e| e.line.as_str()).collect();
        if lines.is_empty() {
            return;
        }
        emit(out, "\n");
        for line in lines {
            emit(out, line);
            emit(out, "\n");
        }
        if matches!(self.stage, Stage::Line) && ios.logging_synchronous(node) {
            self.redraw(world, out);
        }
    }

    fn timed_out(&self, world: &World) -> bool {
        let Some(node) = world.nodes.get(&self.node) else {
            return false;
        };
        let Shell::Ios(ios) = &self.shell else {
            return false;
        };
        let idle = world.now.saturating_sub(self.last_input);
        matches!(self.stage, Stage::Line)
            && ios.exec_timeout(node).is_some_and(|limit| idle >= limit)
    }
}

/// A decoded keypress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Char(char),
    Enter,
    Backspace,
    Tab,
    Home,
    LineEnd,
    KillLine,
    KillWord,
    Left,
    Right,
    Up,
    Down,
    Redraw,
    /// Ctrl-C.
    Interrupt,
    /// Ctrl-Shift-6.
    Abort,
    /// Ctrl-Z.
    End,
}

impl Console {
    pub(crate) fn open(world: &mut World, node: NodeId) -> Result<Console, LabError> {
        world.session_keys += 1;
        let (mut session, greeting) = Session::open(world, node, None, world.session_keys)?;
        let mut console = Console {
            stack: Vec::new(),
            out: Vec::new(),
            esc: Esc::None,
            last_was_cr: false,
            cols: 80,
            rows: 24,
        };
        emit(&mut console.out, "\n");
        let outcome = session.apply(world, &mut console.out, greeting, false);
        console.stack.push(session);
        console.settle(world, outcome);
        Ok(console)
    }

    /// The node the console was opened on.
    pub(crate) fn root_node(&self) -> Option<NodeId> {
        self.stack.first().map(|s| s.node)
    }

    pub(crate) fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    pub(crate) fn close(&mut self, world: &mut World) {
        for session in self.stack.drain(..) {
            session.close(world);
        }
    }

    pub(crate) fn input(&mut self, world: &mut World, data: &[u8]) {
        for &byte in data {
            let key = match (self.esc, byte) {
                (Esc::Escape, b'[' | b'O') => {
                    self.esc = Esc::Bracket;
                    continue;
                }
                (Esc::Bracket, b'A') => Some(Key::Up),
                (Esc::Bracket, b'B') => Some(Key::Down),
                (Esc::Bracket, b'C') => Some(Key::Right),
                (Esc::Bracket, b'D') => Some(Key::Left),
                (Esc::Bracket, b'H') => Some(Key::Home),
                (Esc::Bracket, b'F') => Some(Key::LineEnd),
                // Parameters of longer sequences: keep swallowing until the final byte.
                (Esc::Bracket, b'0'..=b'9' | b';') => continue,
                (Esc::Bracket | Esc::Escape, _) => None,
                (Esc::None, 0x1b) => {
                    self.esc = Esc::Escape;
                    continue;
                }
                (Esc::None, b'\r') => Some(Key::Enter),
                (Esc::None, b'\n') if self.last_was_cr => None,
                (Esc::None, b'\n') => Some(Key::Enter),
                (Esc::None, 0x08 | 0x7f) => Some(Key::Backspace),
                (Esc::None, 0x09) => Some(Key::Tab),
                (Esc::None, 0x01) => Some(Key::Home),
                (Esc::None, 0x05) => Some(Key::LineEnd),
                (Esc::None, 0x15) => Some(Key::KillLine),
                (Esc::None, 0x17) => Some(Key::KillWord),
                (Esc::None, 0x02) => Some(Key::Left),
                (Esc::None, 0x06) => Some(Key::Right),
                (Esc::None, 0x10) => Some(Key::Up),
                (Esc::None, 0x0e) => Some(Key::Down),
                (Esc::None, 0x0c | 0x12) => Some(Key::Redraw),
                (Esc::None, 0x03) => Some(Key::Interrupt),
                (Esc::None, 0x1e) => Some(Key::Abort),
                (Esc::None, 0x1a) => Some(Key::End),
                (Esc::None, 0x20..=0x7e) => Some(Key::Char(byte as char)),
                (Esc::None, _) => None,
            };
            self.esc = Esc::None;
            self.last_was_cr = byte == b'\r';
            let Some(key) = key else { continue };
            let Some(session) = self.stack.last_mut() else {
                return;
            };
            let outcome = session.key(world, &mut self.out, key);
            self.settle(world, outcome);
        }
        self.pump(world);
    }

    /// Carry out what a session asked for at the level of the session stack.
    fn settle(&mut self, world: &mut World, outcome: Turn) {
        match outcome {
            Turn::Stay => {}
            Turn::Push(session, greeting) => {
                emit(&mut self.out, &greeting);
                self.stack.push(*session);
            }
            Turn::Close => self.pop(world),
            Turn::Reload => {
                let Some(session) = self.stack.last() else {
                    return;
                };
                let node = session.node;
                if world
                    .nodes
                    .get(&node)
                    .is_some_and(|n| n.powered && n.epoch == session.epoch)
                {
                    world.boot(node);
                    world.touch();
                    world.reconcile();
                }
                self.restart_top(world);
            }
        }
    }

    /// The top session ended: return to the one below, or start over at the root.
    fn pop(&mut self, world: &mut World) {
        let Some(session) = self.stack.pop() else {
            return;
        };
        session.close(world);
        match self.stack.last_mut() {
            Some(below) => {
                emit(
                    &mut self.out,
                    &format!(
                        "\n[Connection to {} closed by foreign host]\n",
                        session.reached_name()
                    ),
                );
                below.stage = Stage::Line;
                below.show_prompt(world, &mut self.out);
            }
            None => {
                // Logging out of the console line starts a fresh session on it.
                world.session_keys += 1;
                let hostname = world
                    .nodes
                    .get(&session.node)
                    .map(|n| n.cfg.hostname.clone())
                    .unwrap_or_default();
                if let Ok((mut fresh, greeting)) =
                    Session::open(world, session.node, None, world.session_keys)
                {
                    emit(
                        &mut self.out,
                        &format!("\n{hostname} con0 is now available\n\n\n\n"),
                    );
                    let outcome = fresh.apply(world, &mut self.out, greeting, false);
                    self.stack.push(fresh);
                    self.settle(world, outcome);
                }
            }
        }
    }

    /// The device under the top session restarted: sessions on it start afresh.
    fn restart_top(&mut self, world: &mut World) {
        let Some(session) = self.stack.pop() else {
            return;
        };
        session.close(world);
        if !self.stack.is_empty() {
            emit(
                &mut self.out,
                &format!(
                    "\n[Connection to {} closed by foreign host]\n",
                    session.reached_name()
                ),
            );
            if let Some(below) = self.stack.last_mut() {
                below.stage = Stage::Line;
                below.show_prompt(world, &mut self.out);
            }
            return;
        }
        world.session_keys += 1;
        match Session::open(world, session.node, None, world.session_keys) {
            Ok((mut fresh, greeting)) => {
                emit(&mut self.out, "\n\n");
                let outcome = fresh.apply(world, &mut self.out, greeting, false);
                self.stack.push(fresh);
                self.settle(world, outcome);
            }
            Err(_) => {
                // Powered off: keep a dead session so the console revives with the device.
                let mut dead = session;
                dead.stage = Stage::Dead;
                self.stack.push(dead);
            }
        }
    }

    /// Flush process output, log messages and timeouts for the active session.
    pub(crate) fn pump(&mut self, world: &mut World) {
        for _ in 0..4 {
            // A remote session does not outlive the device it runs on.
            let remote_gone = self.stack.len() > 1
                && self
                    .stack
                    .last()
                    .is_some_and(|s| world.nodes.get(&s.node).is_none_or(|n| !n.powered));
            if remote_gone {
                self.restart_top(world);
                continue;
            }
            let Some(session) = self.stack.last_mut() else {
                return;
            };
            let outcome = session.pump(world, &mut self.out);
            let again = !matches!(outcome, Turn::Stay);
            self.settle(world, outcome);
            if !again {
                break;
            }
        }
    }
}
