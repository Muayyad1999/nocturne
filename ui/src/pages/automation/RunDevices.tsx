/**
 * What each device did — grouped by the four outcomes, worst first.
 *
 * The grouping is the point. "Three failed, two were never reached and one
 * was skipped because another run held it" is three different problems with
 * three different answers, and a flat table of red rows would flatten them
 * into one. Each group carries the sentence that says what its outcome means,
 * and each device carries the backend's own words for why.
 */

import { ChevronDown, Table as TableIcon } from 'lucide-react';
import { useState } from 'react';
import type { CommandResult, DeviceResult, DeviceState, Run, StepResult } from '@/api/types/automation';
import { Badge } from '@/design/Badge';
import { CodeBlock } from '@/design/CodeBlock';
import { EmptyState } from '@/design/EmptyState';
import { Icon } from '@/design/Icon';
import { StatusPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { formatCount, formatDuration, plural } from '@/lib/format';
import styles from './Automation.module.css';
import { given } from './given';
import { STATE_ORDER, stateWords } from './vocab';

export interface RunDevicesProps {
  run: Run;
  results: readonly DeviceResult[];
  labDevices: ReadonlySet<string>;
}

export function RunDevices({ run, results, labDevices }: RunDevicesProps) {
  if (results.length === 0) {
    return (
      <EmptyState
        size="compact"
        icon={TableIcon}
        title={given(run.finished_at) ? 'This run has no results' : 'The devices are still being worked on'}
        description={
          !given(run.finished_at)
            ? 'A device’s output is written when the run finishes; what is known so far is in the live view above.'
            : 'The run record kept no per-device result. Its output may have been pruned by the retention setting.'
        }
      />
    );
  }

  const groups = STATE_ORDER.map((state) => ({
    state,
    devices: results.filter((result) => result.state === state),
  })).filter((group) => group.devices.length > 0);

  return (
    <div className={styles.stack}>
      {groups.map((group) => (
        <StateGroup
          key={group.state}
          state={group.state}
          devices={group.devices}
          total={results.length}
          labDevices={labDevices}
        />
      ))}
    </div>
  );
}

function StateGroup({
  state,
  devices,
  total,
  labDevices,
}: {
  state: DeviceState;
  devices: readonly DeviceResult[];
  total: number;
  labDevices: ReadonlySet<string>;
}) {
  const words = stateWords(state);
  return (
    <section className={styles.stateGroup} data-tone={words.tone} aria-label={`${words.group}: ${devices.length} of ${total}`}>
      <header className={styles.stateHead}>
        <StatusPill tone={words.tone} icon={words.icon}>
          {words.group}
        </StatusPill>
        <strong>
          {formatCount(devices.length)} of {plural(total, 'device')}
        </strong>
        <p className={styles.hint}>{words.explain}</p>
      </header>
      <ul className={styles.deviceList}>
        {devices.map((device) => (
          <DeviceRow key={device.device_id} device={device} lab={labDevices.has(device.device_id)} />
        ))}
      </ul>
    </section>
  );
}

function DeviceRow({ device, lab }: { device: DeviceResult; lab: boolean }) {
  const hasBody = device.outputs.length > 0 || device.steps.length > 0;
  const [open, setOpen] = useState(false);
  const words = stateWords(device.state);

  return (
    <li className={styles.deviceRow} data-tone={words.tone}>
      <div className={styles.deviceHead}>
        {hasBody ? (
          <button type="button" className={styles.deviceToggle} aria-expanded={open} onClick={() => setOpen(!open)}>
            <Icon icon={ChevronDown} size={14} className={open ? styles.openChevron : styles.shutChevron} />
            <span className={styles.deviceName}>{device.name}</span>
          </button>
        ) : (
          <span className={styles.deviceToggle}>
            <span className={styles.deviceName}>{device.name}</span>
          </span>
        )}
        {lab && <span className={styles.labTag}>LAB</span>}
        {given(device.change_id) && (
          <Badge tone="accent" title="Drafted as a change; nothing has been applied">
            Change drafted
          </Badge>
        )}
        <span className={styles.spacer} />
        {device.outputs.length > 0 && (
          <span className={styles.muted}>
            {plural(device.outputs.length, 'answer')} · {formatDuration(device.elapsed_ms)}
          </span>
        )}
      </div>

      {given(device.error) && (
        <p className={styles.deviceError}>
          <strong>{device.state === 'unreached' ? 'Never reached:' : 'The device said:'}</strong> {device.error}
        </p>
      )}
      {given(device.reason) && (
        <p className={styles.deviceReason}>
          <strong>Why:</strong> {device.reason}
        </p>
      )}

      {open && (
        <div className={styles.deviceBody}>
          {device.steps.map((step, index) => (
            <Step key={`${index}-${step.name}`} step={step} />
          ))}
          {device.outputs.map((output, index) => (
            <Output key={`${index}-${output.command}`} output={output} />
          ))}
        </div>
      )}
    </li>
  );
}

function Step({ step }: { step: StepResult }) {
  const tone = step.state === 'ok' ? 'ok' : step.state === 'failed' ? 'crit' : 'idle';
  return (
    <section className={styles.step}>
      <header className={styles.stepHead}>
        <StatusPill tone={tone}>{step.state === 'ok' ? 'Done' : step.state === 'failed' ? 'Failed' : 'Skipped'}</StatusPill>
        <strong>{step.name}</strong>
        <span className={styles.hint}>{step.detail}</span>
      </header>
      {step.outputs.map((output, index) => (
        <Output key={`${index}-${output.command}`} output={output} />
      ))}
    </section>
  );
}

/** One command, exactly as it was sent, and exactly what came back. */
function Output({ output }: { output: CommandResult }) {
  const [parsed, setParsed] = useState(false);
  const rows = output.parsed;
  return (
    <div className={styles.output}>
      <div className={styles.outputHead}>
        <code className={styles.sent}>{output.command}</code>
        <span className={styles.spacer} />
        {output.truncated && (
          <Badge tone="warn" title="The output was longer than the cap in Settings → Automation; the marker saying how much was dropped is in the text itself">
            Cut at the cap
          </Badge>
        )}
        <span className={styles.muted}>{formatDuration(output.elapsed_ms)}</span>
        {given(rows) && rows.length > 0 && (
          <button type="button" className={styles.linkButton} onClick={() => setParsed(!parsed)}>
            <Icon icon={TableIcon} size={12} /> {parsed ? 'Show what it printed' : `Show ${plural(rows.length, 'parsed row')}`}
          </button>
        )}
      </div>
      {given(output.error) && (
        <p className={styles.deviceError}>
          <strong>Rejected:</strong> {output.error}
        </p>
      )}
      {parsed && given(rows) ? (
        <ParsedTable rows={rows} command={output.command} />
      ) : (
        output.output !== '' && <CodeBlock code={output.output} maxHeight={320} />
      )}
    </div>
  );
}

function ParsedTable({ rows, command }: { rows: readonly Record<string, string | string[]>[]; command: string }) {
  const keys = [...new Set(rows.flatMap((row) => Object.keys(row)))];
  const columns: TableColumn<Record<string, string | string[]>>[] = keys.map((key, index) => ({
    id: key,
    header: key.replace(/_/g, ' '),
    required: index === 0,
    mono: true,
    sortValue: (row) => String(row[key] ?? ''),
    cell: (row) => {
      const value = row[key];
      return Array.isArray(value) ? value.join(', ') : (value ?? '');
    },
  }));
  return (
    <Table
      aria-label={`Parsed rows of ${command}`}
      columns={columns}
      rows={rows}
      rowKey={(row) => JSON.stringify(row)}
      maxHeight={320}
      empty={null}
    />
  );
}
