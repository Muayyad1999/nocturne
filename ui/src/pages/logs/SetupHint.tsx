/**
 * What to do when nothing is arriving — which is not the same as nothing
 * happening.
 *
 * Most of a campus sends no syslog, because nobody ever told it where to. An
 * empty log screen that says "no results" teaches the reader that their network
 * is quiet. So when `logs.status` says the receiver is off, cannot bind, or has
 * never been sent a line, this says which of those it is and then shows **the
 * exact configuration lines for the chosen device's platform**, from
 * `logs.device_setup` — the real port this installation listens on and the real
 * address of this machine as that device would reach it, not a generic
 * "configure logging on your devices".
 *
 * Those lines are text. `logs.device_setup` applies nothing, and neither does
 * this screen: configuration goes through the change pipeline, and the note
 * under the block says so.
 *
 * The other half is the addresses that *have* sent and that no device claims.
 * A line filed against no device is still a line, and the inventory entry that
 * would claim it is one click away.
 */

import { CircleAlert, Download, PlugZap, Radio, TriangleAlert } from 'lucide-react';
import { useState } from 'react';
import { Link } from 'react-router';
import type { Device } from '@/api/types/core';
import type { LogSource, LogsStatus } from '@/api/types/logs';
import { Button } from '@/design/Button';
import { Card } from '@/design/Card';
import { CodeBlock } from '@/design/CodeBlock';
import { Combobox } from '@/design/Combobox';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { Panel } from '@/design/Panel';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { formatCount, plural } from '@/lib/format';
import type { Verdict } from './coverage';
import { useDeviceSetup, useEnableReceiver, useLogSettings, usePull } from './data';
import styles from './Logs.module.css';

export interface SetupHintProps {
  verdict: Verdict;
  status: LogsStatus;
  devices: readonly Device[];
  /** The device the screen is filtered to, if any: the one to explain first. */
  deviceId: string | null;
}

export function SetupHint({ verdict, status, devices, deviceId }: SetupHintProps) {
  const settings = useLogSettings();
  const enable = useEnableReceiver();
  const pull = usePull();

  // The device to show the lines for: the one being looked at, else the first
  // one whose platform is known, because only then are the lines real.
  const withProfile = devices.filter((device) => device.profile !== '');
  const [chosen, setChosen] = useState<string | null>(null);
  const device = chosen ?? deviceId ?? withProfile[0]?.id ?? devices[0]?.id ?? null;
  const setup = useDeviceSetup(device);
  const port = setup.data?.udp_port ?? settings.data?.syslog.udp_port;

  const tone = verdict.kind === 'failed' ? 'crit' : 'warn';

  return (
    <Card
      className={styles.hint}
      data-tone={tone}
      padding="md"
      role="region"
      aria-label="Log reception"
    >
      <div className={styles.hintHead}>
        <Icon icon={verdict.kind === 'failed' ? CircleAlert : Radio} size={16} />
        <div>
          <strong>{verdict.title}</strong>
          <p className={styles.hintDetail}>{verdict.detail}</p>
          {verdict.error && (
            <p className={styles.hintError}>
              <Icon icon={TriangleAlert} size={12} /> {verdict.error}
            </p>
          )}
        </div>
        {verdict.kind === 'off' && (
          <Button
            variant="primary"
            icon={PlugZap}
            loading={enable.isPending}
            onClick={() =>
              enable.mutate({ section: 'logs', value: { syslog: { enabled: true } } })
            }
            title={
              settings.data
                ? `Opens UDP ${settings.data.syslog.udp_port}${
                    settings.data.syslog.tcp_port === 0
                      ? ''
                      : ` and TCP ${settings.data.syslog.tcp_port}`
                  } on ${settings.data.syslog.bind}`
                : 'Opens a listening socket on this machine'
            }
          >
            Turn the receiver on
          </Button>
        )}
      </div>

      {devices.length === 0 ? (
        <p className={styles.hintDetail}>
          There are no devices yet, so there is no platform to write the lines for.{' '}
          <Link to="/devices">Add a device</Link> and the exact commands for it appear here.
        </p>
      ) : (
        <div className={styles.hintBody}>
          <div className={styles.hintRow}>
            <label className={styles.hintLabel} htmlFor="logs-setup-device">
              Point this device at Nocturne
            </label>
            <Combobox
              id="logs-setup-device"
              className={styles.hintPicker}
              size="sm"
              aria-label="Device to show the syslog commands for"
              value={device}
              onChange={setChosen}
              options={devices.map((entry) => ({
                value: entry.id,
                label: entry.name,
                description: entry.profile === '' ? 'platform not known yet' : entry.profile,
              }))}
              emptyText="No such device"
            />
            <Button
              size="sm"
              icon={Download}
              loading={pull.isPending}
              disabled={device === null}
              onClick={() => device && pull.mutate({ device_id: device })}
              title="Reads the device's own log buffer over the read-only CLI. Nothing is configured and nothing is sent."
            >
              Read its buffer instead
            </Button>
          </div>

          {setup.isError ? (
            <ErrorState
              size="compact"
              title="The lines for this device are not known"
              error={setup.error}
            />
          ) : setup.data ? (
            <>
              <CodeBlock
                code={setup.data.lines.join('\n')}
                title={`${devices.find((entry) => entry.id === device)?.name ?? 'This device'} · ${setup.data.profile} · syslog to ${setup.data.address}${port === undefined ? '' : `:${port}`}`}
                copyable
                lineNumbers
              />
              <p className={styles.hintDetail}>
                {setup.data.address} is {setup.data.address_note}. {setup.data.note}
              </p>
            </>
          ) : null}

          {pull.data && (
            <p className={styles.hintDetail} data-testid="pull-result">
              {pull.data.note === ''
                ? `${pull.data.command}: ${plural(pull.data.read, 'line')} read, ${formatCount(pull.data.stored)} stored, ${formatCount(pull.data.duplicates)} already known, ${formatCount(pull.data.dated_by_device)} dated by the device’s own clock.`
                : pull.data.note}
            </p>
          )}
        </div>
      )}

      {status.traps.implemented === false && (
        <p className={styles.hintFoot}>
          SNMP traps and flow collection are not in this build, so nothing is listening for them —
          they are not switched off, they are absent.
        </p>
      )}
    </Card>
  );
}

