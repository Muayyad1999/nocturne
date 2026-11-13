//! A configuration as numbered logical lines in their block structure.
//!
//! The hierarchical engine (`nocturne_drivers::hier`) parses a configuration
//! into a tree but forgets where each line was. Search results, diff sections
//! and compliance evidence all have to point at a line, so this module keeps
//! the positions: every logical line knows its physical line number, its
//! depth and the line that encloses it.
//!
//! It follows the same grammar rules as `hier` (comment characters, display
//! noise, multi-line IOS banners), and adds what `hier` leaves out for
//! RouterOS: an `/export` groups commands under menu lines and wraps long
//! commands, an `/export terse` repeats the menu on every line. Both are read
//! into the same shape here — a menu and the commands in it — so a search hit
//! or a compliance rule means the same thing whichever way the device printed
//! its configuration.

use std::sync::LazyLock;

use nocturne_drivers::hier::{Node, Syntax};
use regex::Regex;

/// One logical configuration line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// Number (from 1) of its first physical line in the text.
    pub no: usize,
    /// Physical lines it covers: more than one for a multi-line banner or a wrapped RouterOS command.
    pub span: usize,
    /// How many blocks enclose it.
    pub depth: usize,
    /// Index into [`Outline::lines`] of the line that encloses it.
    pub parent: Option<usize>,
    /// The content without indentation. A banner keeps its line breaks.
    pub text: String,
}

/// A top-level block: its opening line and what is inside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section<'a> {
    pub header: &'a str,
    /// Index of the (first) opening line.
    pub index: usize,
    /// Indices of every line inside, at any depth.
    pub members: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outline {
    pub syntax: Syntax,
    pub lines: Vec<Line>,
}

static BANNER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^banner\s+\S+\s+(\^C|\S)").expect("banner pattern"));

/// Words that start a RouterOS command and therefore end the menu path before them.
const ROUTEROS_VERBS: [&str; 12] = [
    "add", "set", "remove", "enable", "disable", "unset", "reset", "move", "edit", "print",
    "export", "find",
];

impl Outline {
    pub fn parse(text: &str, syntax: Syntax) -> Outline {
        let lines = match syntax {
            Syntax::Ios | Syntax::Vrp => indented(text, syntax),
            Syntax::Routeros => routeros(text),
            Syntax::Set | Syntax::Flat => flat(text),
        };
        Outline { syntax, lines }
    }

    /// The lines enclosing `index`, outermost first.
    pub fn path(&self, index: usize) -> Vec<&str> {
        let mut path = Vec::new();
        let mut next = self.lines[index].parent;
        while let Some(parent) = next {
            path.push(self.lines[parent].text.as_str());
            next = self.lines[parent].parent;
        }
        path.reverse();
        path
    }

    /// The logical line that covers physical line `no`, if that line is configuration.
    pub fn index_at(&self, no: usize) -> Option<usize> {
        // A terse RouterOS line holds a menu and a command; the command comes last and wins.
        let index = self
            .lines
            .partition_point(|line| line.no <= no)
            .checked_sub(1)?;
        let line = &self.lines[index];
        (no < line.no + line.span).then_some(index)
    }

    /// The sections physical line `no` sits in, outermost first. Empty for a
    /// top-level line, a comment or a blank line.
    pub fn enclosing(&self, no: usize) -> Vec<String> {
        let Some(index) = self.index_at(no) else {
            return Vec::new();
        };
        let line = &self.lines[index];
        let mut path: Vec<String> = self.path(index).into_iter().map(first_line).collect();
        // The body of a banner belongs to the banner. (The second half of a
        // wrapped RouterOS command is simply more of that command.)
        if no > line.no && line.text.contains('\n') {
            path.push(first_line(&line.text));
        }
        path
    }

    /// The section a change to physical line `no` is filed under: like
    /// [`enclosing`](Self::enclosing), except that a line which opens a block
    /// counts as part of that block.
    pub fn section_of(&self, no: usize) -> Vec<String> {
        let mut path = self.enclosing(no);
        if let Some(index) = self.index_at(no)
            && no == self.lines[index].no
            && self.has_children(index)
        {
            path.push(first_line(&self.lines[index].text));
        }
        path
    }

    pub fn has_children(&self, index: usize) -> bool {
        self.lines
            .get(index + 1)
            .is_some_and(|next| next.parent == Some(index))
    }

