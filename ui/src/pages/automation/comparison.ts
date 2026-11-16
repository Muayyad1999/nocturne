/**
 * Reading a comparison: "forty-seven devices say this, three differ".
 *
 * `runner.compare` does the work — it trims the lines that are supposed to
 * differ, groups the devices by what is left and diffs every group against
 * the largest one. What is left for the screen is saying what that means in a
 * sentence an engineer can act on, and making the odd one out impossible to
 * miss. These helpers are pure so the sentence can be tested without a
 * backend.
 */

import type { Comparison, CompareGroup, DiffLine } from '@/api/types/automation';
import type { StatusTone } from '@/design/tone';
import { formatCount, plural } from '@/lib/format';

/** How many devices a group holds. */
export function groupSize(group: CompareGroup): number {
  return group.devices.length;
}

export interface Headline {
  title: string;
  detail: string;
  tone: StatusTone;
  /** True when every device that answered said the same thing. */
  unanimous: boolean;
}

/**
 * The one sentence at the top of a comparison.
 *
 * `info` rather than `ok` when everything agrees: agreement is not health —
 * fifty devices can agree on the wrong answer — and a green tick would be the
 * screen having an opinion it has not earned.
 */
export function headline(comparison: Comparison): Headline {
  const groups = comparison.groups;
  const answered = comparison.devices;
  const missing = comparison.missing.length;
  const missingNote =
    missing === 0
      ? ''
      : ` ${plural(missing, 'device')} in this run had no output to compare.`;

  if (answered === 0) {
    return {
      title: 'Nothing to compare',
      detail: `No device in this run produced output for this command.${missingNote}`,
      tone: 'idle',
      unanimous: false,
    };
  }
  if (groups.length <= 1) {
    return {
      title: `All ${formatCount(answered)} agree`,
      detail: `Every device that answered printed the same thing, once the volatile lines were left out.${missingNote}`,
      tone: 'info',
      unanimous: true,
    };
  }
  const majority = groups[0] === undefined ? 0 : groupSize(groups[0]);
  const odd = answered - majority;
  if (majority === 1 && groups.length === answered) {
    return {
      title: `All ${formatCount(answered)} answers differ`,
      detail: `No two devices printed the same thing.${missingNote}`,
      tone: 'warn',
      unanimous: false,
    };
  }
  return {
    title: `${formatCount(majority)} ${majority === 1 ? 'says' : 'say'} one thing, ${formatCount(odd)} ${odd === 1 ? 'differs' : 'differ'}`,
    detail:
      groups.length === 2
        ? `Two different answers across ${plural(answered, 'device')}.${missingNote}`
        : `${formatCount(groups.length)} different answers across ${plural(answered, 'device')}.${missingNote}`,
    tone: 'warn',
    unanimous: false,
  };
}

export interface DiffCount {
  added: number;
  removed: number;
  same: number;
}

/** How much of a group's answer is different from the majority's. */
export function diffCount(diff: readonly DiffLine[]): DiffCount {
  let added = 0;
  let removed = 0;
  let same = 0;
  for (const line of diff) {
    if (line.op === '+') added += 1;
    else if (line.op === '-') removed += 1;
    else same += 1;
  }
  return { added, removed, same };
}

/** "3 lines differ (2 extra, 1 missing)", or "the whole answer differs". */
export function diffSummary(diff: readonly DiffLine[]): string {
  const { added, removed, same } = diffCount(diff);
  if (added + removed === 0) return 'No difference outside the ignored lines';
  if (same === 0) return 'Every line differs';
  const parts: string[] = [];
  if (added > 0) parts.push(`${formatCount(added)} only here`);
  if (removed > 0) parts.push(`${formatCount(removed)} only in the majority`);
  return `${plural(added + removed, 'line')} differ: ${parts.join(', ')}`;
}

/**
 * Only the differing lines, with a little context — what you want to read
 * when a group's answer is four hundred lines and six of them matter.
 */
export function interestingLines(
  diff: readonly DiffLine[],
  context = 2,
): { lines: DiffLine[]; hidden: number } {
  const keep = new Set<number>();
  diff.forEach((line, index) => {
    if (line.op === '=') return;
    for (let at = index - context; at <= index + context; at += 1) {
      if (at >= 0 && at < diff.length) keep.add(at);
    }
  });
  if (keep.size === 0) return { lines: [], hidden: diff.length };
  const lines = diff.filter((_line, index) => keep.has(index));
  return { lines, hidden: diff.length - lines.length };
}

/** The device names of a group, for a chip row and for a table cell. */
export function memberNames(group: CompareGroup): string[] {
  return group.devices.map((device) => device.name);
}

/**
 * Which commands a comparison can be asked for: the ones the run sent. A run
 * of one command still gets the picker, because the answer to "did they all
 * say the same thing" is the point of the screen.
 */
export function comparableCommands(commands: readonly string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const command of commands) {
    const trimmed = command.trim();
    if (trimmed === '' || seen.has(trimmed)) continue;
    seen.add(trimmed);
    out.push(trimmed);
  }
  return out;
}
