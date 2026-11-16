/**
 * Everything the drawing needs to know, as plain functions over what
 * `topology.get` answered with.
 *
 * Nothing here calls the backend and nothing here invents anything: a link's
 * style and the sentence next to it are derived from `kind` and `confidence`
 * as the backend set them, a node's position is the backend's `x`/`y`, and the
 * only geometry computed in the browser is where a line leaves a card and how
 * far a line bows to get round one. That matters because the backend
 * guarantees the cards do not overlap (its layout works in 190 × 64 world
 * units per node, `crates/nocturne-topology/src/layout.rs`), so the drawn card
 * is kept inside that box and no position is ever computed here.
 */

import {
  Cloud,
  CircleHelp,
  Monitor,
  Network,
  Router,
  Server,
  Shield,
  Wifi,
} from 'lucide-react';
import type {
  Confidence,
  Graph,
  Group,
  GroupKind,
  Link,
  Node,
  NodeKind,
  Position,
} from '@/api/types/topology';
import type { LucideIcon } from '@/design/Icon';
import type { Bounds } from '@/design/Viewport';

/**
 * The drawn size of a node, in world units. The backend's layout separates
 * nodes by `NODE_WIDTH` 190 × `NODE_HEIGHT` 64 and leaves a 48px gap, so a
 * card this size can never cover another one.
 */
export const NODE_W = 184;
export const NODE_H = 52;
/** Room under a card for the second line (address, or what it is). */
export const NODE_LABEL_H = 18;

export interface Point {
  x: number;
  y: number;
}

// ------------------------------------------------------------------ node kinds

const KIND_ICON: Record<NodeKind, LucideIcon> = {
  router: Router,
  switch: Network,
  l3_switch: Network,
  firewall: Shield,
  access_point: Wifi,
  server: Server,
  host: Monitor,
  cloud: Cloud,
  unknown: CircleHelp,
};

/** The words the backend's own SVG legend uses, so the two drawings agree. */
const KIND_LABEL: Record<NodeKind, string> = {
  router: 'Router',
  switch: 'Switch',
  l3_switch: 'Layer-3 switch',
  firewall: 'Firewall',
  access_point: 'Access point',
  server: 'Server',
  host: 'Host',
  cloud: 'External',
  unknown: 'Unknown',
};

export function nodeIcon(kind: NodeKind): LucideIcon {
  return KIND_ICON[kind] ?? CircleHelp;
}

export function nodeKindLabel(kind: NodeKind): string {
  return KIND_LABEL[kind] ?? kind;
}

/** The kinds in the order a legend should list them, most important first. */
export const KIND_ORDER: readonly NodeKind[] = [
  'router',
  'firewall',
  'l3_switch',
  'switch',
  'access_point',
  'server',
  'host',
  'cloud',
  'unknown',
];

// -------------------------------------------------------------------- evidence

/** Where a link came from — not how sure anyone is that it exists. */
export type LinkSource =
  /** A neighbour protocol: LLDP, CDP, MNDP. */
  | 'reported'
  /** Worked out from addresses or MAC tables. */
  | 'derived'
  /** A person drew it. */
  | 'drawn'
  /** The lab owns its own wiring. */
  | 'lab';

export interface Evidence {
  source: LinkSource;
  confidence: Confidence;
  /** Two or three words, drawn on the line itself. Empty = nothing to add. */
  chip: string;
  /** What this line claims, in one sentence. */
  claim: string;
  /** What the claim rests on: `LLDP and CDP`, `a MAC table`, … */
  basis: string;
  /**
   * What `confirmed` means for this link when it does not mean "both devices
   * said so" — a person is sure, or the lab owns the wiring. Empty otherwise.
   */
  confidenceNote: string;
}

function listed(words: readonly string[]): string {
  if (words.length === 0) return '';
  if (words.length === 1) return words[0] ?? '';
  return `${words.slice(0, -1).join(', ')} and ${words[words.length - 1]}`;
}

