/**
 * The cable tool: a two-click gesture, and the refusals it can work out for
 * itself.
 *
 * The common case is one click on each node and the simulator picks the first
 * free port that suits the cable — exactly what `labs.link.add` does with no
 * interface named. Choosing the ports by hand is a separate dialog, because
 * making every cable a four-step wizard is how a lab stops being fun.
 *
 * What this module will *not* do is guess: if a node has no free port, the
 * attempt is refused here with the port list as the reason, and if the
 * backend refuses for a reason only it can know, its sentence is the one the
 * user sees.
 */

import type { CableKind, NodeView } from '@/api/types/lab';
import { freeInterfaces } from './model';

export type CableTool =
  /** Not cabling: clicks select. */
  | { mode: 'off' }
  /** Cabling, waiting for the first end. */
  | { mode: 'armed' }
  /** The first end is chosen; the next click finishes the cable. */
  | { mode: 'from'; node: string };

export const CABLE_OFF: CableTool = { mode: 'off' };

/** What a click on the canvas background does to the tool: it keeps cabling, but forgets the first end. */
export function cancelEnd(tool: CableTool): CableTool {
  return tool.mode === 'from' ? { mode: 'armed' } : tool;
}

export interface CableStep {
  tool: CableTool;
  /** Set when the gesture is complete and a cable should be made. */
  connect?: { a: string; b: string };
  /** Why this click did nothing, in a sentence to show the user. */
  refused?: string;
}

/**
 * A click on `node` while the tool is armed. The first click records the end,
 * the second asks for the cable; a node with no free port is refused before
 * anything is sent.
 */
export function clickNode(tool: CableTool, node: NodeView): CableStep {
  if (tool.mode === 'off') return { tool };

  const free = freeInterfaces(node);
  if (free.length === 0) {
    return {
      tool,
      refused: `${node.name} has no free interface: all ${node.interfaces.length} of its ports are cabled. Remove a cable, or use a model with more ports.`,
    };
  }

  if (tool.mode === 'armed') return { tool: { mode: 'from', node: node.name } };

  if (tool.node === node.name) {
    return { tool, refused: 'A cable needs two different nodes.' };
  }
  return { tool: { mode: 'armed' }, connect: { a: tool.node, b: node.name } };
}

/** The sentence under the canvas while the tool is on. */
export function cableHint(tool: CableTool): string | null {
  switch (tool.mode) {
    case 'off':
      return null;
    case 'armed':
      return 'Click the first device to cable, then the second. Esc stops.';
    case 'from':
      return `Cabling from ${tool.node}: click the device at the other end. Esc stops.`;
  }
}

/**
 * The port `labs.link.add` would choose at this end, so the dialog can offer
 * it as the default and name it. Fibre prefers a Gigabit or 10-Gigabit port,
 * as the backend does; a console cable has its own pseudo-port.
 */
export function defaultInterface(node: NodeView, cable: CableKind): string | null {
  if (cable === 'console') {
    if (node.cli === 'ios') return 'Console';
    if (node.cli === 'host') return 'RS232';
    return null;
  }
  const free = freeInterfaces(node).map((port) => port.name);
  if (cable === 'fiber') {
    const fibre = free.find((name) => name.startsWith('TenGigabit') || name.startsWith('Gigabit'));
    if (fibre) return fibre;
  }
  return free[0] ?? null;
}

/** Why a console cable cannot be attached to this node, if it cannot. */
export function consoleRefusal(node: NodeView): string | null {
  if (node.cli === 'none') return `${node.name} has no console port: it has no command line.`;
  return null;
}
