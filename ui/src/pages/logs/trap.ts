/**
 * Reading one stored trap.
 *
 * A trap is numbers; the words are the receiver's. Its `message` is the
 * notification's name and its bindings by name, its `body` is the same thing
 * field by field ({@link TrapBody}), and neither ever holds the community
 * the datagram was sent with — `<community>` stands where one was.
 *
 * Three things are kept straight here so that the row and the drawer cannot
 * get them wrong:
 *
 * * **A trap has no severity.** Not "informational", not "unknown": a
 *   notification carries none, the engine gives it none, and the screen shows
 *   none. That is a different thing from a syslog line that came without a
 *   priority, and the two are worded differently.
 * * **A v1 trap's agent address is a claim.** It is shown as what the agent
 *   says about itself, next to the address the datagram really came from.
 * * **Everything in it came from the network.** The values are text and are
 *   returned as text: nothing here builds markup from them.
 *
 * Like `entry.ts`, every field is read defensively. A body that is not the
 * shape the receiver writes yields what it does hold, and `null` for a row
 * that is not a trap at all.
 */

import type { LogEntry } from '@/api/types/core';
import type { TrapBody, TrapVarbind } from '@/api/types/logs';

export interface TrapView {
  version: string | null;
  /** The notification's name; `null` when the dictionary has none for it. */
  name: string | null;
  oid: string | null;
  /** Seconds, as the sender counted them. */
  uptime: number | null;
  inform: boolean;
  enterprise: string | null;
  generic: number | null;
  specific: number | null;
  /** SNMPv1: the address the agent claims, which never decides the device. */
  agentAddress: string | null;
  bindings: TrapVarbind[];
  /** Bindings the datagram held that the row does not. */
  omitted: number;
  ifIndex: number | null;
  /** The interface's name, when the monitor's index map had it. */
  interface: string | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function text(value: unknown): string | null {
  return typeof value === 'string' && value !== '' ? value : null;
}

function number(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

/** Whether a row is of the `trap` stream. */
export function isTrap(entry: Pick<LogEntry, 'stream'>): boolean {
  return entry.stream === 'trap';
}

/** The trap in a row, or `null` when the row is not one. Never throws, never invents. */
export function trapOf(entry: LogEntry): TrapView | null {
  if (!isTrap(entry)) return null;
  const body = (isRecord(entry.body) ? entry.body : {}) as Partial<Record<keyof TrapBody, unknown>>;
  const bindings: TrapVarbind[] = [];
  if (Array.isArray(body.varbinds)) {
    for (const item of body.varbinds) {
      if (!isRecord(item)) continue;
      const oid = text(item.oid);
      if (oid === null) continue;
      bindings.push({
        oid,
        name: text(item.name),
        // A value can honestly be the empty string; only a missing one is not a value.
        value: typeof item.value === 'string' ? item.value : '',
        ...(text(item.interface) === null ? {} : { interface: text(item.interface) as string }),
      });
    }
  }
  return {
    version: text(body.version),
    name: text(body.trap),
    oid: text(body.oid),
    uptime: number(body.uptime),
    inform: body.inform === true,
    enterprise: text(body.enterprise),
    generic: number(body.generic),
    specific: number(body.specific),
    agentAddress: text(body.agent_addr),
    bindings,
    omitted: number(body.varbinds_omitted) ?? 0,
    ifIndex: number(body.if_index),
    interface: text(body.interface),
  };
}

/** What to call a trap in a narrow column: its name, else its OID. */
export function trapTag(view: TrapView): string | null {
  return view.name ?? view.oid;
}

const VERSION_WORD: Record<string, string> = {
  v1: 'SNMPv1',
  v2c: 'SNMPv2c',
  v3: 'SNMPv3',
};

export interface TrapFact {
  label: string;
  /** Text, exactly as it will be shown. `null` hides the row. */
  value: string | null;
  mono?: boolean;
  /** A sentence under the value, where the value alone would mislead. */
  note?: string;
}

/**
 * The body, field by field, for the drawer. A field the datagram did not
 * carry is `null` and is left out; none is filled in.
 */
export function trapFacts(view: TrapView, source: string): TrapFact[] {
  return [
    {
      label: 'Notification',
      value: view.name ?? (view.oid === null ? null : 'no name in the dictionary for this OID'),
      mono: view.name !== null,
    },
    { label: 'OID', value: view.oid, mono: true },
    {
      label: 'Version',
      value:
        view.version === null
          ? null
          : `${VERSION_WORD[view.version] ?? view.version}${view.inform ? ', an inform (acknowledged once stored)' : ''}`,
    },
    { label: 'Sent from', value: source === '' ? null : source, mono: true },
    {
      label: 'Agent address (claimed)',
      value: view.agentAddress,
      mono: true,
      note:
        'What the SNMPv1 agent says its address is. It never decides the device: only the address the datagram came from does.',
    },
    { label: 'Sender’s uptime', value: view.uptime === null ? null : `${view.uptime} s`, mono: true },
    { label: 'Enterprise', value: view.enterprise, mono: true },
    {
      label: 'Generic / specific trap',
      value:
        view.generic === null && view.specific === null
          ? null
          : `${view.generic ?? '—'} / ${view.specific ?? '—'}`,
      mono: true,
    },
    {
      label: 'Interface',
      value:
        view.ifIndex === null
          ? view.interface
          : view.interface === null
            ? `ifIndex ${view.ifIndex}`
            : `${view.interface} (ifIndex ${view.ifIndex})`,
      mono: true,
      note:
        view.ifIndex !== null && view.interface === null
          ? 'No name: the monitor has no index map for this device, or does not know this index. None is made up.'
          : undefined,
    },
  ];
}

/** How a binding is named in the table: its name, else its OID, said to be one. */
export function bindingName(binding: TrapVarbind): { text: string; named: boolean } {
  return binding.name === null
    ? { text: binding.oid, named: false }
    : { text: binding.name, named: true };
}

/** One sentence about the bindings that did not fit, or `null`. */
export function omittedNote(view: TrapView): string | null {
  if (view.omitted <= 0) return null;
  const shown = view.bindings.length;
  return `${view.omitted} more ${view.omitted === 1 ? 'binding was' : 'bindings were'} in the datagram and did not fit in the row (${shown} are shown).`;
}