function basisOf(link: Pick<Link, 'kind' | 'protocols' | 'label'>): string {
  const protocols = (link.protocols ?? []).map((protocol) => protocol.toUpperCase());
  if (protocols.length > 0) return listed(protocols);
  switch (link.kind) {
    case 'lldp':
      return 'LLDP';
    case 'cdp':
      return 'CDP';
    case 'mndp':
      return 'MNDP';
    case 'subnet':
      return 'two interfaces in one subnet';
    case 'mac':
      return 'a MAC-table entry on an access port';
    case 'manual':
      return 'a person';
    case 'lab':
      return 'the lab that owns these devices';
    default:
      return 'an unnamed source';
  }
}

export function linkSource(kind: Link['kind']): LinkSource {
  switch (kind) {
    case 'manual':
      return 'drawn';
    case 'lab':
      return 'lab';
    case 'subnet':
    case 'mac':
      return 'derived';
    default:
      return 'reported';
  }
}

/**
 * How this link is known. The three confidences are three different claims
 * about reality, so each gets its own words; `one_sided` deliberately does not
 * name the end that reported it, because the backend sorts a link's ends by
 * name and does not say which one spoke.
 */
export function evidence(link: Pick<Link, 'kind' | 'confidence' | 'protocols' | 'label'>): Evidence {
  const source = linkSource(link.kind);
  const basis = basisOf(link);

  if (source === 'drawn') {
    return {
      source,
      confidence: link.confidence,
      chip: 'drawn by hand',
      claim: 'A person drew this link. No device reported it.',
      basis,
      // The backend calls a drawn link `confirmed`, because the person who
      // drew it is sure. Said on its own, next to links two devices really
      // did report, that word would be read as the devices confirming it.
      confidenceNote: 'the person who drew it, not the devices',
    };
  }
  if (source === 'lab') {
    return {
      source,
      confidence: link.confidence,
      chip: 'lab wiring',
      claim: 'The lab owns this wiring, so both ends of it are known.',
      basis,
      confidenceNote: 'the lab, which owns its own wiring',
    };
  }
  switch (link.confidence) {
    case 'confirmed':
      return {
        source,
        confidence: 'confirmed',
        chip: '',
        claim: `Both ends reported this link (${basis}).`,
        basis,
        confidenceNote: '',
      };
    case 'one_sided':
      return {
        source,
        confidence: 'one_sided',
        chip: 'one end only',
        claim:
          `Only one end reported this link (${basis}). The other end has not: ` +
          'it is unmanaged, silent, or has never been collected from.',
        basis,
        confidenceNote: '',
      };
    case 'inferred':
      return {
        source,
        confidence: 'inferred',
        chip: 'inferred',
        claim: `Derived from ${basis}. Neither end reported the other as a neighbour.`,
        basis,
        confidenceNote: '',
      };
  }
}

/** The legend's rows, in the order they are explained. */
export const CONFIDENCE_LEGEND: readonly { confidence: Confidence; title: string; text: string }[] = [
  {
    confidence: 'confirmed',
    title: 'Confirmed',
    text: 'both ends reported the link',
  },
  {
    confidence: 'one_sided',
    title: 'One-sided',
    text: 'only one end reported it',
  },
  {
    confidence: 'inferred',
    title: 'Inferred',
    text: 'derived from addresses or MAC tables',
  },
];

export const SOURCE_LEGEND: readonly { source: LinkSource; title: string; text: string }[] = [
  { source: 'reported', title: 'Neighbour protocol', text: 'LLDP, CDP or MNDP said so' },
  { source: 'derived', title: 'Worked out', text: 'from a subnet or a MAC table' },
  { source: 'drawn', title: 'Drawn by a person', text: 'what discovery cannot see' },
  { source: 'lab', title: 'Lab wiring', text: 'the simulator owns it' },
];

// -------------------------------------------------------------------- geometry

/** The centre of a node, which is what the backend's `x`/`y` already is. */
export function centre(node: Pick<Node, 'x' | 'y'>): Point {
  return { x: node.x, y: node.y };
}

/**
 * Where a line from the centre of a card towards `to` leaves the card, so a
 * link starts at the edge of the box and not under the name.
 */
