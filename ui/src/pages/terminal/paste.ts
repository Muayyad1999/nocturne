/**
 * The seat belt in front of a paste.
 *
 * Two things are worth a second look: text with more than one line (every
 * newline in it is an Enter on the device) and a line the device's own policy
 * calls a change or dangerous. Everything else goes straight through — the
 * belt must not become a lock.
 */

import type { CommandClass, TerminalLineClass } from '@/api/types/terminal';

/** Lines, keeping blank ones, in the order the backend classifies them. */
export function splitLines(text: string): string[] {
  return text.replace(/\r\n?/g, '\n').split('\n');
}

export interface PasteRisk {
  lines: number;
  /** Lines that would change the configuration. */
  changes: number;
  /** Lines the policy calls disruptive. */
  dangerous: number;
  /** The first reason worth repeating in the summary. */
  headline: string | null;
}

const RANK: Record<CommandClass, number> = { read_only: 0, change: 1, dangerous: 2 };

export function pasteRisk(classes: readonly TerminalLineClass[]): PasteRisk {
  let changes = 0;
  let dangerous = 0;
  let worst: TerminalLineClass | null = null;
  for (const line of classes) {
    if (line.class === 'change') changes += 1;
    if (line.class === 'dangerous') dangerous += 1;
    if (!worst || RANK[line.class] > RANK[worst.class]) worst = line;
  }
  return {
    lines: classes.length,
    changes,
    dangerous,
    headline: worst && worst.class !== 'read_only' ? worst.reason : null,
  };
}

/**
 * Whether to put the sheet in front of this paste. `classes` is what
 * `terminal.classify` said; without it (the call failed) only the shape of
 * the text decides, because a missing answer must not wave anything through
 * *or* block everything.
 */
export function needsConfirmation(text: string, classes: readonly TerminalLineClass[] | null): boolean {
  if (hasSeveralLines(text)) return true;
  if (!classes) return false;
  return classes.some((line) => line.class !== 'read_only');
}

/**
 * More than one line of input. A single line with a trailing newline is one
 * command and an Enter: that is what typing it would do anyway.
 */
export function hasSeveralLines(text: string): boolean {
  return splitLines(text.replace(/\n$/, '')).length > 1;
}

/** What the sheet's title says, in one sentence. */
export function pasteSummary(text: string, risk: PasteRisk | null): string {
  const lines = splitLines(text.replace(/\n$/, '')).length;
  const count = lines === 1 ? 'one line' : `${lines} lines`;
  if (!risk || (risk.changes === 0 && risk.dangerous === 0)) {
    return `${count} will be sent to the device as if you had typed them.`;
  }
  const parts: string[] = [];
  if (risk.dangerous > 0) parts.push(`${risk.dangerous} disruptive`);
  if (risk.changes > 0) parts.push(`${risk.changes} that change the configuration`);
  return `${count}, ${parts.join(' and ')}.`;
}
