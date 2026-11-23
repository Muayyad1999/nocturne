/**
 * The receivers: syslog, SNMP traps and flows.
 *
 * One card each, straight from `logs.status`. A card says whether its
 * receiver is listening and — when it is not — which of three things that
 * is: switched off, unable to bind (with the engine's own reason), or not in
 * this build. It shows the counters the engine gives for that receiver, in
 * that receiver's units, and nothing the engine does not give
 * (`receivers.ts`). The numbers are since the application started.
 *
 * The switch and the port are the `logs` settings section, changed with
 * `settings.set`; a receiver follows its settings by itself. "Restart
 * receivers" (`logs.restart`) is for the one case where it cannot: whatever
 * held the port has gone, and the receiver that failed to bind should try
 * again. One that is running is left alone.
 *
 * Opening a port is the user's decision, so everything is off until somebody
 * switches it on here — and where a trap or flow receiver is on, the card
 * says what anybody who can reach that port is able to do.
 */

import { RotateCw, Save, TriangleAlert } from 'lucide-react';
import { useState } from 'react';
import type { Device } from '@/api/types/core';
import type { LogsSettings, LogsStatus, Rejection } from '@/api/types/logs';
import { Button } from '@/design/Button';
import { Card } from '@/design/Card';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { Panel } from '@/design/Panel';
import { Skeleton } from '@/design/Skeleton';
import { StatusPill } from '@/design/StatusPill';
import { Switch } from '@/design/Switch';
import { TimeAgo } from '@/design/TimeAgo';
import { EMPTY, formatBytes, formatCount, plural } from '@/lib/format';
import { formatTimestamp } from '@/lib/time';
import { useDevices, useLogSettings, useRestartReceivers, useSetLogSettings } from './data';
import { DeviceSetup } from './DeviceSetup';
import { NOT_VERIFIED } from './flows';
import styles from './Logs.module.css';
import type { Capabilities } from './panes';
import {
  SETTING_FIELDS,
  TRAPS_NOT_VERIFIED,
  draftOf,
  enablePatch,
  receiverCards,
  settingChange,
  unitFor,
  wouldBind,
  type Counter,
  type ReceiverCard,
  type SettingDraft,
} from './receivers';

export interface ReceiversPaneProps {
  capabilities: Capabilities;
  status: { data: LogsStatus | undefined; isError: boolean; error: unknown; refetch: () => unknown };
}

const SWITCH_LABEL = {
  syslog: 'Receive syslog',
  traps: 'Receive SNMP traps',
  flows: 'Collect flows',
} as const;

export function ReceiversPane({ capabilities, status }: ReceiversPaneProps) {
  const settings = useLogSettings(capabilities.settingsRead);
  const devices = useDevices(capabilities.devices);
  const restart = useRestartReceivers();

  if (status.isError) {
    return (
      <ErrorState
        title="The receivers could not be read"
        error={status.error}
        onRetry={() => void status.refetch()}
      />
    );
  }
  if (!status.data) {
    return (
      <div className={styles.cards}>
        <Skeleton shape="block" height={280} />
        <Skeleton shape="block" height={280} />
        <Skeleton shape="block" height={280} />
      </div>
    );
  }

  const cards = receiverCards(status.data);
  const failed = cards.filter((card) => card.state === 'failed');

  return (
    <div className={styles.paneBody}>
      <div className={styles.paneHead}>
        <p className={styles.paneLead}>
          Everything is off until it is switched on here: opening a listening port on this machine is
          your decision. The counters are since Nocturne started, not since for ever.
        </p>
        {capabilities.restart && (
          <Button
            icon={RotateCw}
            loading={restart.pending}
            onClick={restart.run}
            title="Applies the receiver settings again. A receiver that could not bind is tried again; one that is running is left alone."
          >
            Restart receivers
          </Button>
        )}
      </div>

      {failed.length > 0 && capabilities.restart && (
        <p className={styles.note} role="note">
          {failed.map((card) => card.title).join(' and ')} could not open {failed.length === 1 ? 'its' : 'their'}{' '}
          port. Once whatever holds it has gone, “Restart receivers” tries again.
        </p>
      )}

      <div className={styles.cards}>
        {cards.map((card) => (
          <ReceiverCardView
            key={card.id}
            card={card}
            status={status.data as LogsStatus}
            settings={settings.data}
            devices={devices.data ?? []}
            capabilities={capabilities}
          />
        ))}
      </div>

      <StoreCard status={status.data} />

      {(capabilities.deviceSetup || capabilities.pull) && (
        <Panel title="What a device has to be told" storageId="logs.receivers.setup" collapsible>
          <p className={styles.panelLead}>
            The configuration lines for one device’s own platform, naming the ports above and this
            machine’s address as that device would reach it. They are text: nothing here applies
            them. Flow export has no lines here yet — point the exporter at the flow collector’s
            port by hand.
          </p>
          <DeviceSetup
            id="logs-receivers-setup"
            devices={devices.data ?? []}
            show="both"
            capabilities={capabilities}
          />
        </Panel>
      )}
    </div>
  );
}