export function edgeExit(from: Point, to: Point): Point {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  if (dx === 0 && dy === 0) return { x: from.x + NODE_W / 2, y: from.y };
  const halfW = NODE_W / 2 + 2;
  const halfH = NODE_H / 2 + 2;
  const scale = Math.min(
    dx === 0 ? Number.POSITIVE_INFINITY : halfW / Math.abs(dx),
    dy === 0 ? Number.POSITIVE_INFINITY : halfH / Math.abs(dy),
  );
  return { x: from.x + dx * scale, y: from.y + dy * scale };
}

export interface LinkGeometry {
  a: Point;
  b: Point;
  /** The point on the line where a label or a chip belongs. */
  mid: Point;
  /** An SVG path: a straight line, a curve, or a loop for a stack cable. */
  path: string;
  /** How far the curve is pushed off the straight line (0 = straight). */
  bow: number;
}

/** A cable from a device to itself (a stack cable) is drawn as a loop. */
export function isSelfLink(link: Pick<Link, 'a' | 'b'>): boolean {
  return link.a.node === link.b.node;
}

function loop(at: Point, index: number): LinkGeometry {
  const radius = 26 + index * 10;
  const start = { x: at.x + NODE_W / 2, y: at.y - NODE_H / 4 };
  const end = { x: at.x + NODE_W / 2, y: at.y + NODE_H / 4 };
  return {
    a: start,
    b: end,
    mid: { x: start.x + radius * 1.4, y: at.y },
    path: `M ${start.x} ${start.y} C ${start.x + radius * 2} ${start.y - radius} ${
      start.x + radius * 2
    } ${end.y + radius} ${end.x} ${end.y}`,
    bow: radius,
  };
}

/**
 * The line between two cards. `bow` pushes it off the straight line — used to
 * fan parallel links apart and to get round a card that is in the way.
 */
export function linkGeometry(
  aCentre: Point,
  bCentre: Point,
  bow = 0,
  selfIndex = 0,
): LinkGeometry {
  if (aCentre.x === bCentre.x && aCentre.y === bCentre.y) return loop(aCentre, selfIndex);
  const a = edgeExit(aCentre, bCentre);
  const b = edgeExit(bCentre, aCentre);
  const midpoint = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
  if (bow === 0) {
    return { a, b, mid: midpoint, path: `M ${a.x} ${a.y} L ${b.x} ${b.y}`, bow };
  }
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const length = Math.hypot(dx, dy) || 1;
  // The normal of the segment, so the curve leaves it sideways.
  const nx = -dy / length;
  const ny = dx / length;
  const control = { x: midpoint.x + nx * bow * 2, y: midpoint.y + ny * bow * 2 };
  return {
    a,
    b,
    // A quadratic curve passes through the midpoint of its control polygon.
    mid: { x: midpoint.x + nx * bow, y: midpoint.y + ny * bow },
    path: `M ${a.x} ${a.y} Q ${control.x} ${control.y} ${b.x} ${b.y}`,
    bow,
  };
}

/**
 * How far a load ribbon sits off the wire. Two of them, one per direction,
 * either side of the line, close enough to read as belonging to it.
 */
export const RIBBON_OFFSET = 4.5;

/**
 * The line a direction's load bar is drawn along: the wire, moved sideways.
 *
 * It is traced *from* `fromCentre`, so a bar that grows from the start of the
 * path grows from the end that is sending. Calling it with the ends swapped
 * therefore gives the other direction — and lands it on the other side of the
 * wire by itself, because the normal flips with the direction. The bow has to
 * be negated to swap with it, or the mirrored curve would bend the wrong way
 * and cross the line it belongs to.
 */
export function ribbonPath(
  fromCentre: Point,
  toCentre: Point,
  bow: number,
  offset: number,
  selfIndex = 0,
): string {
  const geometry = linkGeometry(fromCentre, toCentre, bow, selfIndex);
  // A cable from a device to itself is a loop; one bar along it is the most
  // that can be read, and it is drawn on the loop itself.
  if (fromCentre.x === toCentre.x && fromCentre.y === toCentre.y) return geometry.path;

  const dx = geometry.b.x - geometry.a.x;
  const dy = geometry.b.y - geometry.a.y;
  const length = Math.hypot(dx, dy) || 1;
  const nx = (-dy / length) * offset;
  const ny = (dx / length) * offset;
  const a = { x: geometry.a.x + nx, y: geometry.a.y + ny };
  const b = { x: geometry.b.x + nx, y: geometry.b.y + ny };
  if (bow === 0) return `M ${a.x} ${a.y} L ${b.x} ${b.y}`;

  const midpoint = { x: (geometry.a.x + geometry.b.x) / 2, y: (geometry.a.y + geometry.b.y) / 2 };
  const control = {
    x: midpoint.x + (-dy / length) * (bow * 2 + offset),
    y: midpoint.y + (dx / length) * (bow * 2 + offset),
  };
  return `M ${a.x} ${a.y} Q ${control.x} ${control.y} ${b.x} ${b.y}`;
}

