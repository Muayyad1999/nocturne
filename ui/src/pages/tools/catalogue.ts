/**
 * The drawer, worked out from `tools.list` rather than from a list kept here.
 *
 * `tools.list` is the backend's own catalogue (`crates/nocturne-toolbox/
 * src/catalogue.rs`), and a Rust test keeps it exactly in step with the
 * methods the crate registers. So this screen asks for it and builds itself
 * from the answer: a tool added to the crate appears here without a line of
 * TypeScript, and a tool a build does not have never appears at all.
 *
 * Two things the catalogue does not say, and this module decides:
 *
 * 1. **What kind of thing a tool is** ({@link ToolKind}). The catalogue's
 *    `offline` and `job` flags nearly say it, but not quite: generating a
 *    password is offline arithmetic that must *not* run on every keystroke,
 *    and a server is a probe-shaped thing that keeps running after the screen
 *    is closed. The kind decides what the panel looks like and how it behaves,
 *    and the three families the screen must never let look alike —
 *    calculators, probes, and things that outlive the screen — are exactly
 *    the kinds below.
 * 2. **Which entries are one panel.** `tftp.start`, `tftp.stop` and
 *    `tftp.status` are three methods and one thing on screen; so are the three
 *    `probe.history*` methods, the four `probe.syslog.*` and
 *    `tools.reference` with `tools.reference.tables`. {@link FOLDED} says
 *    which method belongs to which panel, and a panel carries the methods it
 *    folded in ({@link Panel.uses}) so the screen can show them.
 *
 * An entry that is neither a panel nor folded into one is a tool the backend
 * has and this screen has no form for. That is shown as exactly that —
 * `unformed` — because the alternative is a button that does nothing, and the
 * alternative to *that* is a list that quietly hides half the drawer.
 */

import type { Tool, ToolGroup } from '@/api/types/toolbox';
import { fuzzyFilter } from '@/lib/fuzzy';

/**
 * What a tool is, which is what its panel is. The screen's whole shape hangs
 * off this: the first three never look alike.
 */
export type ToolKind =
  /** Arithmetic. Instant, nothing leaves the machine, answers as you type. */
  | 'calculator'
  /** Arithmetic too, but it makes a new secret every time it is called: on request only. */
  | 'generator'
  /** Puts packets on the wire from here and answers in the reply. */
  | 'probe'
  /** Puts packets on the wire from here as a job, streaming what it finds. */
  | 'job'
  /** Runs on a device, through the read-only gate. */
  | 'device'
  /** Binds a socket here and keeps running when the screen is closed. */
  | 'server'
  /** The record of what was run. */
  | 'history'
  /** In this build, but this screen has no form for it. */
  | 'unformed';

/**
 * The tools whose kind the catalogue's flags would get wrong.
 *
 * `tools.password`, `tools.secret.encode` and `tools.keys.generate` take a
 * secret or produce one. Answering as you type would send every prefix of a
 * password the user is choosing to the backend and would throw away the
 * previous answer on every keystroke, so they are asked once, by a button.
 */
const KIND_BY_ID: Record<string, ToolKind> = {
  'tools.password': 'generator',
  'tools.secret.encode': 'generator',
  'tools.keys.generate': 'generator',
  'tftp.start': 'server',
  'probe.syslog.start': 'server',
  'probe.history': 'history',
};

/** Which panel a method belongs to, when it is not a panel of its own. */
export const FOLDED: Record<string, string> = {
  'tools.reference.tables': 'tools.reference',
  'probe.history.get': 'probe.history',
  'probe.history.delete': 'probe.history',
  'tftp.stop': 'tftp.start',
  'tftp.status': 'tftp.start',
  'probe.syslog.stop': 'probe.syslog.start',
  'probe.syslog.status': 'probe.syslog.start',
  'probe.syslog.messages': 'probe.syslog.start',
};

/**
 * Every id this screen has a form for. The source of truth for "the screen
 * covers the whole drawer": `specs.ts` is checked against it by a test, and
 * anything in the live catalogue that is not here renders as `unformed`.
 */
