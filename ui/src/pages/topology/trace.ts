/**
 * A traced path, put on top of the drawing.
 *
 * `topology.path` walks the collected routing, ARP and MAC tables and stops
 * when they run out, saying why. The valuable part of the answer is often that
 * sentence, so the overlay keeps it: every hop that is a node in the drawing
 * is numbered, the steps between them are drawn over the links they used, and
 * a walk that did not arrive ends in a stub that goes nowhere with the
 * backend's own reason beside it.
 *
 * A hop the drawing does not contain (a host that was never collected, a next
 * hop belonging to nobody we manage) is not invented as a node: it becomes the
 * end of the line, which is what it is.
 */

import type { Hop, Link, Node, Trace } from '@/api/types/topology';
import { centre, edgeExit, type Point } from './graph';

export interface TraceStep {
  hop: Hop;
  /** The node this hop is, when the drawing has it. */
  node?: Node;
  /** Its number on the drawing, from 1. */
  index: number;
}

export interface TraceSegment {
  from: Point;
  to: Point;
  /** The link this step went over, when one of the drawn links is it. */
  linkId?: string;
}

export interface TraceOverlay {
  steps: TraceStep[];
  segments: TraceSegment[];
  /** Where the walk stopped, when it did not arrive: the end of the stub. */
  gap?: { from: Point; to: Point; text: string };
  /** Node ids on the path, for dimming everything else. */
  onPath: Set<string>;
  /** Link ids on the path. */
  usedLinks: Set<string>;
}

/** The node a hop is, matched on the device id first and only then on the name. */
function nodeFor(hop: Hop, nodes: readonly Node[]): Node | undefined {
  if (hop.device_id) {
    const byId = nodes.find((node) => node.device_id === hop.device_id);
    if (byId) return byId;
  }
  return nodes.find((node) => node.name === hop.node || node.address === hop.node);
}

/**
 * The link between two consecutive hops. The interfaces the walk named are
 * preferred, so a pair of nodes joined by several cables highlights the one
 * that was actually used; without them, any link between the two will do.
 */
function linkBetween(
  from: Node,
  to: Node,
  out: string | undefined,
  into: string | undefined,
  links: readonly Link[],
): Link | undefined {
  const joins = links.filter(
    (link) =>
      (link.a.node === from.id && link.b.node === to.id) ||
      (link.b.node === from.id && link.a.node === to.id),
  );
  if (joins.length <= 1) return joins[0];
  const named = joins.find((link) => {
    const fromEnd = link.a.node === from.id ? link.a : link.b;
    const toEnd = link.a.node === from.id ? link.b : link.a;
    return (
      (!out || fromEnd.interface === out) && (!into || toEnd.interface === into)
    );
  });
  return named ?? joins[0];
}

/** How far the stub that marks a dead end reaches, in world units. */
const GAP_LENGTH = 120;

export function traceOverlay(
  trace: Trace | undefined,
  nodes: readonly Node[],
  links: readonly Link[],
): TraceOverlay | null {
  if (!trace) return null;
  const steps: TraceStep[] = trace.hops.map((hop, index) => {
    const node = nodeFor(hop, nodes);
    return node ? { hop, node, index: index + 1 } : { hop, index: index + 1 };
  });

  const segments: TraceSegment[] = [];
  const onPath = new Set<string>();
  const usedLinks = new Set<string>();
  for (const step of steps) {
    if (step.node) onPath.add(step.node.id);
  }

  for (let index = 0; index + 1 < steps.length; index += 1) {
    const from = steps[index];
    const to = steps[index + 1];
    if (!from?.node || !to?.node) continue;
    const link = linkBetween(from.node, to.node, from.hop.out_interface, to.hop.in_interface, links);
    if (link) usedLinks.add(link.id);
    segments.push({
      from: edgeExit(centre(from.node), centre(to.node)),
      to: edgeExit(centre(to.node), centre(from.node)),
      ...(link ? { linkId: link.id } : {}),
    });
  }

  let gap: TraceOverlay['gap'];
  const reason = trace.stopped_because;
  if (!trace.complete && reason) {
    // The stub hangs off the last hop that is on the drawing.
    const last = [...steps].reverse().find((step) => step.node);
    if (last?.node) {
      const previous = steps[steps.indexOf(last) - 1];
      const anchor = centre(last.node);
      // Away from where it came from, so the stub does not lie over the path.
      const back = previous?.node ? centre(previous.node) : { x: anchor.x, y: anchor.y - 1 };
      const dx = anchor.x - back.x;
      const dy = anchor.y - back.y;
      const length = Math.hypot(dx, dy) || 1;
      const direction = { x: dx / length, y: dy / length };
      const start = edgeExit(anchor, {
        x: anchor.x + direction.x * 1000,
        y: anchor.y + direction.y * 1000,
      });
      gap = {
        from: start,
        to: { x: start.x + direction.x * GAP_LENGTH, y: start.y + direction.y * GAP_LENGTH },
        text: reason,
      };
    }
  }

  return { steps, segments, onPath, usedLinks, ...(gap ? { gap } : {}) };
}

/** How a hop was worked out, in words for a person. */
export function viaLabel(via: Hop['via']): string {
  switch (via) {
    case 'connected':
      return 'directly connected';
    case 'route':
      return 'routing table';
    case 'arp':
      return 'ARP table';
    case 'mac':
      return 'MAC table';
  }
}