/**
 * Where a direction's number goes: beside the middle of its own bar, on that
 * direction's own side of the line.
 *
 * The first attempt put it a third of the way along from the end that is
 * sending, which reads well on a drawing of two devices and badly on a real
 * one: the port names are near the ends too (and on the same side), so
 * `no rate` was printed through `Gi0/0`, and every link into a distribution
 * switch put its number in the same place as the others. Midpoints are as
 * far apart as the links are. Which direction a number belongs to is carried
 * by the side it is on — the same side as its own bar, which grows from the
 * end that is sending.
 */
export function labelBeside(from: Point, to: Point, mid: Point, offset: number): Point {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const length = Math.hypot(dx, dy) || 1;
  return { x: mid.x + (-dy / length) * offset, y: mid.y + (dx / length) * offset };
}

/** How far apart parallel links between the same pair of nodes are fanned. */
const PARALLEL_GAP = 20;

/**
 * How much each link has to bow. Two things earn a bow: several links between
 * the same pair of nodes (otherwise they are one line and the drawing hides
 * how many there are), and a line that would pass straight through a card it
 * is not connected to.
 */
export function bows(links: readonly Link[], positions: ReadonlyMap<string, Point>): Map<string, number> {
  const byPair = new Map<string, Link[]>();
  for (const link of links) {
    const pair = [link.a.node, link.b.node].sort().join('\u0000');
    const group = byPair.get(pair);
    if (group) group.push(link);
    else byPair.set(pair, [link]);
  }

  const result = new Map<string, number>();
  for (const group of byPair.values()) {
    // Sorted by id so the same graph always fans the same way.
    const ordered = [...group].sort((one, other) => (one.id < other.id ? -1 : 1));
    for (const [index, link] of ordered.entries()) {
      const a = positions.get(link.a.node);
      const b = positions.get(link.b.node);
      if (!a || !b) continue;
      let bow =
        ordered.length > 1 ? (index - (ordered.length - 1) / 2) * PARALLEL_GAP : 0;
      if (bow === 0 && !isSelfLink(link)) bow = detour(a, b, link, positions);
      result.set(link.id, bow);
    }
  }
  return result;
}

/**
 * A line that runs through a card it has nothing to do with says that card is
 * on the path, which is a lie. When one is in the way the line bows round it,
 * on the side the card is not on.
 */
function detour(
  a: Point,
  b: Point,
  link: Link,
  positions: ReadonlyMap<string, Point>,
): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const length = Math.hypot(dx, dy);
  if (length === 0) return 0;
  let worst = 0;
  for (const [id, at] of positions) {
    if (id === link.a.node || id === link.b.node) continue;
    // How far along the segment this card is, and how far off it.
    const along = ((at.x - a.x) * dx + (at.y - a.y) * dy) / (length * length);
    if (along <= 0.05 || along >= 0.95) continue;
    const side = ((at.x - a.x) * -dy + (at.y - a.y) * dx) / length;
    const nearest = { x: a.x + dx * along, y: a.y + dy * along };
    const inside =
      Math.abs(at.x - nearest.x) < NODE_W / 2 + 8 && Math.abs(at.y - nearest.y) < NODE_H / 2 + 8;
    if (!inside) continue;
    // Away from the card, by enough to clear it.
    const push = (side >= 0 ? -1 : 1) * (NODE_H / 2 + 34);
    if (Math.abs(push) > Math.abs(worst)) worst = push;
  }
  return worst;
}

/**
 * An interface name as a port label: the short form every network engineer
 * writes. Full names (`GigabitEthernet1/0/11`) are four times as wide as the
 * gap between two cards, so a drawing labelled with them is a drawing of
 * overlapping text — which is what the backend's own first attempt looked
 * like. The long form stays in the panels, where there is room for it.
 */