    /// Indices of everything inside the block that `index` opens.
    pub fn descendants(&self, index: usize) -> std::ops::Range<usize> {
        let depth = self.lines[index].depth;
        let end = self.lines[index + 1..]
            .iter()
            .position(|line| line.depth <= depth)
            .map_or(self.lines.len(), |offset| index + 1 + offset);
        index + 1..end
    }

    /// Whether `index` is a RouterOS menu line (`/ip service`) rather than a command.
    pub fn is_menu(&self, index: usize) -> bool {
        let line = &self.lines[index];
        self.syntax == Syntax::Routeros && line.depth == 0 && line.text.starts_with('/')
    }

    /// The complete statement a line makes at the top level of the
    /// configuration, for the lines that make one: a global line, or on
    /// RouterOS a command together with its menu.
    pub fn statement(&self, index: usize) -> Option<String> {
        let line = &self.lines[index];
        match self.syntax {
            Syntax::Routeros if self.is_menu(index) => None,
            Syntax::Routeros => Some(match line.parent {
                Some(menu) => format!("{} {}", self.lines[menu].text, line.text),
                None => line.text.clone(),
            }),
            _ => (line.depth == 0).then(|| line.text.clone()),
        }
    }

    /// A line as a pattern sees it: indented one space per level, the way
    /// `show running-config` prints it (so `^ntp ` only matches a global
    /// line); on RouterOS the command together with its menu. Menu lines say
    /// nothing by themselves and have no such form.
    pub fn display(&self, index: usize) -> Option<String> {
        let line = &self.lines[index];
        match self.syntax {
            Syntax::Routeros => self.statement(index),
            _ => Some(format!("{}{}", " ".repeat(line.depth), line.text)),
        }
    }

    /// The top-level blocks, or `None` when this grammar has no blocks.
    /// RouterOS commands are grouped by menu wherever the menu was written.
    pub fn sections(&self) -> Option<Vec<Section<'_>>> {
        match self.syntax {
            Syntax::Ios | Syntax::Vrp => Some(
                self.lines
                    .iter()
                    .enumerate()
                    .filter(|(_, line)| line.depth == 0)
                    .map(|(index, line)| Section {
                        header: &line.text,
                        index,
                        members: self.descendants(index).collect(),
                    })
                    .collect(),
            ),
            Syntax::Routeros => {
                let mut sections: Vec<Section<'_>> = Vec::new();
                for index in (0..self.lines.len()).filter(|index| self.is_menu(*index)) {
                    let header = self.lines[index].text.as_str();
                    let members = self.descendants(index);
                    match sections.iter_mut().find(|known| known.header == header) {
                        Some(known) => known.members.extend(members),
                        None => sections.push(Section {
                            header,
                            index,
                            members: members.collect(),
                        }),
                    }
                }
                Some(sections)
            }
            Syntax::Set | Syntax::Flat => None,
        }
    }

    /// The configuration as a tree. RouterOS becomes a flat list of complete
    /// statements, so that a block written in one export style can be compared
    /// with a configuration fetched in the other.
    pub fn tree(&self) -> Vec<Node> {
        fn build(outline: &Outline, range: std::ops::Range<usize>) -> Vec<Node> {
            let mut nodes = Vec::new();
            let mut index = range.start;
            while index < range.end {
                let inner = outline.descendants(index);
                let next = inner.end;
                nodes.push(Node {
                    text: outline.lines[index].text.clone(),
                    children: build(outline, inner),
                });
                index = next;
            }
            nodes
        }
        match self.syntax {
            Syntax::Ios | Syntax::Vrp => build(self, 0..self.lines.len()),
            _ => (0..self.lines.len())
                .filter_map(|index| self.statement(index))
                .map(|text| Node {
                    text,
                    children: Vec::new(),
                })
                .collect(),
        }
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_string()
}

/// Lines that are part of the display, not of the configuration (as `hier` sees it).
fn is_noise(line: &str, syntax: Syntax) -> bool {
    match syntax {
        Syntax::Ios => {
            line == "end"
                || line.starts_with("Building configuration")
                || line.starts_with("Current configuration")
                || line.starts_with("Last configuration change")
                || line.starts_with("NVRAM config last updated")
        }
        Syntax::Vrp => line == "return",
        _ => false,
    }
}

