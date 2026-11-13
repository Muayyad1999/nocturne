/**
 * What the one search box does.
 *
 * A device name finds a node in the drawing. An address — IP or MAC — is a
 * question the drawing cannot answer by itself, so it goes to `locate.host`,
 * which reads the collected ARP and MAC tables and says which port it decided
 * on. Deciding which of the two a query is happens here, with no guessing:
 * `lib/net` parses it, and anything that parses as neither is a name.
 */

import type { Node } from '@/api/types/topology';
import { normalizeMac, parseIp } from '@/lib/net';

export type Query =
  | { kind: 'empty' }
  /** An IP or a MAC address: ask `locate.host` where it is plugged in. */
  | { kind: 'address'; text: string; address: 'ip' | 'mac' }
  /** Anything else: look for it among the nodes on screen. */
  | { kind: 'name'; text: string };

export function classify(text: string): Query {
  const trimmed = text.trim();
  if (trimmed === '') return { kind: 'empty' };
  if (parseIp(trimmed)) return { kind: 'address', text: trimmed, address: 'ip' };
  if (normalizeMac(trimmed)) return { kind: 'address', text: trimmed, address: 'mac' };
  return { kind: 'name', text: trimmed };
}

/**
 * The nodes a name matches, best first: an exact name, then a name that starts
 * with it, then one that contains it, then an address that contains it. Ties
 * break on the name, so the list is the same every time.
 */
export function matches(nodes: readonly Node[], text: string): Node[] {
  const needle = text.trim().toLowerCase();
  if (needle === '') return [];
  const scored: { node: Node; score: number }[] = [];
  for (const node of nodes) {
    const name = node.name.toLowerCase();
    const address = (node.address ?? '').toLowerCase();
    let score = -1;
    if (name === needle) score = 0;
    else if (address === needle) score = 1;
    else if (name.startsWith(needle)) score = 2;
    else if (name.includes(needle)) score = 3;
    else if (address.includes(needle)) score = 4;
    if (score >= 0) scored.push({ node, score });
  }
  scored.sort((one, other) =>
    one.score === other.score ? one.node.name.localeCompare(other.node.name) : one.score - other.score,
  );
  return scored.map((entry) => entry.node);
}

/** What a device looks like to `topology.path`: an id if it has one, else its name. */
export function pathReference(node: Pick<Node, 'device_id' | 'name'>): string {
  return node.device_id ?? node.name;
}