export const FORMED = new Set<string>([
  'tools.subnet',
  'tools.subnet.split',
  'tools.vlsm',
  'tools.supernet',
  'tools.contains',
  'tools.overlaps',
  'tools.ip.range',
  'tools.ip.convert',
  'tools.wildcard',
  'tools.wildcard.acl',
  'tools.mac',
  'tools.oui',
  'tools.oid',
  'tools.interface',
  'tools.secret.decode',
  'tools.secret.encode',
  'tools.secret.identify',
  'tools.password',
  'tools.keys.generate',
  'tools.convert',
  'tools.transfer_time',
  'tools.mtu',
  'tools.qos.marking',
  'tools.qos.voice',
  'tools.reference',
  'tools.regex',
  'tools.diff',
  'tools.hash',
  'tools.base64',
  'probe.ping',
  'probe.traceroute',
  'probe.ports',
  'probe.sweep',
  'probe.dns',
  'probe.whois',
  'probe.tls',
  'probe.http',
  'probe.ntp',
  'probe.local',
  'probe.wol',
  'probe.device.ping',
  'probe.device.traceroute',
  'probe.history',
  'tftp.start',
  'probe.syslog.start',
]);

export function kindOf(tool: Tool): ToolKind {
  const known = KIND_BY_ID[tool.id];
  if (known) return known;
  if (!FORMED.has(tool.id)) return 'unformed';
  if (tool.group === 'device') return 'device';
  if (tool.group === 'servers') return 'server';
  if (tool.group === 'history') return 'history';
  if (tool.offline) return 'calculator';
  return tool.job ? 'job' : 'probe';
}

/** One thing in the list, which is one panel on the right. */
export interface Panel {
  tool: Tool;
  kind: ToolKind;
  /** The other methods this panel uses, in catalogue order. */
  uses: string[];
}

/**
 * The panels of a catalogue, in its own order, each carrying the methods
 * folded into it. A folded method whose panel is missing from the build is
 * dropped rather than promoted: half a panel is worse than none.
 */
export function panels(catalogue: readonly Tool[]): Panel[] {
  const own = new Map<string, Panel>();
  for (const tool of catalogue) {
    if (FOLDED[tool.id]) continue;
    own.set(tool.id, { tool, kind: kindOf(tool), uses: [] });
  }
  for (const tool of catalogue) {
    const into = FOLDED[tool.id];
    if (into) own.get(into)?.uses.push(tool.id);
  }
  return [...own.values()];
}

/** The group headings, in the order the drawer shows them. */
export const GROUP_LABEL: Record<ToolGroup, string> = {
  addressing: 'Addressing',
  identity: 'Identity',
  secrets: 'Secrets',
  converters: 'Converters',
  qos: 'Quality of service',
  reference: 'Reference',
  text: 'Text',
  probes: 'Probes from here',
  device: 'Probes from a device',
  history: 'History',
  servers: 'Servers',
};

const GROUP_ORDER: ToolGroup[] = [
  'addressing',
  'identity',
  'secrets',
  'converters',
  'qos',
  'reference',
  'text',
  'probes',
  'device',
  'servers',
  'history',
];

/** A group as the list renders it: a heading and the panels under it. */
export interface PanelGroup {
  group: ToolGroup;
  label: string;
  entries: { panel: Panel; indices: number[] }[];
}

/**
 * The list, filtered by `query` and grouped.
 *
 * Titles are matched the way the command palette matches, so `subcalc` finds
 * "Subnet calculator"; the description, the group and the method name are
 * matched as **plain text**, not fuzzily. That second half is deliberate: a
 * subsequence match against forty sentences of prose matches almost anything
 * three letters long — `mss` brought back more than half the drawer — and a
 * filter that never narrows is not a filter. Substring matching over the
 * descriptions still finds the tool by what it does ("mss" → "MTU and MSS",
 * through its own description), and only that.
 *
 * With no query the groups keep their fixed order, because a drawer whose
 * shelves move while you browse cannot be learnt. With one, the group holding
 * the best match comes first: somebody typing `mss` is searching, not
 * browsing, and the answer being third because "Addressing" sorts before
 * "Converters" is the wrong answer.
 */
