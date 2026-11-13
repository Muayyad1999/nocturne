/**
 * Recently opened targets, remembered on this machine.
 *
 * The backend keeps sessions, not a history of what was dialled, so the
 * "recent" list is the one piece of this screen that is local. It holds no
 * secrets: a device id, or the `user@host:port` that was typed.
 */

import type { AdhocTransport } from '@/api/types/terminal';
import { readStored, storageKey, writeStored } from '@/lib/storage';

export interface RecentDevice {
  kind: 'device';
  id: string;
  label: string;
  at: number;
}

export interface RecentAdhoc {
  kind: 'adhoc';
  transport: AdhocTransport;
  /** What was typed into quick connect, `user@host[:port]`. */
  target: string;
  label: string;
  at: number;
}

export type Recent = RecentDevice | RecentAdhoc;

/** A recent before it has a time: what callers hand to `rememberRecent`. */
export type NewRecent = Omit<RecentDevice, 'at'> | Omit<RecentAdhoc, 'at'>;

const KEY = storageKey('terminal.recent');
const LIMIT = 8;

function isRecent(value: unknown): value is Recent {
  if (typeof value !== 'object' || value === null) return false;
  const entry = value as Partial<Recent> & { kind?: string };
  if (typeof entry.label !== 'string' || typeof entry.at !== 'number') return false;
  if (entry.kind === 'device') return typeof (entry as { id?: unknown }).id === 'string';
  if (entry.kind === 'adhoc') {
    return (
      typeof (entry as { target?: unknown }).target === 'string' &&
      typeof (entry as { transport?: unknown }).transport === 'string'
    );
  }
  return false;
}

function isRecentList(value: unknown): value is Recent[] {
  return Array.isArray(value) && value.every(isRecent);
}

export function readRecents(): Recent[] {
  return (readStored(KEY, isRecentList) ?? []).slice(0, LIMIT);
}

/** Puts a target at the top of the list, keeping it to one entry per target. */
export function rememberRecent(entry: NewRecent): Recent[] {
  const identity = (recent: Recent) =>
    recent.kind === 'device' ? `device:${recent.id}` : `adhoc:${recent.transport}:${recent.target}`;
  const next: Recent = { ...entry, at: Date.now() };
  const list = [next, ...readRecents().filter((recent) => identity(recent) !== identity(next))].slice(0, LIMIT);
  writeStored(KEY, list);
  return list;
}
