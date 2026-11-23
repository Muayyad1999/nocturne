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
 *
 * A trap is not a line and is not shown as one. Its body is laid out field by
 * field, every value as text: the notification, how it was sent, the address
 * a v1 agent *claims* next to the one the datagram came from, and each
 * binding by name. It has no severity, and the drawer says "none" — which is
 * a different statement from a syslog line that came without a priority.
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
import { bindingName, omittedNote, trapFacts, trapOf, type TrapView } from './trap';

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
  const trap = trapOf(entry);
  if (trap !== null) {
    return (
      <TrapDrawer
        entry={entry}
        trap={trap}
        deviceName={deviceName}
        onClose={onClose}
        onContext={onContext}
      />
    );
  }
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
    {
      label: 'Sent from',
      value:
        entry.source !== '' ? (
          entry.source
        ) : via === 'lab' ? (
          <span className={styles.muted}>no address — a simulated device sends from none</span>
        ) : null,
      mono: entry.source !== '',
    },
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
      <div className={styles.form}>
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
      </div>
    </Drawer>
  );
}

interface TrapDrawerProps extends Omit<EntryDrawerProps, 'entry'> {
  entry: LogEntry;
  trap: TrapView;
}

function TrapDrawer({ entry, trap, deviceName, onClose, onContext }: TrapDrawerProps) {
  const facts: KeyValueItem[] = [
    { label: 'Received', value: formatTimestamp(entry.ts), mono: true },
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
    {
      label: 'Severity',
      value: <span className={styles.muted}>none — an SNMP notification carries no severity</span>,
    },
    ...trapFacts(trap, entry.source).map((fact) => ({
      label: fact.label,
      value:
        fact.value === null ? null : fact.note === undefined ? (
          fact.value
        ) : (
          <span className={styles.factWithNote}>
            <span className={fact.mono ? styles.monoText : undefined}>{fact.value}</span>
            <span className={styles.muted}>{fact.note}</span>
          </span>
        ),
      mono: fact.mono && fact.note === undefined,
    })),
    { label: 'Stream', value: entry.stream },
  ];
  const omitted = omittedNote(trap);

  return (
    <Drawer
      open
      onClose={onClose}
      title={trap.name ?? 'SNMP notification'}
      description={`${deviceName ?? entry.source} · ${formatTimestamp(entry.ts)}`}
      size="lg"
      footer={
        <Button icon={Crosshair} onClick={() => onContext(entry)}>
          Show the minute around this
        </Button>
      }
    >
      <div className={styles.form}>
        <p className={styles.clockNote}>
          <Badge tone="neutral" variant="outline">
            <Clock size={11} aria-hidden="true" /> receipt time
          </Badge>{' '}
          This time is when Nocturne received the datagram. A trap carries its sender’s uptime, not a
          date.
        </p>

        <CodeBlock
          code={entry.message}
          title="The message Nocturne wrote for this notification"
          copyable
          wrap
        />

        <KeyValue items={facts} />

        <section aria-label="Bindings" className={styles.bindings}>
          <h4 className={styles.subhead}>
            {trap.bindings.length === 0
              ? 'No bindings'
              : `${formatCount(trap.bindings.length)} ${trap.bindings.length === 1 ? 'binding' : 'bindings'}, in the order they were sent`}
          </h4>
          {trap.bindings.length > 0 && (
            <table className={styles.bindingTable}>
              <thead>
                <tr>
                  <th scope="col">Object</th>
                  <th scope="col">Value</th>
                  <th scope="col">OID</th>
                </tr>
              </thead>
              <tbody>
                {trap.bindings.map((binding, index) => {
                  const name = bindingName(binding);
                  return (
                    // Bindings can repeat an object, so the position is the key.
                    <tr key={`${index}-${binding.oid}`}>
                      <td>
                        {name.named ? (
                          name.text
                        ) : (
                          <span className={styles.muted} title="The dictionary has no name for this object">
                            no name
                          </span>
                        )}
                      </td>
                      <td>
                        {binding.value === '' ? <span className={styles.muted}>(empty)</span> : binding.value}
                        {binding.interface !== undefined && (
                          <span className={styles.muted}> — {binding.interface}</span>
                        )}
                      </td>
                      <td>{binding.oid}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
          {omitted !== null && <p className={styles.muted}>{omitted}</p>}
        </section>

        <p className={styles.muted}>
          The community this datagram was sent with is not stored and is shown nowhere.{' '}
          <code className={styles.inlineCode}>&lt;community&gt;</code> stands wherever a binding repeated
          it.
        </p>
      </div>
    </Drawer>
  );
}