fn indented(text: &str, syntax: Syntax) -> Vec<Line> {
    let comment = if syntax == Syntax::Vrp { '#' } else { '!' };
    let mut lines: Vec<Line> = Vec::new();
    // The blocks that are open: (indentation, index of the opening line).
    let mut open: Vec<(usize, usize)> = Vec::new();
    let mut physical = text.lines().enumerate();
    while let Some((index, raw)) = physical.next() {
        let line = raw.trim_end();
        let content = line.trim_start();
        if content.is_empty() || content.starts_with(comment) || is_noise(content, syntax) {
            continue;
        }
        let indent = line.len() - content.len();
        let mut text = content.to_string();
        let mut span = 1;

        // A banner is one value that runs to the line repeating its delimiter.
        if syntax == Syntax::Ios
            && let Some(delimiter) = BANNER.captures(content).and_then(|caps| caps.get(1))
            && !content[delimiter.end()..].contains(delimiter.as_str())
        {
            for (_, next) in physical.by_ref() {
                text.push('\n');
                text.push_str(next.trim_end());
                span += 1;
                if next.contains(delimiter.as_str()) {
                    break;
                }
            }
        }

        while open.last().is_some_and(|(at, _)| *at >= indent) {
            open.pop();
        }
        lines.push(Line {
            no: index + 1,
            span,
            depth: open.len(),
            parent: open.last().map(|(_, opener)| *opener),
            text,
        });
        open.push((indent, lines.len() - 1));
    }
    lines
}

fn flat(text: &str) -> Vec<Line> {
    text.lines()
        .enumerate()
        .map(|(index, line)| (index, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
        .map(|(index, line)| Line {
            no: index + 1,
            span: 1,
            depth: 0,
            parent: None,
            text: line.to_string(),
        })
        .collect()
}

/// Split `/ip service set telnet disabled=yes` into its menu and its command.
/// A line that is only a menu has an empty command.
fn split_menu(line: &str) -> (&str, &str) {
    let plain = |word: &str| {
        !word.is_empty()
            && word
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    let mut end = 0;
    let mut offset = 0;
    for word in line.split(' ') {
        let start = offset;
        offset += word.len() + 1;
        if word.is_empty() {
            continue;
        }
        let belongs = if start == 0 {
            word.strip_prefix('/').is_some_and(plain)
        } else {
            plain(word) && !ROUTEROS_VERBS.contains(&word)
        };
        if !belongs {
            break;
        }
        end = start + word.len();
    }
    (&line[..end], line[end..].trim_start())
}

fn routeros(text: &str) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    // Index of the menu the following commands belong to.
    let mut menu: Option<usize> = None;
    let mut physical = text.lines().enumerate();
    while let Some((index, raw)) = physical.next() {
        let mut content = raw.trim().to_string();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        let no = index + 1;
        let mut span = 1;
        // `/export` wraps a long command with a trailing backslash.
        while content.ends_with('\\') {
            let Some((_, next)) = physical.next() else {
                break;
            };
            content.pop();
            // A break after a blank is between two words; anywhere else it cuts a value in two.
            let between_words = content.ends_with(char::is_whitespace);
            content.truncate(content.trim_end().len());
            if between_words {
                content.push(' ');
            }
            content.push_str(next.trim());
            span += 1;
        }

        if !content.starts_with('/') {
            lines.push(Line {
                no,
                span,
                depth: usize::from(menu.is_some()),
                parent: menu,
                text: content,
            });
            continue;
        }
        let (path, command) = split_menu(&content);
        if path.is_empty() {
            // Not a menu path we recognise: keep the line whole and attach nothing to it.
            menu = None;
            lines.push(Line {
                no,
                span,
                depth: 0,
                parent: None,
                text: content,
            });
            continue;
        }
        // Consecutive terse lines of one menu share its header.
        let header = match menu.filter(|at| !command.is_empty() && lines[*at].text == path) {
            Some(at) => at,
            None => {
                lines.push(Line {
                    no,
                    span: if command.is_empty() { span } else { 1 },
                    depth: 0,
                    parent: None,
                    text: path.to_string(),
                });
                lines.len() - 1
            }
        };
        menu = Some(header);
        if !command.is_empty() {
            lines.push(Line {
                no,
                span,
                depth: 1,
                parent: Some(header),
                text: command.to_string(),
            });
        }
    }
    lines
}

/// Collapse runs of blanks, line by line: the form in which lines are compared
/// when the spacing a person typed must not matter.
pub fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.lines().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        for (position, word) in line.split_whitespace().enumerate() {
            if position > 0 {
                out.push(' ');
            }
            out.push_str(word);
        }
    }
    out
}