const COLUMNS: readonly TableColumn<LogSource>[] = [
  {
    id: 'address',
    header: 'Address',
    cell: (row) => row.address,
    mono: true,
    required: true,
    width: 180,
  },
  {
    id: 'why',
    header: 'Why no device',
    // The engine's own sentence, not a word of ours: it names what it looked
    // at. The whole of it is in the cell's tooltip when the column is narrow.
    cell: (row) => (
      <span className={styles.muted} title={row.reason}>
        {row.reason}
      </span>
    ),
  },
  {
    id: 'count',
    header: 'Lines',
    align: 'right',
    width: 90,
    cell: (row) => formatCount(row.count),
    sortValue: (row) => row.count,
  },
  {
    id: 'dropped',
    header: 'Dropped',
    align: 'right',
    width: 120,
    cell: (row) =>
      row.dropped === 0 ? <span className={styles.muted}>—</span> : formatCount(row.dropped),
    sortValue: (row) => row.dropped,
  },
  {
    id: 'last',
    header: 'Last line',
    width: 140,
    cell: (row) => <TimeAgo at={row.last_at} />,
    sortValue: (row) => row.last_at,
  },
];

export interface SourcesPanelProps {
  sources: readonly LogSource[];
  onPick: (address: string) => void;
}

/** The addresses that have sent and that no device in the inventory claims. */
export function SourcesPanel({ sources, onPick }: SourcesPanelProps) {
  const unclaimed = sources.filter((source) => source.device_id == null);
  if (unclaimed.length === 0) return null;
  return (
    <Panel
      title={`${plural(unclaimed.length, 'address', 'addresses')} no device claims`}
      storageId="logs.sources"
      collapsible
      defaultCollapsed
      padding="none"
    >
      <p className={styles.panelNote}>
        Their lines are stored with no device, so nothing is lost — but nothing on a device page
        will show them either. Add the device to the inventory with the address below and the lines
        are filed from the next one onwards.
      </p>
      <Table
        id="logs.sources"
        columns={COLUMNS}
        rows={unclaimed}
        rowKey={(row) => row.address}
        density="compact"
        onRowActivate={(row) => onPick(row.address)}
        empty="No unclaimed addresses"
        aria-label="Addresses no device claims"
      />
    </Panel>
  );
}