export function groupPanels(all: readonly Panel[], query: string): PanelGroup[] {
  const text = query.trim().toLowerCase();
  const byTitle = fuzzyFilter(all, query, { text: (entry) => entry.tool.title });
  const found = new Set(byTitle.map((match) => match.item.tool.id));
  const matches: { item: Panel; indices: number[] }[] = [...byTitle];
  if (text) {
    for (const panel of all) {
      if (found.has(panel.tool.id)) continue;
      const haystack = [
        panel.tool.description,
        GROUP_LABEL[panel.tool.group] ?? panel.tool.group,
        panel.tool.id,
      ]
        .join(' ')
        .toLowerCase();
      if (haystack.includes(text)) matches.push({ item: panel, indices: [] });
    }
  }

  const byGroup = new Map<ToolGroup, { panel: Panel; indices: number[] }[]>();
  for (const match of matches) {
    const group = match.item.tool.group;
    const list = byGroup.get(group) ?? [];
    list.push({ panel: match.item, indices: match.indices });
    byGroup.set(group, list);
  }
  const unknownGroups = [...byGroup.keys()].filter((group) => !GROUP_ORDER.includes(group));
  const groups = [...GROUP_ORDER, ...unknownGroups]
    .filter((group) => (byGroup.get(group)?.length ?? 0) > 0)
    .map((group) => ({
      group,
      label: GROUP_LABEL[group] ?? group,
      entries: byGroup.get(group) ?? [],
    }));
  if (!text) return groups;
  // The order the matches came back in is the order of their scores; a group
  // is placed by its best one.
  const rank = new Map(matches.map((match, index) => [match.item.tool.id, index]));
  const best = (entries: { panel: Panel }[]) =>
    Math.min(...entries.map((entry) => rank.get(entry.panel.tool.id) ?? Number.MAX_SAFE_INTEGER));
  return groups.sort((a, b) => best(a.entries) - best(b.entries));
}

/** The panel the route's `:tool` segment names, or the first one in the list. */
export function selectPanel(all: readonly Panel[], route: string | undefined): Panel | undefined {
  if (route) {
    const exact = all.find((entry) => entry.tool.id === route);
    if (exact) return exact;
  }
  return all[0];
}

/** What each kind promises, in one line, above the panel's form. */
export const KIND_PROMISE: Record<ToolKind, string> = {
  calculator: 'Arithmetic on this machine. It answers as you type and nothing leaves the computer.',
  generator: 'Arithmetic on this machine. It answers when you ask, and the answer is kept nowhere.',
  probe: 'This leaves the machine the engine runs on and is recorded in the probe history.',
  job: 'This leaves the machine the engine runs on, streams what it finds, and can be stopped.',
  device: 'The device runs this, not you: its own command, through the read-only gate.',
  server: 'This binds a socket and keeps running after you leave this screen, until you stop it.',
  history:
    'The record is the engine’s: it is written before a probe starts, so one still running is in the list too.',
  unformed: 'This build has this tool, but this screen has no form for it yet.',
};

/** The one-word badge on the panel header and the list row. */
export const KIND_BADGE: Record<ToolKind, string> = {
  calculator: 'Offline',
  generator: 'Offline',
  probe: 'Probe',
  job: 'Probe',
  device: 'On a device',
  server: 'Server',
  history: 'Record',
  unformed: 'No form',
};

/**
 * Which of the three families a kind belongs to. Only this decides the
 * panel's colour and frame, so two tools of one family can never drift apart
 * and two of different families can never converge.
 */
export type ToolFamily = 'offline' | 'network' | 'standing';

export const FAMILY: Record<ToolKind, ToolFamily> = {
  calculator: 'offline',
  generator: 'offline',
  probe: 'network',
  job: 'network',
  device: 'standing',
  server: 'standing',
  history: 'standing',
  unformed: 'offline',
};