interface CardViewProps {
  card: ReceiverCard;
  status: LogsStatus;
  settings: LogsSettings | undefined;
  devices: readonly Device[];
  capabilities: Capabilities;
}

function ReceiverCardView({ card, status, settings, devices, capabilities }: CardViewProps) {
  const change = useSetLogSettings();
  const opens = wouldBind(card.id, settings);
  const absent = card.state === 'absent';

  return (
    <Card className={styles.receiver} role="region" aria-label={`${card.title} receiver`} data-state={card.state}>
      <header className={styles.receiverHead}>
        <div className={styles.receiverTitle}>
          <h3 className={styles.cardTitle}>{card.title}</h3>
          <StatusPill tone={card.tone}>{card.stateLabel}</StatusPill>
        </div>
        {!absent && capabilities.settingsWrite && (
          <Switch
            checked={card.enabled}
            aria-busy={change.isPending || undefined}
            className={change.isPending ? styles.busySwitch : undefined}
            aria-label={SWITCH_LABEL[card.id]}
            onChange={(event) => {
              if (change.isPending) return;
              change.mutate({ section: 'logs', value: enablePatch(card.id, event.target.checked) });
            }}
          />
        )}
      </header>

      <p className={styles.receiverWhat}>
        {card.what}
        {card.state === 'listening' ? (
          <>
            {' '}
            on <span className={styles.monoText}>{card.listening.join(', ')}</span>
          </>
        ) : card.state === 'off' && opens !== null ? (
          <> — switching it on opens {opens}</>
        ) : null}
      </p>

      {card.error !== null && (
        <p className={styles.receiverError} role="alert">
          <Icon icon={TriangleAlert} size={12} /> {card.error}
        </p>
      )}

      {card.caution !== null && <p className={styles.caution}>{card.caution}</p>}

      {!absent && (
        <>
          <Counters counters={card.counters} label={`${card.title} counters`} />

          {card.lastAt !== undefined && (
            <p className={styles.receiverLast}>
              {card.lastAt === null ? (
                'Nothing has arrived since Nocturne started.'
              ) : (
                <>
                  Last arrival <TimeAgo at={card.lastAt} />.
                </>
              )}
            </p>
          )}

          {card.drops.length > 0 && (
            <div className={styles.subBlock}>
              <h4 className={styles.subhead}>Dropped flow records, by reason</h4>
              <Counters counters={card.drops} label="Dropped flow records by reason" />
            </div>
          )}

          {card.memory.length > 0 && (
            <div className={styles.subBlock}>
              <h4 className={styles.subhead}>Memory — nothing of a flow is stored</h4>
              <Counters counters={card.memory} label="Flow memory" />
              <p className={styles.receiverLast}>
                {card.oldestAt == null
                  ? 'No bucket is held.'
                  : `The oldest bucket starts at ${formatTimestamp(card.oldestAt)}.`}{' '}
                Everything here is lost when Nocturne restarts.
              </p>
            </div>
          )}

          {card.id === 'traps' && (
            <TrapPolicy status={status} devices={devices} capabilities={capabilities} />
          )}

          {card.lab !== null && <p className={styles.receiverLab}>{card.lab}</p>}
          {card.id === 'traps' && <p className={styles.receiverLab}>{TRAPS_NOT_VERIFIED}</p>}
          {card.id === 'flows' && <p className={styles.receiverLab}>{NOT_VERIFIED}</p>}

          {capabilities.settingsRead && capabilities.settingsWrite && settings && (
            <PortForm key={JSON.stringify(settings[card.id])} id={card.id} settings={settings} title={card.title} />
          )}
        </>
      )}
    </Card>
  );
}

function Counters({ counters, label }: { counters: readonly Counter[]; label: string }) {
  if (counters.length === 0) return null;
  return (
    <dl className={styles.counters} aria-label={label}>
      {counters.map((entry) => (
        <div
          key={entry.id}
          className={styles.counter}
          data-counter={entry.id}
          data-attention={(entry.attention && entry.value > 0) || undefined}
          title={entry.hint}
        >
          <dt>{entry.label}</dt>
          <dd>
            {entry.format === 'bytes' ? formatBytes(entry.value) : formatCount(entry.value)}
            {entry.unit && <span className={styles.counterUnit}> {unitFor(entry)}</span>}
            {entry.of !== undefined && (
              <span className={styles.counterUnit}>
                {' '}
                of {entry.format === 'bytes' ? formatBytes(entry.of) : formatCount(entry.of)}
              </span>
            )}
          </dd>
        </div>
      ))}
    </dl>
  );
}

