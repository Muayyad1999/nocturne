//! The command grammar engine.
//!
//! Every command is declared as a pattern string, e.g.
//! `ip address <ip:addr> <ip:mask> [secondary]`. Patterns compile to small
//! state machines, and a line is matched against all commands of a mode at
//! once. That single mechanism provides abbreviation matching, ambiguity
//! detection, the caret position of an invalid word, `?` help and Tab
//! completion.
//!
//! Pattern syntax:
//! * `word` — a keyword, matched by any unambiguous prefix;
//! * `<kind>` / `<kind:name>` / `<kind:name;description>` — an argument, where
//!   `kind` is `ip`, `word`, `line` (rest of the line), `if` (interface name),
//!   `mac`, or a numeric range such as `1-4094`;
//! * `[a | b]` optional, `(a | b)` choice, `{a | b}` any number in any order;
//! * `~` — everything after this point may be omitted in the `no` form.

use std::net::Ipv4Addr;

use crate::ifname::IfId;
use crate::util::mac::Mac;
use crate::util::net::parse_ip;
use crate::util::text::{common_prefix_len, is_abbrev};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Val {
    Ip(Ipv4Addr),
    Num(u64),
    Word(String),
    If(IfId),
    Mac(Mac),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArgKind {
    Ip,
    Num(u64, u64),
    Word,
    Line,
    If,
    Mac,
}

/// What a command handler receives.
#[derive(Clone, Debug, Default)]
pub(crate) struct Args {
    /// The line was entered with the `no` prefix.
    pub(crate) no: bool,
    keywords: Vec<&'static str>,
    values: Vec<(&'static str, Val)>,
}

impl Args {
    /// Was this keyword part of the line?
    pub(crate) fn has(&self, keyword: &str) -> bool {
        self.keywords.contains(&keyword)
    }

    /// The keywords of the line, in the order they were typed.
    pub(crate) fn keywords(&self) -> &[&'static str] {
        &self.keywords
    }

    fn get(&self, name: &str) -> Option<&Val> {
        self.values.iter().find(|(n, _)| *n == name).map(|(_, v)| v)
    }

    pub(crate) fn ip(&self, name: &str) -> Option<Ipv4Addr> {
        match self.get(name)? {
            Val::Ip(ip) => Some(*ip),
            _ => None,
        }
    }

    pub(crate) fn num(&self, name: &str) -> Option<u64> {
        match self.get(name)? {
            Val::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// A word or rest-of-line argument.
    pub(crate) fn text(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            Val::Word(text) => Some(text),
            _ => None,
        }
    }

    pub(crate) fn iface(&self, name: &str) -> Option<IfId> {
        match self.get(name)? {
            Val::If(id) => Some(*id),
            _ => None,
        }
    }

    pub(crate) fn mac(&self, name: &str) -> Option<Mac> {
        match self.get(name)? {
            Val::Mac(mac) => Some(*mac),
            _ => None,
        }
    }

    /// Every word captured under `name`, in the order typed.
    pub(crate) fn texts(&self, name: &str) -> Vec<&str> {
        self.values
            .iter()
            .filter(|(n, _)| *n == name)
            .filter_map(|(_, v)| match v {
                Val::Word(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Every value captured under `name` (arguments inside `{...}` may repeat).
    pub(crate) fn ips(&self, name: &str) -> Vec<Ipv4Addr> {
        self.values
            .iter()
            .filter(|(n, _)| *n == name)
            .filter_map(|(_, v)| match v {
                Val::Ip(ip) => Some(*ip),
                _ => None,
            })
            .collect()
    }
}

/// A command declaration.
pub(crate) struct Cmd<H> {
    pub(crate) pattern: &'static str,
    /// Describes the command (its last leading keyword) in `?` listings.
    pub(crate) help: &'static str,
    /// The command also exists in a `no` form.
    pub(crate) negatable: bool,
    pub(crate) run: H,
}

#[derive(Debug)]
enum Pat {
    /// Keyword, description, and whether the description is the command's own.
    Kw(&'static str, &'static str, bool),
    Arg {
        name: &'static str,
        kind: ArgKind,
        desc: &'static str,
    },
    Seq(Vec<Pat>),
    Opt(Box<Pat>),
    Alt(Vec<Pat>),
    Many(Vec<Pat>),
    NoCut,
}

#[derive(Debug)]
enum State {
    Kw {
        word: &'static str,
        help: &'static str,
        primary: bool,
        next: usize,
    },
    Arg {
        name: &'static str,
        kind: ArgKind,
        desc: &'static str,
        next: usize,
    },
    Split(Vec<usize>),
    /// In the `no` form the command may end here.
    NoCut(usize),
    Accept,
}

struct Compiled<H> {
    states: Vec<State>,
    start: usize,
    negatable: bool,
    run: H,
}

pub(crate) struct Grammar<H> {
    commands: Vec<Compiled<H>>,
}

// ----------------------------------------------------------- pattern parsing

struct PatternParser<'d> {
    tokens: Vec<&'static str>,
    pos: usize,
    dictionary: &'d [(&'static str, &'static str)],
    command_help: &'static str,
    /// Number of keywords that open the pattern; the last one carries `command_help`.
    leading: usize,
}

fn lex(pattern: &'static str) -> Vec<&'static str> {
    let mut tokens = Vec::new();
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b' ' => i += 1,
            b'[' | b']' | b'(' | b')' | b'{' | b'}' | b'|' | b'~' => {
                tokens.push(&pattern[i..i + 1]);
                i += 1;
            }
            b'<' => {
                let end = pattern[i..].find('>').map_or(bytes.len(), |e| i + e + 1);
                tokens.push(&pattern[i..end]);
                i = end;
            }
            _ => {
                let end = pattern[i..]
                    .find([' ', '[', ']', '(', ')', '{', '}', '|', '<'])
                    .map_or(bytes.len(), |e| i + e);
                tokens.push(&pattern[i..end]);
                i = end;
            }
        }
    }
    tokens
}

impl PatternParser<'_> {
    fn lookup(&self, path: &[&'static str]) -> &'static str {
        // Longest matching suffix of the keyword path wins.
        (0..path.len())
            .find_map(|skip| {
                let key = path[skip..].join(" ");
                self.dictionary
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| *v)
            })
            .unwrap_or("")
    }

    /// Parse alternatives up to (not including) the closing token.
    fn alternatives(&mut self, path: &[&'static str], top: bool) -> Vec<Pat> {
        let mut branches = Vec::new();
        loop {
            branches.push(self.sequence(path, top));
            if self.tokens.get(self.pos) == Some(&"|") {
                self.pos += 1;
            } else {
                return branches;
            }
        }
    }

    fn sequence(&mut self, path: &[&'static str], top: bool) -> Pat {
        let mut path = path.to_vec();
        let mut items = Vec::new();
        let mut leading_run = top;
        while let Some(token) = self.tokens.get(self.pos).copied() {
            match token {
                "]" | ")" | "}" | "|" => break,
                "[" | "(" | "{" => {
                    self.pos += 1;
                    let mut branches = self.alternatives(&path, false);
                    self.pos += 1; // the closing bracket
                    items.push(match token {
                        "[" if branches.len() == 1 => Pat::Opt(Box::new(branches.remove(0))),
                        "[" => Pat::Opt(Box::new(Pat::Alt(branches))),
                        "(" => Pat::Alt(branches),
                        _ => Pat::Many(branches),
                    });
                    leading_run = false;
                }
                "~" => {
                    self.pos += 1;
                    items.push(Pat::NoCut);
                }
                _ if token.starts_with('<') => {
                    self.pos += 1;
                    items.push(parse_arg(token));
                    leading_run = false;
                }
                _ => {
                    self.pos += 1;
                    path.push(token);
                    let primary = leading_run && path.len() == self.leading;
                    let help = if primary {
                        self.command_help
                    } else {
                        self.lookup(&path)
                    };
                    items.push(Pat::Kw(token, help, primary));
                }
            }
        }
        Pat::Seq(items)
    }
}

fn parse_arg(token: &'static str) -> Pat {
    let inner = token.trim_start_matches('<').trim_end_matches('>');
    let (spec, desc) = inner.split_once(';').unwrap_or((inner, ""));
    let (kind_text, name) = spec.split_once(':').unwrap_or((spec, spec));
    let (kind, default_desc) = match kind_text {
        "ip" => (ArgKind::Ip, "IP address"),
        "word" => (ArgKind::Word, ""),
        "line" => (ArgKind::Line, ""),
        "if" => (ArgKind::If, "Interface name"),
        "mac" => (ArgKind::Mac, "48 bit MAC address"),
        range => {
            let (lo, hi) = range
                .split_once('-')
                .unwrap_or_else(|| panic!("bad argument kind in pattern: {token}"));
            let parse = |s: &str| {
                s.parse::<u64>()
                    .unwrap_or_else(|_| panic!("bad range in pattern: {token}"))
            };
            (ArgKind::Num(parse(lo), parse(hi)), "")
        }
    };
    Pat::Arg {
        name,
        kind,
        desc: if desc.is_empty() { default_desc } else { desc },
    }
}

fn compile(pat: &Pat, next: usize, states: &mut Vec<State>) -> usize {
    match pat {
        Pat::Kw(word, help, primary) => {
            states.push(State::Kw {
                word,
                help,
                primary: *primary,
                next,
            });
            states.len() - 1
        }
        Pat::Arg { name, kind, desc } => {
            states.push(State::Arg {
                name,
                kind: *kind,
                desc,
                next,
            });
            states.len() - 1
        }
        Pat::Seq(items) => items
            .iter()
            .rev()
            .fold(next, |next, item| compile(item, next, states)),
        Pat::Opt(inner) => {
            let start = compile(inner, next, states);
            states.push(State::Split(vec![start, next]));
            states.len() - 1
        }
        Pat::Alt(branches) => {
            let starts: Vec<usize> = branches.iter().map(|b| compile(b, next, states)).collect();
            states.push(State::Split(starts));
            states.len() - 1
        }
        Pat::Many(branches) => {
            // Reserve the loop head so the branches can jump back to it.
            states.push(State::Accept);
            let head = states.len() - 1;
            let mut targets: Vec<usize> =
                branches.iter().map(|b| compile(b, head, states)).collect();
            targets.push(next);
            states[head] = State::Split(targets);
            head
        }
        Pat::NoCut => {
            states.push(State::NoCut(next));
            states.len() - 1
        }
    }
}

// ------------------------------------------------------------------ matching

#[derive(Clone, Copy, Debug)]
struct Token<'a> {
    text: &'a str,
    at: usize,
}

fn tokenize(line: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (i, c) in line.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                tokens.push(Token {
                    text: &line[s..i],
                    at: s,
                });
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    if let Some(s) = start {
        tokens.push(Token {
            text: &line[s..],
            at: s,
        });
    }
    tokens
}

#[derive(Clone, Debug)]
struct Thread {
    command: usize,
    state: usize,
    /// Tokens still to swallow (rest-of-line argument, second half of `gig 0/1`).
    skip: usize,
    keywords: Vec<&'static str>,
    values: Vec<(&'static str, Val)>,
}

/// One way a token can be consumed.
struct Hit {
    thread: usize,
    next: usize,
    skip: usize,
    keyword: Option<(&'static str, bool)>,
    value: Option<(&'static str, Val)>,
}

pub(crate) enum Parsed<H> {
    Empty,
    Ok {
        run: H,
        args: Args,
    },
    Ambiguous,
    Incomplete,
    /// Byte offset of the first character that could not be matched.
    Invalid {
        at: usize,
    },
}

/// A `?` listing entry: label and description.
pub(crate) type HelpEntry = (String, String);

impl<H: Copy> Grammar<H> {
    /// Compile a command table. Panics on a malformed pattern (a programming
    /// error; every table is built by a unit test).
    pub(crate) fn new(
        commands: Vec<Cmd<H>>,
        dictionary: &[(&'static str, &'static str)],
    ) -> Grammar<H> {
        let compiled = commands
            .into_iter()
            .map(|cmd| {
                let tokens = lex(cmd.pattern);
                let leading = tokens
                    .iter()
                    .take_while(|t| !matches!(t.as_bytes()[0], b'[' | b'(' | b'{' | b'<' | b'~'))
                    .count();
                let mut parser = PatternParser {
                    tokens,
                    pos: 0,
                    dictionary,
                    command_help: cmd.help,
                    leading,
                };
                let pat = parser.sequence(&[], true);
                assert!(
                    parser.pos == parser.tokens.len(),
                    "unbalanced pattern: {}",
                    cmd.pattern
                );
                let mut states = vec![State::Accept];
                let start = compile(&pat, 0, &mut states);
                Compiled {
                    states,
                    start,
                    negatable: cmd.negatable,
                    run: cmd.run,
                }
            })
            .collect();
        Grammar { commands: compiled }
    }

    /// States reachable from `state` without consuming a token; the flag says
    /// whether the command may end here.
    fn closure(&self, command: usize, state: usize, negate: bool) -> (Vec<usize>, bool) {
        let states = &self.commands[command].states;
        let mut out = Vec::new();
        let mut accept = false;
        let mut stack = vec![state];
        let mut seen = vec![false; states.len()];
        while let Some(s) = stack.pop() {
            if std::mem::replace(&mut seen[s], true) {
                continue;
            }
            match &states[s] {
                State::Split(targets) => stack.extend(targets.iter().rev()),
                State::NoCut(next) => {
                    accept |= negate;
                    stack.push(*next);
                }
                State::Accept => accept = true,
                State::Kw { .. } | State::Arg { .. } => out.push(s),
            }
        }
        (out, accept)
    }

    fn initial(&self, negate: bool) -> Vec<Thread> {
        self.commands
            .iter()
            .enumerate()
            .filter(|(_, c)| !negate || c.negatable)
            .map(|(i, c)| Thread {
                command: i,
                state: c.start,
                skip: 0,
                keywords: Vec::new(),
                values: Vec::new(),
            })
            .collect()
    }

    /// Consume one token. `Err` carries (ambiguous, matched prefix length).
    fn step(
        &self,
        threads: &[Thread],
        tokens: &[Token<'_>],
        index: usize,
        line: &str,
        negate: bool,
    ) -> Result<Vec<Thread>, (bool, usize)> {
        let token = tokens[index];
        let mut next_threads: Vec<Thread> = Vec::new();
        let mut hits: Vec<Hit> = Vec::new();
        let mut best_prefix = 0;
        for (t, thread) in threads.iter().enumerate() {
            if thread.skip > 0 {
                next_threads.push(Thread {
                    skip: thread.skip - 1,
                    ..thread.clone()
                });
                continue;
            }
            let (states, _) = self.closure(thread.command, thread.state, negate);
            for s in states {
                match &self.commands[thread.command].states[s] {
                    State::Kw { word, next, .. } => {
                        if is_abbrev(token.text, word) {
                            let exact = token.text.eq_ignore_ascii_case(word);
                            hits.push(Hit {
                                thread: t,
                                next: *next,
                                skip: 0,
                                keyword: Some((word, exact)),
                                value: None,
                            });
                        } else {
                            best_prefix = best_prefix.max(common_prefix_len(token.text, word));
                        }
                    }
                    State::Arg {
                        name, kind, next, ..
                    } => {
                        if let Some((value, skip)) = match_arg(*kind, tokens, index, line) {
                            hits.push(Hit {
                                thread: t,
                                next: *next,
                                skip,
                                keyword: None,
                                value: Some((name, value)),
                            });
                        }
                    }
                    State::Split(_) | State::NoCut(_) | State::Accept => {}
                }
            }
        }
        // An exact keyword beats longer keywords it is a prefix of.
        if hits.iter().any(|h| matches!(h.keyword, Some((_, true)))) {
            hits.retain(|h| !matches!(h.keyword, Some((_, false))));
        }
        let mut words: Vec<&str> = hits
            .iter()
            .filter_map(|h| h.keyword.map(|(w, _)| w))
            .collect();
        words.sort_unstable();
        words.dedup();
        let has_values = hits.iter().any(|h| h.value.is_some());
        if words.len() > 1 && !has_values {
            return Err((true, token.text.len()));
        }
        for hit in hits {
            let source = &threads[hit.thread];
            let mut thread = Thread {
                state: hit.next,
                skip: hit.skip,
                ..source.clone()
            };
            if let Some((word, _)) = hit.keyword {
                thread.keywords.push(word);
            }
            if let Some(value) = hit.value {
                thread.values.push(value);
            }
            let duplicate = next_threads.iter().any(|t| {
                t.command == thread.command && t.state == thread.state && t.skip == thread.skip
            });
            if !duplicate {
                next_threads.push(thread);
            }
        }
        if next_threads.is_empty() {
            Err((false, best_prefix))
        } else {
            Ok(next_threads)
        }
    }

    /// Match a whole line.
    pub(crate) fn parse(&self, line: &str, negate: bool) -> Parsed<H> {
        let tokens = tokenize(line);
        if tokens.is_empty() {
            return Parsed::Empty;
        }
        let mut threads = self.initial(negate);
        for index in 0..tokens.len() {
            match self.step(&threads, &tokens, index, line, negate) {
                Ok(next) => threads = next,
                Err((true, _)) => return Parsed::Ambiguous,
                Err((false, prefix)) => {
                    return Parsed::Invalid {
                        at: tokens[index].at + prefix.min(tokens[index].text.len()),
                    };
                }
            }
        }
        let done = threads
            .iter()
            .filter(|t| t.skip == 0)
            .find(|t| self.closure(t.command, t.state, negate).1);
        match done {
            Some(thread) => Parsed::Ok {
                run: self.commands[thread.command].run,
                args: Args {
                    no: negate,
                    keywords: thread.keywords.clone(),
                    values: thread.values.clone(),
                },
            },
            None => Parsed::Incomplete,
        }
    }

    /// What could come next after `line` (which ends at a word boundary when
    /// `partial` is `None`, or with an unfinished word otherwise).
    fn expectations(
        &self,
        line: &str,
        negate: bool,
    ) -> Option<(Vec<HelpEntry>, Vec<HelpEntry>, bool, Option<String>)> {
        let mut tokens = tokenize(line);
        let partial = match tokens.last() {
            Some(last) if !line.ends_with(char::is_whitespace) => {
                let text = last.text.to_string();
                tokens.pop();
                Some(text)
            }
            _ => None,
        };
        let mut threads = self.initial(negate);
        for index in 0..tokens.len() {
            threads = self.step(&threads, &tokens, index, line, negate).ok()?;
        }
        let mut keywords: Vec<(String, String, bool)> = Vec::new();
        let mut arguments: Vec<HelpEntry> = Vec::new();
        let mut accept = false;
        for thread in threads.iter().filter(|t| t.skip == 0) {
            let (states, ends) = self.closure(thread.command, thread.state, negate);
            accept |= ends;
            for s in states {
                match &self.commands[thread.command].states[s] {
                    State::Kw {
                        word,
                        help,
                        primary,
                        ..
                    } => {
                        // A command declared without a description is accepted but not advertised.
                        let hidden = *primary && help.is_empty();
                        if hidden || partial.as_deref().is_some_and(|p| !is_abbrev(p, word)) {
                            continue;
                        }
                        // A command's own description beats a dictionary one.
                        match keywords.iter_mut().find(|(w, _, _)| w == word) {
                            Some(entry) if (*primary && !entry.2) || entry.1.is_empty() => {
                                entry.1 = (*help).to_string();
                                entry.2 |= *primary;
                            }
                            Some(_) => {}
                            None => {
                                keywords.push(((*word).to_string(), (*help).to_string(), *primary))
                            }
                        }
                    }
                    State::Arg { kind, desc, .. } => {
                        for entry in arg_help(*kind, desc) {
                            if !arguments.contains(&entry) {
                                arguments.push(entry);
                            }
                        }
                    }
                    State::Split(_) | State::NoCut(_) | State::Accept => {}
                }
            }
        }
        keywords.sort();
        let keywords = keywords
            .into_iter()
            .map(|(word, help, _)| (word, help))
            .collect();
        Some((arguments, keywords, accept, partial))
    }

    /// The `?` listing for `line`: `None` when the line cannot be parsed that far.
    /// The flag is true when the listing is for a partial word (names only).
    pub(crate) fn help(&self, line: &str, negate: bool) -> Option<(Vec<HelpEntry>, bool)> {
        let (arguments, keywords, accept, partial) = self.expectations(line, negate)?;
        if partial.is_some() {
            return Some((keywords, true));
        }
        let mut entries = arguments;
        entries.extend(keywords);
        if accept {
            entries.push(("<cr>".to_string(), String::new()));
        }
        Some((entries, false))
    }

    /// Tab completion: the full keyword for the unfinished last word, if unique.
    pub(crate) fn complete(&self, line: &str, negate: bool) -> Option<String> {
        let (_, keywords, _, partial) = self.expectations(line, negate)?;
        partial.as_ref()?;
        match keywords.as_slice() {
            [(word, _)] => Some(word.clone()),
            _ => None,
        }
    }
}

/// Try to read an argument at `tokens[index]`; returns the value and how many
/// further tokens it swallowed.
fn match_arg(
    kind: ArgKind,
    tokens: &[Token<'_>],
    index: usize,
    line: &str,
) -> Option<(Val, usize)> {
    let token = tokens[index];
    match kind {
        ArgKind::Ip => parse_ip(token.text).map(|ip| (Val::Ip(ip), 0)),
        ArgKind::Num(lo, hi) => {
            let digits = !token.text.is_empty()
                && token.text.len() <= 19
                && token.text.bytes().all(|b| b.is_ascii_digit());
            let value: u64 = digits.then(|| token.text.parse().ok()).flatten()?;
            (lo..=hi).contains(&value).then_some((Val::Num(value), 0))
        }
        ArgKind::Word => Some((Val::Word(token.text.to_string()), 0)),
        ArgKind::Line => Some((
            Val::Word(line[token.at..].trim_end().to_string()),
            tokens.len() - index - 1,
        )),
        ArgKind::Mac => Mac::parse(token.text).map(|mac| (Val::Mac(mac), 0)),
        ArgKind::If => {
            if let Some(id) = IfId::parse(token.text) {
                return Some((Val::If(id), 0));
            }
            // `gig 0/1`: the type word and the numbers as separate tokens.
            let next = tokens.get(index + 1)?;
            if IfId::is_type_word(token.text) && next.text.starts_with(|c: char| c.is_ascii_digit())
            {
                return IfId::parse(&format!("{}{}", token.text, next.text))
                    .map(|id| (Val::If(id), 1));
            }
            None
        }
    }
}

fn arg_help(kind: ArgKind, desc: &'static str) -> Vec<HelpEntry> {
    let one = |label: &str| vec![(label.to_string(), desc.to_string())];
    match kind {
        ArgKind::Ip => one("A.B.C.D"),
        ArgKind::Num(lo, hi) => one(&format!("<{lo}-{hi}>")),
        ArgKind::Word => one("WORD"),
        ArgKind::Line => one("LINE"),
        ArgKind::Mac => one("H.H.H"),
        ArgKind::If => [
            ("FastEthernet", "FastEthernet IEEE 802.3"),
            ("GigabitEthernet", "GigabitEthernet IEEE 802.3z"),
            ("Loopback", "Loopback interface"),
            ("Port-channel", "Ethernet Channel of interfaces"),
            ("TenGigabitEthernet", "Ten Gigabit Ethernet"),
            ("Vlan", "Catalyst Vlans"),
        ]
        .iter()
        .map(|(label, help)| ((*label).to_string(), (*help).to_string()))
        .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Ctx {
        ran: Vec<String>,
    }

    fn cmd(
        pattern: &'static str,
        help: &'static str,
        negatable: bool,
        run: fn(&mut Ctx, &Args),
    ) -> Cmd<fn(&mut Ctx, &Args)> {
        Cmd {
            pattern,
            help,
            negatable,
            run,
        }
    }

    fn grammar() -> Grammar<fn(&mut Ctx, &Args)> {
        Grammar::new(
            vec![
                cmd("show ip interface brief", "Brief summary", false, |c, _| {
                    c.ran.push("brief".into())
                }),
                cmd(
                    "show ip route [<ip:prefix> | static | connected]",
                    "IP routing table",
                    false,
                    |c, a| {
                        c.ran.push(format!(
                            "route {:?} static={}",
                            a.ip("prefix"),
                            a.has("static")
                        ))
                    },
                ),
                cmd(
                    "show interfaces [<if>]",
                    "Interface status",
                    false,
                    |c, a| {
                        c.ran
                            .push(format!("interfaces {:?}", a.iface("if").map(|i| i.name())))
                    },
                ),
                cmd("show interfaces status", "Port status", false, |c, _| {
                    c.ran.push("status".into())
                }),
                cmd("show inventory", "Hardware inventory", false, |c, _| {
                    c.ran.push("inventory".into())
                }),
                cmd("switchport", "Make it a switch port", true, |c, a| {
                    c.ran.push(format!("switchport no={}", a.no))
                }),
                cmd(
                    "ip address ~ <ip:addr> <ip:mask> [secondary]",
                    "Set the IP address",
                    true,
                    |c, a| {
                        c.ran.push(format!(
                            "address {:?} {:?} sec={} no={}",
                            a.ip("addr"),
                            a.ip("mask"),
                            a.has("secondary"),
                            a.no
                        ))
                    },
                ),
                cmd("description ~ <line:text>", "Describe", true, |c, a| {
                    c.ran.push(format!("desc {:?}", a.text("text")))
                }),
                cmd(
                    "ping <ip:dst> {repeat <1-100:count> | size <36-18024:size>}",
                    "Send echo",
                    false,
                    |c, a| {
                        c.ran.push(format!(
                            "ping {:?} {:?} {:?}",
                            a.ip("dst"),
                            a.num("count"),
                            a.num("size")
                        ))
                    },
                ),
                cmd("vlan <1-4094:id>", "VLAN", true, |c, a| {
                    c.ran.push(format!("vlan {:?}", a.num("id")))
                }),
            ],
            &[
                ("show", "Show running system information"),
                ("show ip", "IP information"),
            ],
        )
    }

    fn run(line: &str, negate: bool) -> String {
        let g = grammar();
        let mut ctx = Ctx::default();
        match g.parse(line, negate) {
            Parsed::Ok { run, args, .. } => {
                run(&mut ctx, &args);
                ctx.ran.join(";")
            }
            Parsed::Ambiguous => "ambiguous".into(),
            Parsed::Incomplete => "incomplete".into(),
            Parsed::Invalid { at } => format!("invalid@{at}"),
            Parsed::Empty => "empty".into(),
        }
    }

    #[test]
    fn abbreviations_and_arguments() {
        assert_eq!(run("show ip interface brief", false), "brief");
        assert_eq!(run("sh ip int br", false), "brief");
        assert_eq!(run("SH IP INT BR", false), "brief");
        assert_eq!(run("sh ip ro", false), "route None static=false");
        assert_eq!(
            run("sh ip ro 10.0.0.0", false),
            "route Some(10.0.0.0) static=false"
        );
        assert_eq!(run("sh ip ro st", false), "route None static=true");
        assert_eq!(run("sh int", false), "interfaces None");
        assert_eq!(
            run("sh int g0/1", false),
            "interfaces Some(\"GigabitEthernet0/1\")"
        );
        assert_eq!(
            run("sh int gig 0/1", false),
            "interfaces Some(\"GigabitEthernet0/1\")"
        );
        assert_eq!(run("sh int status", false), "status");
        assert_eq!(run("sh int st", false), "status");
        assert_eq!(
            run("ping 10.0.0.1 size 200 repeat 3", false),
            "ping Some(10.0.0.1) Some(3) Some(200)"
        );
        assert_eq!(
            run("ip address 10.0.0.1 255.255.255.0 secondary", false),
            "address Some(10.0.0.1) Some(255.255.255.0) sec=true no=false"
        );
        assert_eq!(
            run("description  uplink to  core ", false),
            "desc Some(\"uplink to  core\")"
        );
    }

    #[test]
    fn errors() {
        assert_eq!(run("", false), "empty");
        assert_eq!(run("sh i", false), "ambiguous");
        assert_eq!(run("s", false), "ambiguous");
        assert_eq!(run("show ip", false), "incomplete");
        assert_eq!(run("ip address 10.0.0.1", false), "incomplete");
        assert_eq!(run("show ip routx", false), "invalid@12");
        assert_eq!(run("frobnicate", false), "invalid@0");
        assert_eq!(run("show ip route 300.1.1.1", false), "invalid@14");
        assert_eq!(run("vlan 5000", false), "invalid@5");
        assert_eq!(run("vlan 99999999999999999999999", false), "invalid@5");
        assert_eq!(run("switchport now", false), "invalid@11");
    }

    #[test]
    fn no_forms() {
        assert_eq!(run("switchport", true), "switchport no=true");
        assert_eq!(
            run("ip address", true),
            "address None None sec=false no=true"
        );
        assert_eq!(
            run("ip address 10.0.0.1 255.255.255.0", true),
            "address Some(10.0.0.1) Some(255.255.255.0) sec=false no=true"
        );
        assert_eq!(run("description", true), "desc None");
        // Not negatable: the command does not exist after `no`.
        assert_eq!(run("show inventory", true), "invalid@1");
        assert_eq!(run("ip address", false), "incomplete");
    }

    #[test]
    fn help_and_completion() {
        let g = grammar();
        let (entries, partial) = g.help("show ", false).unwrap();
        assert!(!partial);
        assert_eq!(
            entries,
            vec![
                ("interfaces".to_string(), "Interface status".to_string()),
                ("inventory".to_string(), "Hardware inventory".to_string()),
                ("ip".to_string(), "IP information".to_string()),
            ]
        );
        let (entries, partial) = g.help("show i", false).unwrap();
        assert!(partial);
        assert_eq!(
            entries.iter().map(|e| e.0.as_str()).collect::<Vec<_>>(),
            ["interfaces", "inventory", "ip"]
        );
        let (entries, _) = g.help("show ip route ", false).unwrap();
        assert_eq!(entries.first().unwrap().0, "A.B.C.D");
        assert_eq!(entries.last().unwrap().0, "<cr>");
        let (entries, _) = g.help("", false).unwrap();
        assert!(
            entries
                .iter()
                .any(|(w, h)| w == "show" && h == "Show running system information")
        );
        assert!(g.help("bogus ", false).is_none());
        assert_eq!(g.complete("sh ip int b", false).as_deref(), Some("brief"));
        assert_eq!(g.complete("sh i", false), None);
        assert_eq!(g.complete("show ", false), None);
        assert_eq!(g.complete("sw", false).as_deref(), Some("switchport"));
    }
}
