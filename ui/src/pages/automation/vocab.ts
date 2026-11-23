/**
 * The words this screen uses, in one place.
 *
 * Four outcomes per device is the heart of it, and three of them are not
 * "it broke": a device that could not be *reached* was never spoken to, a
 * device that was *skipped* was deliberately left alone (another run held it,
 * or the run was stopped first), and only a *failure* means the device
 * answered and the answer was a refusal. A cell that disagreed with a tooltip
 * about which of those happened would be worse than no cell at all, so every
 * label, tone, icon and explanation comes from here.
 */

import {
  CircleCheck,
  CircleMinus,
  OctagonX,
  PlugZap,
  type LucideIcon,
} from 'lucide-react';
import type { DeviceState, Run, RunKind, Schedule, When } from '@/api/types/automation';
import type { StatusTone } from '@/design/tone';
import { formatCount, formatDuration, plural } from '@/lib/format';
import { given, orNone } from './given';

export interface StateWords {
  /** The word in a cell or a heading: short, and never just a colour. */
  label: string;
  /** The heading over a group of these devices. */
  group: string;
  tone: StatusTone;
  icon: LucideIcon;
  /** One sentence: what this outcome means, for the group's caption. */
  explain: string;
}

export const DEVICE_STATE: Record<DeviceState, StateWords> = {
  ok: {
    label: 'Answered',
    group: 'Answered',
    tone: 'ok',
    icon: CircleCheck,
    explain: 'Every command was sent and the device answered all of them.',
  },
  failed: {
    label: 'Failed',
    group: 'Failed',
    tone: 'crit',
    icon: OctagonX,
    explain:
      'Nocturne spoke to the device and the device refused a command or broke off part way. What it said is below, as it said it.',
  },
  unreached: {
    label: 'Not reached',
    group: 'Not reached',
    tone: 'warn',
    icon: PlugZap,
    explain:
      'No session could be opened, so nothing was sent at all. This is not a failed command: the device never heard one.',
  },
  skipped: {
    label: 'Skipped',
    group: 'Skipped',
    tone: 'idle',
    icon: CircleMinus,
    explain:
      'Nocturne has no answer from these to show, and each one says why: another run held the device, this run was stopped before its turn, or Nocturne was closed while the run was going and what the device did was not kept. None of them refused anything: nothing failed that Nocturne saw.',
  },
};

export function stateWords(state: DeviceState): StateWords {
  return DEVICE_STATE[state] ?? DEVICE_STATE.failed;
}

/** The order results are grouped in: what you have, then what you have not. */
export const STATE_ORDER: readonly DeviceState[] = ['failed', 'unreached', 'skipped', 'ok'];

export const RUN_KIND: Record<RunKind, string> = {
  commands: 'Commands',
  playbook: 'Playbook',
  ansible: 'Ansible',
};

export function kindLabel(kind: RunKind): string {
  return RUN_KIND[kind] ?? kind;
}

/** Who asked for it. The backend's own words are `user`, `schedule`, … */
export function originLabel(origin: string): string {
  switch (origin) {
    case 'user':
      return 'A person';
    case 'schedule':
      return 'A schedule';
    case 'trigger':
      return 'A trigger';
    case 'assistant':
      return 'The assistant';
    default:
      return origin;
  }
}

export interface Outcome {
  title: string;
  detail?: string;
  tone: StatusTone;
}

/**
 * What a finished run did, in one sentence, from the run record — which is
 * where the truth about the devices is, not in the job.
 *
 * The counts are named separately on purpose: "2 failed" and "2 could not be
 * reached" are different facts and a single "2 problems" would hide which.
 */