function TrapPolicy({
  status,
  devices,
  capabilities,
}: Pick<CardViewProps, 'status' | 'devices' | 'capabilities'>) {
  const change = useSetLogSettings();
  const traps = status.traps;
  const names = new Map(devices.map((device) => [device.id, device.name]));

  return (
    <div className={styles.subBlock}>
      {typeof traps.require_known_community === 'boolean' &&
        (capabilities.settingsWrite ? (
          <Switch
            checked={traps.require_known_community}
            aria-busy={change.isPending || undefined}
            className={change.isPending ? styles.busySwitch : undefined}
            onChange={(event) => {
              if (change.isPending) return;
              change.mutate({
                section: 'logs',
                value: { traps: { require_known_community: event.target.checked } },
              });
            }}
          >
            Keep a trap only when it carries the community stored for its device
          </Switch>
        ) : (
          <p className={styles.receiverLast}>
            {traps.require_known_community
              ? 'A trap is kept only when it carries the community stored for its device.'
              : 'Any community is accepted from a device in the inventory.'}
          </p>
        ))}
      <p className={styles.receiverLast}>
        {traps.require_known_community
          ? 'The community is compared with the SNMP credential of the device the datagram came from, and then dropped: it is never stored or shown.'
          : 'Switched off: any community is taken from a known device, and an unknown source follows the “unknown sources” setting.'}
      </p>

      {Array.isArray(traps.rejections) && traps.rejections.length > 0 && (
        <>
          <h4 className={styles.subhead}>Addresses that were refused, most recent first</h4>
          {/* A list, not a table: the reason is a sentence and has to be read
              whole, in a card that is a third of the screen wide. */}
          <ul className={styles.rejections} aria-label="Trap sources that were refused">
            {traps.rejections.map((rejection: Rejection) => (
              <li key={`${rejection.address}|${rejection.reason}`}>
                <span className={styles.rejectionHead}>
                  <span className={styles.monoText}>{rejection.address}</span>
                  <span className={styles.muted}>
                    {rejection.device_id == null
                      ? 'no device claims it'
                      : (names.get(rejection.device_id) ?? rejection.device_id)}
                    {' · '}
                    {plural(rejection.count, 'datagram')}
                    {' · '}
                    <TimeAgo at={rejection.last_at} />
                  </span>
                </span>
                {/* The engine's own sentence. It never holds the community that was tried. */}
                <span>{rejection.reason}</span>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

function PortForm({
  id,
  settings,
  title,
}: {
  id: ReceiverCard['id'];
  settings: LogsSettings;
  title: string;
}) {
  const apply = useSetLogSettings();
  const [draft, setDraft] = useState<SettingDraft>(() => draftOf(id, settings));
  const { patch, errors } = settingChange(id, draft, settings);

  return (
    <form
      className={styles.portForm}
      aria-label={`${title} port and address`}
      onSubmit={(event) => {
        event.preventDefault();
        if (patch) apply.mutate({ section: 'logs', value: patch });
      }}
    >
      {SETTING_FIELDS[id].map((field) => (
        <Field key={field.key} label={field.label} hint={field.hint} error={errors[field.key]}>
          <Input
            size="sm"
            mono
            inputMode={field.kind === 'port' ? 'numeric' : undefined}
            value={draft[field.key] ?? ''}
            onChange={(event) => setDraft({ ...draft, [field.key]: event.target.value })}
          />
        </Field>
      ))}
      <Button type="submit" size="sm" icon={Save} loading={apply.isPending} disabled={patch === null}>
        Apply
      </Button>
    </form>
  );
}

function StoreCard({ status }: { status: LogsStatus }) {
  const store = status.store;
  const facts: { label: string; value: string; hint?: string }[] = [
    {
      label: 'Stored from devices',
      value: `${formatCount(store.rows)} rows`,
      hint: 'Syslog lines and traps together. The audit trail and the signals are not in this number.',
    },
    { label: 'Oldest', value: store.oldest_at == null ? 'Nothing stored' : formatTimestamp(store.oldest_at) },
    {
      label: 'Kept for',
      value: store.retention_days === 0 ? 'Until the row cap' : `${formatCount(store.retention_days)} days`,
      hint: 'The age limit also applies to the audit trail, which nothing else may delete.',
    },
    { label: 'Row cap', value: formatCount(store.max_rows) },
    {
      label: 'Last pruned',
      value: store.last_prune_at == null ? 'Not since Nocturne started' : formatTimestamp(store.last_prune_at),
    },
    { label: 'Removed by age', value: formatCount(store.pruned_by_age) },
    { label: 'Removed by the row cap', value: formatCount(store.pruned_by_rows) },
    {
      label: 'Addresses no device claims',
      value: typeof status.unattributed_sources === 'number' ? formatCount(status.unattributed_sources) : EMPTY,
    },
  ];
  return (
    <Panel title="The store" storageId="logs.receivers.store" collapsible>
      <dl className={styles.counters} aria-label="Log store">
        {facts.map((fact) => (
          <div key={fact.label} className={styles.counter} title={fact.hint}>
            <dt>{fact.label}</dt>
            <dd>{fact.value}</dd>
          </div>
        ))}
      </dl>
    </Panel>
  );
}