const SHORT_FORMS: readonly (readonly [string, string])[] = [
  ['TenGigabitEthernet', 'Te'],
  ['TwentyFiveGigE', 'Twe'],
  ['FortyGigabitEthernet', 'Fo'],
  ['HundredGigE', 'Hu'],
  ['GigabitEthernet', 'Gi'],
  ['FastEthernet', 'Fa'],
  ['Port-channel', 'Po'],
  ['Bundle-Ether', 'BE'],
  ['TenGigE', 'Te'],
  ['Ethernet', 'Et'],
  ['Loopback', 'Lo'],
  ['Vlan', 'Vl'],
  ['Tunnel', 'Tu'],
  ['Serial', 'Se'],
  ['Management', 'Ma'],
];

/**
 * The widest a name can be on a card, in characters. A card is 184 units
 * wide, 38 of them the icon and 10 the right margin, and Inter at 12.5px
 * averages about 6.9 units a character — but the name starts 38 units in,
 * after the icon, and a badge sits on the right, so the room is nearer 110
 * units than 184, and a mixed-case name runs wider than the average. At 19
 * the cut was still being clipped by the card, which put `Campus/Guest…aptop`
 * on two different laptops again. Checked by looking at the drawing.
 */
const NAME_LIMIT = 14;

/**
 * A node's name as it fits on its card. A card clips what overflows, which
 * silently turned `Campus/Guest-Laptop1` and `Campus/Guest-Laptop2` into two
 * cards both reading `Campus/Guest-Laptop` — two different devices with the
 * same name on them. Cutting it here puts an ellipsis where the cut is, so a
 * shortened name cannot be read as the whole one.
 */
export function fitName(name: string): string {
  if (name.length <= NAME_LIMIT) return name;
  // The cut goes in the middle, not at the end: what tells two switches apart
  // is almost always their last few characters, and `Campus/Guest-Laptop1`
  // and `Campus/Guest-Laptop2` cut at the end are the same card twice.
  const tail = 6;
  return `${name.slice(0, NAME_LIMIT - tail - 1)}…${name.slice(-tail)}`;
}

export function shortInterface(name: string | undefined): string {
  if (!name) return '';
  for (const [long, short] of SHORT_FORMS) {
    if (name.startsWith(long)) return `${short}${name.slice(long.length)}`;
  }
  return name;
}

/**
 * The same table in lower case, long form first, for going the other way:
 * `load.ts` joins a link's interface names to monitoring's own and has to
 * expand a short name rather than miss the rate on that port.
 */
export const LONG_FORMS: readonly (readonly [long: string, short: string])[] = SHORT_FORMS.map(
  ([long, short]) => [long.toLowerCase(), short.toLowerCase()] as const,
);

/** The extent of everything drawn, for the Viewport's fit. */
export function sceneBounds(nodes: readonly Node[]): Bounds | null {
  if (nodes.length === 0) return null;
  let minX = Number.POSITIVE_INFINITY;
  let minY = Number.POSITIVE_INFINITY;
  let maxX = Number.NEGATIVE_INFINITY;
  let maxY = Number.NEGATIVE_INFINITY;
  for (const node of nodes) {
    minX = Math.min(minX, node.x - NODE_W / 2);
    maxX = Math.max(maxX, node.x + NODE_W / 2);
    minY = Math.min(minY, node.y - NODE_H / 2 - 12);
    maxY = Math.max(maxY, node.y + NODE_H / 2 + NODE_LABEL_H);
  }
  // Room for a group outline and its label.
  const margin = 28;
  return {
    x: minX - margin,
    y: minY - margin,
    width: maxX - minX + margin * 2,
    height: maxY - minY + margin * 2,
  };
}

/** The box a group's outline is drawn as: its nodes' cards, plus room. */
export function groupBounds(members: readonly Node[]): Bounds | null {
  const box = sceneBounds(members);
  if (!box) return null;
  const grow = 10;
  return {
    x: box.x + 28 - grow,
    y: box.y + 28 - grow - 10,
    width: box.width - 56 + grow * 2,
    height: box.height - 56 + grow * 2 + 10,
  };
}