export function describeRun(run: Run): Outcome {
  const parts: string[] = [];
  if (run.ok > 0) parts.push(`${formatCount(run.ok)} answered`);
  if (run.failed > 0) parts.push(`${formatCount(run.failed)} failed`);
  if (run.unreached > 0) parts.push(`${formatCount(run.unreached)} not reached`);
  if (run.skipped > 0) parts.push(`${formatCount(run.skipped)} skipped`);
  const detail = parts.join(' · ') || undefined;

  if (run.cancelled) {
    return { title: 'Stopped', detail, tone: 'warn' };
  }
  if (!given(run.finished_at)) {
    return { title: 'Running', detail, tone: 'info' };
  }
  if (run.failed > 0) {
    return {
      title: `Failed on ${plural(run.failed, 'device')}`,
      detail,
      tone: 'crit',
    };
  }
  if (run.unreached > 0) {
    return {
      title: `${plural(run.unreached, 'device')} could not be reached`,
      detail,
      tone: 'warn',
    };
  }
  if (run.skipped > 0 && run.ok === 0) {
    return { title: 'Every device was skipped', detail, tone: 'idle' };
  }
  return {
    title: run.dry_run
      ? `Dry run over ${plural(run.devices, 'device')}`
      : `${plural(run.ok, 'device')} answered`,
    detail,
    tone: 'ok',
  };
}

/**
 * A schedule's last run in a word. The backend writes a run down when it
 * starts, before its outcome is known, so "running" is a word of its own: no
 * outcome yet is not a failure. A run Nocturne did not live through is
 * recorded as failed, with the reason, when it next starts.
 */
export function lastRunWords(
  schedule: Pick<Schedule, 'last_run_at' | 'last_ok' | 'last_error'>,
): Outcome | null {
  if (!given(schedule.last_run_at)) return null;
  if (schedule.last_ok === true) return { title: 'Ran', tone: 'ok' };
  if (schedule.last_ok === false) {
    return { title: 'Failed', tone: 'crit', detail: orNone(schedule.last_error) };
  }
  return { title: 'Running', tone: 'info', detail: 'Started, and has not finished yet' };
}

/** How long a run took, or how long it has been going. */
export function runElapsed(run: Run, now: number = Date.now()): string {
  const end = orNone(run.finished_at) ?? now;
  return formatDuration(Math.max(0, end - run.started_at));
}

const DAY_NAMES: Record<string, string> = {
  mon: 'Monday',
  tue: 'Tuesday',
  wed: 'Wednesday',
  thu: 'Thursday',
  fri: 'Friday',
  sat: 'Saturday',
  sun: 'Sunday',
};

/** A list in words: "Monday", "Monday and Thursday", "Mon, Wed and Fri". */
export function listWords(items: readonly string[]): string {
  if (items.length === 0) return '';
  if (items.length === 1) return items[0] ?? '';
  return `${items.slice(0, -1).join(', ')} and ${items.at(-1)}`;
}

/**
 * When a schedule fires, in a sentence. Local time, because that is what the
 * backend walks: "02:30" stays 02:30 across a clock change.
 */
export function whenText(when: When): string {
  switch (when.type) {
    case 'interval':
      return `Every ${formatDuration(when.every_s * 1000)}`;
    case 'daily': {
      const days = when.days ?? [];
      if (days.length === 0 || days.length === 7) return `Every day at ${when.at}`;
      const named = days.map((day) => DAY_NAMES[day] ?? day);
      return `${listWords(named)} at ${when.at}`;
    }
    case 'cron':
      return `Cron: ${when.expr}`;
    default:
      return 'Never';
  }
}

/** The five cron fields in words, for the hint under the expression. */
export const CRON_HELP = 'minute hour day-of-month month day-of-week — `30 2 * * 1-5` is 02:30 on weekdays';

/**
 * A trigger's debounce, in words. `0` is "at the next sweep", which is not
 * "immediately" and must not read as it.
 */
export function debounceText(seconds: number): string {
  if (seconds <= 0) return 'Fires at the next sweep';
  return `Collects events for ${formatDuration(seconds * 1000)}, then fires once`;
}
