/**
 * "Fifty devices, one command, and the odd one out is obvious."
 *
 * `runner.compare` groups the devices by what they printed — after trimming
 * the lines that are *supposed* to differ — and diffs every other group
 * against the largest. This view's whole job is to make the shape of that
 * answer readable in one glance: the majority first with its device count,
 * then each dissenting group with what it says differently, and never a
 * difference hidden without saying so.
 *
 * Three honesty rules live here:
 *
 * * the trimmed patterns are listed, because a comparison that ignored a line
 *   you cared about would otherwise look like agreement;
 * * `+` and `-` are drawn as signs as well as colours, like `design/DiffView`;
 * * the devices with no output for this command are shown with *why* — and an
 *   unreachable device, a failed one and a skipped one are three different
 *   sentences, not one grey "missing".
 */

import { ChevronDown, Equal, Search, Server, SplitSquareHorizontal } from 'lucide-react';
import { useState } from 'react';
import { useRpc } from '@/api/hooks';
import type { Comparison, CompareGroup, DeviceState, DiffLine, Run } from '@/api/types/automation';
import { Badge } from '@/design/Badge';
import { CodeBlock } from '@/design/CodeBlock';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Icon } from '@/design/Icon';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { StatusPill } from '@/design/StatusPill';
import { Switch } from '@/design/Switch';
import { formatCount, plural } from '@/lib/format';
import styles from './Automation.module.css';
import { comparableCommands, diffSummary, headline, interestingLines } from './comparison';
import { given } from './given';
import { stateWords } from './vocab';

export interface CompareViewProps {
  run: Run;
  /** Lab device ids, so a simulated device carries its pill here too. */
  labDevices: ReadonlySet<string>;
}

export function CompareView({ run, labDevices }: CompareViewProps) {
  const commands = comparableCommands(run.commands);
  const [command, setCommand] = useState(commands[0] ?? '');
  const [whole, setWhole] = useState(false);
  const chosen = commands.includes(command) ? command : (commands[0] ?? '');

  const comparison = useRpc<Comparison>(
    'runner.compare',
    { run_id: run.id, command: chosen },
    // The results are written when the run ends, so a comparison asked for
    // while it was still going has to be worked out again when they land.
    { enabled: chosen !== '', invalidateOn: ['automation.runs.changed'] },
  );

  if (commands.length === 0) {
    return (
      <EmptyState
        icon={SplitSquareHorizontal}
        size="compact"
        title="Nothing to compare"
        description={
          run.kind === 'commands'
            ? 'This run sent no command, so there is no answer to put side by side.'
            : `A ${run.kind} run has no single command every device was asked; look at the devices instead.`
        }
      />
    );
  }

  return (
    <div className={styles.stack}>
      <div className={styles.compareBar}>
        <Field label="Compare what they said to">
          <Select
            value={chosen}
            options={commands.map((entry) => ({ value: entry, label: entry }))}
            onChange={(event) => setCommand(event.target.value)}
          />
        </Field>
        <Switch checked={whole} onChange={(event) => setWhole(event.target.checked)}>
          Show every line of a difference
        </Switch>
      </div>

      {comparison.isLoading && <Skeleton shape="block" height={220} />}
      {comparison.error && (
        <ErrorState
          title="The comparison could not be worked out"
          error={comparison.error}
          onRetry={() => void comparison.refetch()}
        />
      )}
      {comparison.data && <Groups comparison={comparison.data} whole={whole} labDevices={labDevices} />}
    </div>
  );
}

function Groups({
  comparison,
  whole,
  labDevices,
}: {
  comparison: Comparison;
  whole: boolean;
  labDevices: ReadonlySet<string>;
}) {
  const summary = headline(comparison);
  return (
    <>
      <div className={styles.compareHead} data-tone={summary.tone}>
        <StatusPill tone={summary.tone} icon={summary.unanimous ? Equal : SplitSquareHorizontal}>
          {summary.title}
        </StatusPill>
        <p>{summary.detail}</p>
      </div>

      {comparison.ignored_patterns.length > 0 && (
        <details className={styles.ignored}>
          <summary>
            <Icon icon={Search} size={12} />
            Lines matching {plural(comparison.ignored_patterns.length, 'pattern')} were left out of the
            grouping
          </summary>
          <p>
            Lines matching these are expected to differ — uptimes, clocks, “Building configuration” — so devices are
            grouped as if they were not there. A real difference on one of these lines will not show up here.
          </p>
          <ul>
            {comparison.ignored_patterns.map((pattern) => (
              <li key={pattern}>
                <code>{pattern}</code>
              </li>
            ))}
          </ul>
        </details>
      )}

      <ol className={styles.groups}>
        {comparison.groups.map((group, index) => (
          <Group
            key={group.devices.map((device) => device.device_id).join(',') || index}
            group={group}
            index={index}
            groupCount={comparison.groups.length}
            whole={whole}
            command={comparison.command}
            labDevices={labDevices}
          />
        ))}
      </ol>

      {comparison.missing.length > 0 && <Missing comparison={comparison} />}
    </>
  );
}