/**
 * Groups to draw, with groups that cover exactly the same nodes merged into
 * one. The campus lab is both a site and a lab, so two outlines were drawn on
 * the same rectangle with their labels on top of each other; one outline with
 * both names is the same information and one line.
 */
export function drawableGroups(groups: readonly Group[]): { id: string; kind: GroupKind; label: string; nodes: string[] }[] {
  const byMembers = new Map<string, { id: string; kind: GroupKind; label: string; nodes: string[] }>();
  for (const group of groups) {
    const key = [...group.nodes].sort().join('\u0000');
    const seen = byMembers.get(key);
    if (seen) {
      seen.label = `${seen.label} · ${group.label}`;
      continue;
    }
    byMembers.set(key, { id: group.id, kind: group.kind, label: group.label, nodes: [...group.nodes] });
  }
  // Biggest first, so a group inside another is drawn on top of it.
  return [...byMembers.values()].sort((one, other) => other.nodes.length - one.nodes.length);
}

// --------------------------------------------------------------------- indexes

export function nodeIndex(nodes: readonly Node[]): Map<string, Node> {
  return new Map(nodes.map((node) => [node.id, node]));
}

/** How many links each node has, which decides whether its ports are labelled. */
export function degrees(links: readonly Link[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const link of links) {
    for (const node of [link.a.node, link.b.node]) {
      counts.set(node, (counts.get(node) ?? 0) + 1);
    }
  }
  return counts;
}

/**
 * Interface names are only worth drawing while they can be read: the backend
 * learnt this the hard way around its twelve-link distribution switches, where
 * every label landed on top of another. Six is its number, so it is ours.
 */
export const LABEL_DEGREE_LIMIT = 6;

export function labelPorts(degree: number | undefined): boolean {
  return (degree ?? 0) <= LABEL_DEGREE_LIMIT;
}

/** Every node's position as `topology.layout.save` wants it. */
export function positionsOf(
  nodes: readonly Node[],
  moved?: { id: string; at: Point },
): Record<string, Position> {
  const positions: Record<string, Position> = {};
  for (const node of nodes) {
    positions[node.id] =
      moved && moved.id === node.id
        ? // A position a person chose is pinned: no automatic layout may take it back.
          { x: Math.round(moved.at.x), y: Math.round(moved.at.y), pinned: true }
        : { x: node.x, y: node.y, pinned: node.pinned };
  }
  return positions;
}

/**
 * Any two cards that would overlap, or null when none do. The backend
 * guarantees they never do; this is how the screen checks that the drawing it
 * made kept the guarantee.
 */
export function overlapping(nodes: readonly Node[]): [Node, Node] | null {
  for (let one = 0; one < nodes.length; one += 1) {
    for (let other = one + 1; other < nodes.length; other += 1) {
      const a = nodes[one];
      const b = nodes[other];
      if (!a || !b) continue;
      if (Math.abs(a.x - b.x) < NODE_W && Math.abs(a.y - b.y) < NODE_H) return [a, b];
    }
  }
  return null;
}

// ----------------------------------------------------------------- the picture

/** The sites and roles the drawing itself contains, for the filter controls. */
export function facetsOf(graph: Graph | undefined): { sites: string[]; roles: string[] } {
  const sites = new Set<string>();
  const roles = new Set<string>();
  for (const node of graph?.nodes ?? []) {
    if (node.site) sites.add(node.site);
    if (node.role) roles.add(node.role);
  }
  return {
    sites: [...sites].sort((a, b) => a.localeCompare(b)),
    roles: [...roles].sort((a, b) => a.localeCompare(b)),
  };
}

/** The device ids in the drawing: what "Refresh data" collects from. */
export function deviceIdsOf(graph: Graph | undefined): string[] {
  return (graph?.nodes ?? [])
    .map((node) => node.device_id)
    .filter((id): id is string => typeof id === 'string' && id !== '');
}

/** How old the data is, in milliseconds, or null when nothing says. */
export function dataAge(graph: Graph | undefined, now: number): number | null {
  const oldest = graph?.data_age.oldest_at;
  if (!oldest) return null;
  return Math.max(0, now - oldest);
}