/// The configuration in the form in which two of them compare: comments and
/// display noise dropped, one space of indentation per level, RouterOS as one
/// complete command per line.
pub fn canonical(text: &str, syntax: Syntax) -> String {
    let mut out = String::new();
    for line in render_lines(&Outline::parse(text, syntax).tree()) {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// A RouterOS configuration with one complete command per line (the form
/// `/export terse` prints and the remediation engine expects), whichever way
/// it was exported.
pub fn routeros_terse(text: &str) -> String {
    let outline = Outline::parse(text, Syntax::Routeros);
    let mut out = String::new();
    for statement in (0..outline.lines.len()).filter_map(|index| outline.statement(index)) {
        out.push_str(&statement);
        out.push('\n');
    }
    out
}

/// The nodes of `wanted` that `have` lacks. A block that exists but lacks some
/// of its lines is returned with only the missing lines inside.
pub fn missing(wanted: &[Node], have: &[Node]) -> Vec<Node> {
    wanted
        .iter()
        .filter_map(
            |node| match have.iter().find(|other| other.text == node.text) {
                None => Some(node.clone()),
                Some(found) => {
                    let children = missing(&node.children, &found.children);
                    (!children.is_empty()).then(|| Node {
                        text: node.text.clone(),
                        children,
                    })
                }
            },
        )
        .collect()
}

/// `have` with everything it lacks from `wanted` added: new lines at the end
/// of the block they belong to, new blocks at the end.
pub fn merged(have: &[Node], wanted: &[Node]) -> Vec<Node> {
    let mut result = have.to_vec();
    for node in wanted {
        match result.iter_mut().find(|other| other.text == node.text) {
            Some(found) => found.children = merged(&found.children, &node.children),
            None => result.push(node.clone()),
        }
    }
    result
}

/// The nodes of `wanted` that `have` contains, with only their present lines inside.
pub fn present(wanted: &[Node], have: &[Node]) -> Vec<Node> {
    wanted
        .iter()
        .filter_map(|node| {
            let found = have.iter().find(|other| other.text == node.text)?;
            Some(Node {
                text: node.text.clone(),
                children: present(&node.children, &found.children),
            })
        })
        .collect()
}

/// Indented text of a tree, one space per level, as lines.
pub fn render_lines(nodes: &[Node]) -> Vec<String> {
    fn walk(nodes: &[Node], depth: usize, out: &mut Vec<String>) {
        for node in nodes {
            for (index, line) in node.text.lines().enumerate() {
                // The body of a banner keeps its own layout.
                let indent = if index == 0 { depth } else { 0 };
                out.push(format!("{}{line}", " ".repeat(indent)));
            }
            walk(&node.children, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(nodes, 0, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const IOS: &str = "\
!
version 15.2
hostname SW1
!
banner motd ^C
 interface is not a command here
Authorised users only
^C
!
interface GigabitEthernet0/1
 description uplink
 switchport mode trunk
!
router bgp 65001
 neighbor 10.0.0.2 remote-as 65002
 address-family ipv4
  network 10.1.0.0 mask 255.255.0.0
 exit-address-family
!
ntp server 192.0.2.1
end
";

    fn texts(outline: &Outline) -> Vec<(usize, usize, &str)> {
        outline
            .lines
            .iter()
            .map(|line| (line.no, line.depth, line.text.as_str()))
            .collect()
    }

    #[test]
    fn ios_lines_keep_their_numbers_depth_and_parents() {
        let outline = Outline::parse(IOS, Syntax::Ios);
        assert_eq!(
            texts(&outline),
            vec![
                (2, 0, "version 15.2"),
                (3, 0, "hostname SW1"),
                (
                    5,
                    0,
                    "banner motd ^C\n interface is not a command here\nAuthorised users only\n^C"
                ),
                (10, 0, "interface GigabitEthernet0/1"),
                (11, 1, "description uplink"),
                (12, 1, "switchport mode trunk"),
                (14, 0, "router bgp 65001"),
                (15, 1, "neighbor 10.0.0.2 remote-as 65002"),
                (16, 1, "address-family ipv4"),
                (17, 2, "network 10.1.0.0 mask 255.255.0.0"),
                (18, 1, "exit-address-family"),
                (20, 0, "ntp server 192.0.2.1"),
            ]
        );
        assert_eq!(outline.lines[2].span, 4);
        assert_eq!(outline.path(9), vec!["router bgp 65001", "address-family ipv4"]);
        assert_eq!(outline.descendants(6), 7..11);
        assert!(outline.has_children(8) && !outline.has_children(10));
    }

    #[test]
    fn physical_lines_map_to_their_sections() {
        let outline = Outline::parse(IOS, Syntax::Ios);
        // A comment and a global line sit in no section.
        assert_eq!(outline.enclosing(1), Vec::<String>::new());
        assert_eq!(outline.enclosing(20), Vec::<String>::new());
        assert_eq!(outline.enclosing(11), vec!["interface GigabitEthernet0/1"]);
        assert_eq!(
            outline.enclosing(17),
            vec!["router bgp 65001", "address-family ipv4"]
        );
        // The body of a banner belongs to the banner, its first line does not.
        assert_eq!(outline.enclosing(7), vec!["banner motd ^C"]);
        assert_eq!(outline.enclosing(5), Vec::<String>::new());
        // A change to a line that opens a block is filed under that block.
        assert_eq!(outline.section_of(10), vec!["interface GigabitEthernet0/1"]);
        assert_eq!(
            outline.section_of(16),
            vec!["router bgp 65001", "address-family ipv4"]
        );
        assert_eq!(outline.section_of(20), Vec::<String>::new());
        assert_eq!(outline.index_at(13), None);
        assert_eq!(outline.index_at(99), None);
    }

    #[test]
    fn the_tree_agrees_with_the_hierarchical_engine() {
        let ours = Outline::parse(IOS, Syntax::Ios).tree();
        let theirs = nocturne_drivers::hier::ConfigTree::parse(IOS, Syntax::Ios).roots;
        assert_eq!(ours, theirs);
        let vrp = "#\nsysname SW-A\n#\ninterface GigabitEthernet0/0/1\n description x\n#\nreturn\n";
        assert_eq!(
            Outline::parse(vrp, Syntax::Vrp).tree(),
            nocturne_drivers::hier::ConfigTree::parse(vrp, Syntax::Vrp).roots
        );
    }

    #[test]
    fn display_indents_one_space_per_level_whatever_the_device_used() {
        let nxos = "interface Ethernet1/1\n    description wide\n    vrf member X\n";
        let outline = Outline::parse(nxos, Syntax::Ios);
        assert_eq!(outline.display(1).as_deref(), Some(" description wide"));
        assert_eq!(outline.statement(1), None);
        assert_eq!(
            outline.statement(0).as_deref(),
            Some("interface Ethernet1/1")
        );
    }

    const ROS_EXPORT: &str = "\
# nov/13/2026 12:00:00 by RouterOS 6.49.10
#
/interface bridge
add name=bridge1
/ip service
set telnet disabled=yes
set ftp disabled=yes
/ip firewall filter
add action=accept chain=input comment=\"accept established\" \\
    connection-state=established,related
add action=drop chain=input in-interface-list=WAN
/system script
add name=hello source=\"/log info \\\"one\\\"\\r\\
    \\n/log info two\"
";

    const ROS_TERSE: &str = "\
/interface bridge add name=bridge1
/ip service set telnet disabled=yes
/ip service set ftp disabled=yes
/ip firewall filter add action=accept chain=input comment=\"accept established\" connection-state=established,related
/ip firewall filter add action=drop chain=input in-interface-list=WAN
/system script add name=hello source=\"/log info \\\"one\\\"\\r\\n/log info two\"
";

    #[test]
    fn routeros_export_and_terse_export_read_the_same() {
        let export = Outline::parse(ROS_EXPORT, Syntax::Routeros);
        let terse = Outline::parse(ROS_TERSE, Syntax::Routeros);
        let statements = |outline: &Outline| -> Vec<String> {
            (0..outline.lines.len())
                .filter_map(|index| outline.statement(index))
                .collect()
        };
        let expected: Vec<String> = ROS_TERSE.lines().map(str::to_string).collect();
        assert_eq!(statements(&export), expected);
        assert_eq!(statements(&terse), expected);
        assert_eq!(routeros_terse(ROS_EXPORT), ROS_TERSE);

        // Menus are sections in both forms, with the commands inside.
        for outline in [&export, &terse] {
            let sections = outline.sections().unwrap();
            let service = sections
                .iter()
                .find(|section| section.header == "/ip service")
                .unwrap();
            let members: Vec<&str> = service
                .members
                .iter()
                .map(|index| outline.lines[*index].text.as_str())
                .collect();
            assert_eq!(members, vec!["set telnet disabled=yes", "set ftp disabled=yes"]);
            assert_eq!(sections.len(), 4);
        }

        // Line numbers are those of the text as stored; a wrapped command covers both lines.
        assert_eq!(export.enclosing(6), vec!["/ip service"]);
        assert_eq!(export.enclosing(10), vec!["/ip firewall filter"]);
        let wrapped = export.index_at(10).unwrap();
        assert_eq!((export.lines[wrapped].no, export.lines[wrapped].span), (9, 2));
        assert_eq!(terse.enclosing(3), vec!["/ip service"]);
        assert_eq!(terse.section_of(3), vec!["/ip service"]);
        assert_eq!(export.section_of(5), vec!["/ip service"]);
        // Menu lines are not statements, and patterns never see them alone.
        assert_eq!(export.display(2), None);
        assert_eq!(
            export.display(3).as_deref(),
            Some("/ip service set telnet disabled=yes")
        );
    }

    #[test]
    fn routeros_menu_splitting() {
        assert_eq!(split_menu("/ip service"), ("/ip service", ""));
        assert_eq!(
            split_menu("/ip service set api-ssl disabled=yes"),
            ("/ip service", "set api-ssl disabled=yes")
        );
        assert_eq!(
            split_menu("/interface ethernet set [ find default-name=ether1 ] name=wan"),
            ("/interface ethernet", "set [ find default-name=ether1 ] name=wan")
        );
        assert_eq!(
            split_menu("/ip dhcp-server option sets add name=x"),
            ("/ip dhcp-server option sets", "add name=x")
        );
        assert_eq!(split_menu("/ip/service set x=y"), ("", "/ip/service set x=y"));
    }

    #[test]
    fn flat_grammars_have_lines_but_no_sections() {
        let outline = Outline::parse(
            "# comment\nset system host-name r1\n\nset system ntp server 192.0.2.1\n",
            Syntax::Set,
        );
        assert_eq!(
            texts(&outline),
            vec![
                (2, 0, "set system host-name r1"),
                (4, 0, "set system ntp server 192.0.2.1")
            ]
        );
        assert!(outline.sections().is_none());
        assert_eq!(outline.enclosing(4), Vec::<String>::new());
    }

    #[test]
    fn tree_helpers_find_merge_and_render_what_is_missing() {
        let have = Outline::parse(IOS, Syntax::Ios).tree();
        let wanted = Outline::parse(
            "ntp server 192.0.2.1\nntp server 192.0.2.2\ninterface GigabitEthernet0/1\n description uplink\n spanning-tree portfast\nline vty 0 4\n transport input ssh\n",
            Syntax::Ios,
        )
        .tree();
        let lacking = missing(&wanted, &have);
        assert_eq!(
            render_lines(&lacking),
            vec![
                "ntp server 192.0.2.2",
                "interface GigabitEthernet0/1",
                " spanning-tree portfast",
                "line vty 0 4",
                " transport input ssh",
            ]
        );
        assert_eq!(
            render_lines(&present(&wanted, &have)),
            vec![
                "ntp server 192.0.2.1",
                "interface GigabitEthernet0/1",
                " description uplink"
            ]
        );
        let all = merged(&have, &wanted);
        assert!(missing(&wanted, &all).is_empty());
        assert!(missing(&have, &all).is_empty());
        // Nothing the device has is dropped or duplicated, and new lines join their block.
        let interface = all
            .iter()
            .find(|node| node.text == "interface GigabitEthernet0/1")
            .unwrap();
        let children: Vec<&str> = interface
            .children
            .iter()
            .map(|node| node.text.as_str())
            .collect();
        assert_eq!(
            children,
            vec!["description uplink", "switchport mode trunk", "spanning-tree portfast"]
        );
        assert_eq!(all.len(), have.len() + 2);
    }

    #[test]
    fn collapse_ignores_spacing_but_keeps_lines() {
        assert_eq!(collapse("  ntp   server\t192.0.2.1 "), "ntp server 192.0.2.1");
        assert_eq!(collapse("banner motd ^C\n  two   words\n^C"), "banner motd ^C\ntwo words\n^C");
    }
}
