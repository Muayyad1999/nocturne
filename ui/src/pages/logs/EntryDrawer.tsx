/**
 * One line, in full, with the two clocks kept apart.
 *
 * The row in the list can only show one time. Here both facts are stated: when
 * this machine received or read the line, and what the device wrote by its own
 * clock, verbatim as text. A device with a wrong clock is ordinary, and the
 * drawer is where that becomes visible instead of confusing.
 *
 * The raw line is the line as it arrived, after redaction (`nocturne-ai`'s
 * redactor runs before anything is stored, so a credential in a log line is
 * never in the store to show). Where the body carries no raw line — a lab
 * device's own syslog, the `audit` and `event` streams — the parsed message is
 * shown and said to be the parsed message.
 */

import { Clock, Crosshair } from 'lucide-react';
import type { LogEntry } from '@/api/types/core';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { Drawer } from '@/design/Drawer';
import { KeyValue, type KeyValueItem } from '@/design/KeyValue';
import { formatCount } from '@/lib/format';
import { formatTimestamp } from '@/lib/time';
import { bodyOf, clockOf, interfaceOf, rawOf, viaOf } from './entry';
import styles from './Logs.module.css';
import { facilityName, severityName } from './query';

const VIA_WORD: Record<string, string> = {
  syslog: 'sent by the device to this receiver',
  pull: 'read out of the device’s own log buffer',
  lab: 'written by a simulated device in a lab',
};

export interface EntryDrawerProps {
  entry: LogEntry | null;
  deviceName: string | null;
  onClose: () => void;
  /** Narrow the range to the minute around this line. */
  onContext: (entry: LogEntry) => void;
}

export function EntryDrawer({ entry, deviceName, onClose, onContext }: EntryDrawerProps) {
  if (entry === null) return null;
  const body = bodyOf(entry);
  const clock = clockOf(entry);
  const raw = rawOf(entry);
  const via = viaOf(entry);
  const port = interfaceOf(entry);

  const facts: KeyValueItem[] = [
    {
      label: clock.shown === 'device' ? 'Time (device’s clock)' : 'Received',
      value: formatTimestamp(entry.ts),
      mono: true,
    },
    {
      label: 'The device wrote',
      value:
        clock.deviceText === null ? (
          <span className={styles.muted}>no timestamp in the line</span>
        ) : (
          clock.deviceText
        ),
      mono: clock.deviceText !== null,
    },
    {
      label: 'Device',
      value:
        deviceName ??
        (entry.device_id == null ? (
          <span className={styles.muted}>no device in the inventory claims {entry.source}</span>
        ) : (
          entry.device_id
        )),
    },
    { label: 'Sent from', value: entry.source, mono: true },
    {
      label: 'Severity',
      value:
        entry.severity == null ? (
          <span className={styles.muted}>none given — not the same as debug</span>
        ) : (
          `${severityName(entry.severity)} (${entry.severity})`
        ),
    },
    {
      label: 'Facility',
      value: body.facility === undefined ? null : `${facilityName(body.facility)} (${body.facility})`,
    },
    { label: 'Mnemonic', value: body.mnemonic ?? null, mono: true },
    { label: 'Interface', value: port, mono: true },
    { label: 'Application', value: body.app ?? null, mono: true },
    {
      label: 'Host name in the line',
      value: body.host ?? null,
      mono: true,
    },
    {
      label: 'Device’s sequence number',
      value: body.sequence === undefined ? null : formatCount(body.sequence),
      mono: true,
    },
    { label: 'Stream', value: entry.stream },
    { label: 'How it got here', value: via === null ? null : VIA_WORD[via] },
  ];

  return (
    <Drawer
      open
      onClose={onClose}
      title={body.mnemonic ?? 'Log line'}
      description={`${deviceName ?? entry.source} · ${formatTimestamp(entry.ts)}`}
      size="lg"
      footer={
        <Button icon={Crosshair} onClick={() => onContext(entry)}>
          Show the minute around this
        </Button>
      }
    >
      <p className={styles.clockNote}>
        <Badge tone={clock.shown === 'device' ? 'warn' : 'neutral'} variant="outline">
          <Clock size={11} aria-hidden="true" /> {clock.shown === 'device' ? 'device clock' : 'receipt time'}
        </Badge>{' '}
        {clock.note}
      </p>

      <CodeBlock
        code={raw.text}
        title={raw.exact ? 'The line as it arrived' : 'The parsed message (no raw line was kept)'}
        copyable
        wrap
      />

      <KeyValue items={facts} />

      {body.host !== undefined && (
        <p className={styles.muted}>
          The host name above is what the sender wrote in the line. Nocturne never attributes a line
          by it — two switches called the same thing in different sites is normal — only by the
          address it came from.
        </p>
      )}
    </Drawer>
  );
}
