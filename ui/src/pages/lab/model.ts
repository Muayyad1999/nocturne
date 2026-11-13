/**
 * What the canvas needs to know about a lab, as plain functions.
 *
 * Nothing here calls the backend: everything is derived from the
 * {@link LabView} the backend answered with, so the drawing can never show a
 * node, a cable or a state that the simulator did not report. The geometry
 * lives here (rather than in the component) because it is the part worth
 * testing without a DOM.
 */

import {
  Cloud,
  Gauge,
  Laptop,
  Monitor,
  Network,
  Phone,
  Printer,
  Router,
  Server,
  Shield,
  Wifi,
} from 'lucide-react';
import type { LucideIcon } from '@/design/Icon';
import type { IfView, LinkView, Light, NodeKind, NodeView, NoteSpec } from '@/api/types/lab';

/** The drawn size of a node's body, in world units. */
export const NODE_W = 76;
export const NODE_H = 56;
/** Room under the body for the name and the model line. */
export const NODE_LABEL_H = 30;

export const KIND_ICON: Record<NodeKind, LucideIcon> = {
  router: Router,
  switch: Network,
  l3_switch: Network,
  firewall: Shield,
  access_point: Wifi,
  pc: Monitor,
  laptop: Laptop,
  server: Server,
  printer: Printer,
  phone: Phone,
  cloud: Cloud,
  nms: Gauge,
};

export const KIND_LABEL: Record<NodeKind, string> = {
  router: 'Router',
  switch: 'Switch',
  l3_switch: 'Layer-3 switch',
  firewall: 'Firewall',
  access_point: 'Access point',
  pc: 'PC',
  laptop: 'Laptop',
  server: 'Server',
  printer: 'Printer',
  phone: 'IP phone',
  cloud: 'Cloud',
  nms: 'Management station',
};

export function kindIcon(kind: NodeKind): LucideIcon {
  return KIND_ICON[kind] ?? Network;
}

export function kindLabel(kind: NodeKind): string {
  return KIND_LABEL[kind] ?? kind;
}

/**
 * A name short enough for a 48px palette button. The catalog's own labels
 * (`Router (4 × GE)`, `Router (8 × GE)`) are the same once the parenthesis is
 * dropped, so the models this build ships have their own short names; any
 * model added later falls back to the catalog's label.
 */
const SHORT_MODEL: Record<string, string> = {
  router: 'Router',
  'router-8': 'Router \u00d78',
  switch: 'Switch',
  'switch-l3': 'L3 switch',
  firewall: 'Firewall',
  ap: 'Wi-Fi AP',
  pc: 'PC',
  laptop: 'Laptop',
  server: 'Server',
  printer: 'Printer',
  phone: 'IP phone',
  cloud: 'Internet',
  nms: 'NMS',
};

export function shortModel(id: string, label: string): string {
  const known = SHORT_MODEL[id];
  if (known) return known;
  const open = label.indexOf('(');
  return (open > 0 ? label.slice(0, open) : label).trim();
}

/** The centre of a node's body, which is what a cable is drawn to. */
export interface Point {
  x: number;
  y: number;
}

export function centre(node: Pick<NodeView, 'x' | 'y'>): Point {
  return { x: node.x, y: node.y };
}

export interface Bounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * The extent of everything drawn, for `Viewport`'s fit. An empty lab has no
 * bounds: the viewport then keeps its identity transform and the canvas shows
 * its empty state, rather than zooming into nothing.
 */
export function sceneBounds(nodes: readonly NodeView[], notes: readonly NoteSpec[]): Bounds | null {
  if (nodes.length === 0 && notes.length === 0) return null;
  let left = Infinity;
  let top = Infinity;
  let right = -Infinity;
  let bottom = -Infinity;
  const include = (x: number, y: number, width: number, height: number) => {
    left = Math.min(left, x);
    top = Math.min(top, y);
    right = Math.max(right, x + width);
    bottom = Math.max(bottom, y + height);
  };
  for (const node of nodes) {
    include(node.x - NODE_W / 2, node.y - NODE_H / 2, NODE_W, NODE_H + NODE_LABEL_H);
  }
  for (const note of notes) include(note.x, note.y, note.w, note.h);
  return { x: left, y: top, width: right - left, height: bottom - top };
}

/**
 * Where a cable meets a node: on the edge of its body, so the line does not
 * run under the icon. Two nodes in the same place get a short stub rather
 * than a division by zero.
 */
export function edgePoint(from: Point, to: Point): Point {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  if (dx === 0 && dy === 0) return { x: from.x + NODE_W / 2, y: from.y };
  const halfW = NODE_W / 2;
  const halfH = NODE_H / 2;
  // Scale the direction until it touches the nearer of the two edges.
  const scale = Math.min(
    dx === 0 ? Infinity : Math.abs(halfW / dx),
    dy === 0 ? Infinity : Math.abs(halfH / dy),
  );
  return { x: from.x + dx * scale, y: from.y + dy * scale };
}

export interface CableGeometry {
  a: Point;
  b: Point;
  /** Where the port label at each end sits, pulled in along the cable. */
  aLabel: Point;
  bLabel: Point;
  /** Where a light sits at each end. */
  aLight: Point;
  bLight: Point;
}