function Group({
  group,
  index,
  groupCount,
  whole,
  command,
  labDevices,
}: {
  group: CompareGroup;
  index: number;
  groupCount: number;
  whole: boolean;
  command: string;
  labDevices: ReadonlySet<string>;
}) {
  const majority = index === 0;
  // What differs is what you came for: the dissenting groups are open and the
  // majority's (long, agreed) output is one click away. With one group there
  // is nothing to differ from, so that one is open.
  const [open, setOpen] = useState(majority ? groupCount === 1 : index < 4);
  const { lines, hidden } = interestingLines(group.diff);
  const shown = whole ? group.diff : lines;

  return (
    <li className={styles.group} data-majority={majority || undefined}>
      <div className={styles.groupHead}>
        <button
          type="button"
          className={styles.groupToggle}
          aria-expanded={open}
          onClick={() => setOpen(!open)}
        >
          <Icon icon={ChevronDown} size={14} className={open ? styles.openChevron : styles.shutChevron} />
          <Badge tone={majority ? 'info' : 'warn'}>
            {!majority ? 'Differs' : group.devices.length > 1 ? 'Majority' : 'Baseline'}
          </Badge>
          <span className={styles.groupCount}>
            <Icon icon={Server} size={12} />
            {plural(group.devices.length, 'device')}
          </span>
          <span className={styles.groupSummary}>
            {majority
              ? groupCount === 1
                ? 'what every device that answered printed'
                : 'the answer everything else is compared against — open it to read it'
              : diffSummary(group.diff)}
          </span>
        </button>
      </div>

      <ul className={styles.groupDevices} aria-label={`Devices in group ${index + 1}`}>
        {group.devices.map((device) => (
          <li key={device.device_id} className={styles.groupDevice}>
            {device.name}
            {labDevices.has(device.device_id) && <span className={styles.labTag}>LAB</span>}
          </li>
        ))}
      </ul>

      {open && (
        <div className={styles.groupBody}>
          {majority ? (
            <CodeBlock title={`${command} — as ${plural(group.devices.length, 'device')} answered it`} code={group.output} maxHeight={320} />
          ) : (
            <>
              <Diff lines={shown} />
              {!whole && hidden > 0 && (
                <p className={styles.hint}>
                  {formatCount(hidden)} identical {hidden === 1 ? 'line is' : 'lines are'} not shown. Turn on “Show
                  every line of a difference” for the whole answer.
                </p>
              )}
              <details className={styles.full}>
                <summary>What this group printed, in full</summary>
                <CodeBlock code={group.output} maxHeight={320} />
              </details>
            </>
          )}
        </div>
      )}
    </li>
  );
}

const SIGN: Record<DiffLine['op'], string> = { '=': ' ', '-': '−', '+': '+' };
const SPOKEN: Record<DiffLine['op'], string> = {
  '=': 'the same',
  '-': 'only in the majority',
  '+': 'only here',
};

function Diff({ lines }: { lines: readonly DiffLine[] }) {
  if (lines.length === 0) {
    return <p className={styles.hint}>Nothing differs outside the ignored lines.</p>;
  }
  // A device's line is not wrapped: it scrolls sideways, like every other
  // place in Nocturne that shows what a device printed.
  return (
    <ul
      className={styles.diff}
      aria-label="What this group printed, against the majority: − is only in the majority, + is only here"
    >
      {lines.map((line, index) => (
        <li key={`${index}-${line.text}`} data-op={line.op === '=' ? 'same' : line.op === '+' ? 'add' : 'remove'}>
          <span className={styles.diffSign}>
            <span aria-hidden="true">{SIGN[line.op]}</span>
            <span className="visually-hidden">{SPOKEN[line.op]}</span>
          </span>
          <code>{line.text === '' ? '\u00a0' : line.text}</code>
        </li>
      ))}
    </ul>
  );
}

/** The devices in the run with nothing to compare — and why, in their own words. */
function Missing({ comparison }: { comparison: Comparison }) {
  const byState = new Map<DeviceState, typeof comparison.missing>();
  for (const device of comparison.missing) {
    const list = byState.get(device.state);
    if (list) list.push(device);
    else byState.set(device.state, [device]);
  }

  return (
    <section className={styles.missing} aria-label="Devices with no output for this command">
      <h4 className={styles.subheading}>
        {plural(comparison.missing.length, 'device')} had nothing to compare
      </h4>
      {[...byState.entries()].map(([state, devices]) => {
        const words = stateWords(state);
        return (
          <div key={state} className={styles.missingGroup}>
            <StatusPill tone={words.tone} icon={words.icon}>
              {words.label}
            </StatusPill>
            <p className={styles.hint}>{words.explain}</p>
            <ul>
              {devices.map((device) => (
                <li key={device.device_id}>
                  <strong>{device.name}</strong>
                  {given(device.reason) && <span className={styles.reason}> — {device.reason}</span>}
                </li>
              ))}
            </ul>
          </div>
        );
      })}
    </section>
  );
}