/**
 * Distance a light and a port label sit from the body they belong to. The
 * label is well clear of the node's own name, which is drawn underneath it.
 */
const LIGHT_OFFSET = 10;
const LABEL_OFFSET = 34;

function along(from: Point, to: Point, distance: number, aside = 0): Point {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const length = Math.hypot(dx, dy);
  if (length === 0) return { ...from };
  const unit = { x: dx / length, y: dy / length };
  // `aside` moves the point across the cable, which is what keeps a port
  // label off the node name drawn directly under the device.
  return {
    x: from.x + unit.x * distance - unit.y * aside,
    y: from.y + unit.y * distance + unit.x * aside,
  };
}

/** How far a port label sits beside the cable, so it clears the node's name. */
const LABEL_ASIDE = 8;

export function cableGeometry(aCentre: Point, bCentre: Point): CableGeometry {
  const a = edgePoint(aCentre, bCentre);
  const b = edgePoint(bCentre, aCentre);
  // On a short cable the two labels would meet in the middle; keep each one
  // in its own third of it.
  const span = Math.hypot(b.x - a.x, b.y - a.y);
  const offset = Math.min(LABEL_OFFSET, Math.max(LIGHT_OFFSET, span * 0.33));
  return {
    a,
    b,
    aLight: along(a, b, LIGHT_OFFSET),
    bLight: along(b, a, LIGHT_OFFSET),
    aLabel: along(a, b, offset, LABEL_ASIDE),
    bLabel: along(b, a, offset, -LABEL_ASIDE),
  };
}

/** The design token a link light is drawn in. Status is never colour alone: the inspector names it. */
export function lightColor(light: Light): string {
  switch (light) {
    case 'green':
      return 'var(--ok)';
    case 'amber':
      return 'var(--warn)';
    case 'red':
      return 'var(--crit)';
    case 'off':
      return 'var(--idle)';
  }
}

export function lightLabel(light: Light): string {
  switch (light) {
    case 'green':
      return 'Forwarding';
    case 'amber':
      return 'Spanning tree is holding it (listening, learning or blocking)';
    case 'red':
      return 'Down';
    case 'off':
      return 'No signal';
  }
}

/** A port's one-line state, in the words a `show` command would use. */
export function interfaceState(port: IfView): string {
  if (!port.admin_up) return 'administratively down';
  return port.oper_up ? 'up' : 'down';
}

/** Interfaces with nothing cabled to them. */
export function freeInterfaces(node: NodeView): IfView[] {
  return node.interfaces.filter((port) => port.link == null);
}

/** An index from node id to node, for the many link → node lookups a draw does. */
export function nodeIndex(nodes: readonly NodeView[]): Map<number, NodeView> {
  return new Map(nodes.map((node) => [node.id, node]));
}

/** The short name of the port at `end` of `link`, for a label. */
export function portLabel(node: NodeView | undefined, interfaceName: string): string {
  const port = node?.interfaces.find((candidate) => candidate.name === interfaceName);
  return port?.short ?? interfaceName;
}

/** Room for one more device, on a grid, so nodes never land on top of each other. */
export function freeSpot(
  nodes: readonly NodeView[],
  start: Point = { x: 160, y: 140 },
  columns = 6,
): Point {
  const taken = (point: Point) =>
    nodes.some((node) => Math.abs(node.x - point.x) < NODE_W && Math.abs(node.y - point.y) < NODE_H + NODE_LABEL_H);
  for (let index = 0; index < 400; index += 1) {
    const wide = Math.max(2, Math.round(columns));
    const point = {
      x: start.x + (index % wide) * (NODE_W + 60),
      y: start.y + Math.floor(index / wide) * (NODE_H + NODE_LABEL_H + 40),
    };
    if (!taken(point)) return point;
  }
  return start;
}

/** Nodes that `labs.connectivity` and `labs.probe` can speak for: powered, with an address. */
export function addressableNodes(nodes: readonly NodeView[]): NodeView[] {
  return nodes.filter((node) => node.powered && nodeAddress(node) !== null);
}

/** The address the simulator would probe this node on, by the same rule the backend uses. */
export function nodeAddress(node: NodeView): string | null {
  if (node.mgmt_ip) return node.mgmt_ip;
  const addressed = node.interfaces.find((port) => port.ip);
  return addressed?.ip?.split('/')[0] ?? null;
}

/** Simulated microseconds as a clock a person can read: `0:21:15`. */
export function simClock(microseconds: number): string {
  const total = Math.max(0, Math.floor(microseconds / 1_000_000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  const pad = (value: number) => String(value).padStart(2, '0');
  return `${hours}:${pad(minutes)}:${pad(seconds)}`;
}

/** Link ids, lights and rates as the last `labs.tick` reported them. */
export type LinkLights = ReadonlyMap<number, Pick<LinkView, 'up' | 'a_light' | 'b_light' | 'bps_ab' | 'bps_ba'>>;

/** The link as the snapshot has it, with anything a later tick changed laid over it. */
export function withLights(link: LinkView, lights: LinkLights): LinkView {
  const live = lights.get(link.id);
  return live ? { ...link, ...live } : link;
}
