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
 * under the block says so. The hint is about whichever receiver fills the
 * stream being looked at: the syslog lines for the syslog stream, the SNMP
 * notification lines for the trap stream (`DeviceSetup.tsx`).
 *
 * The other half is the addresses that *have* sent and that no device claims.
 * A line filed against no device is still a line, and the inventory entry that
 * would claim it is one click away.
 */

import { CircleAlert, PlugZap, Radio, TriangleAlert } from 'lucide-react';
import type { Device } from '@/api/types/core';
import type { LogSource } from '@/api/types/logs';
import { Button } from '@/design/Button';
import { Card } from '@/design/Card';
import { Icon } from '@/design/Icon';
import { Panel } from '@/design/Panel';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { formatCount, plural } from '@/lib/format';
import type { Verdict } from './coverage';
import { useLogSettings, useSetLogSettings } from './data';
import { DeviceSetup } from './DeviceSetup';
import styles from './Logs.module.css';
import type { Capabilities } from './panes';
import { enablePatch, wouldBind } from './receivers';

export interface SetupHintProps {
  /** Which receiver, and what is wrong with it. */
  verdict: Verdict;
  capabilities: Capabilities;
  devices: readonly Device[];
  /** The device the screen is filtered to, if any: the one to explain first. */
  deviceId: string | null;
}

export function SetupHint({ verdict, capabilities, devices, deviceId }: SetupHintProps) {
  const settings = useLogSettings(capabilities.settingsRead);
  const enable = useSetLogSettings('The receiver could not be switched on');
  const tone = verdict.kind === 'failed' ? 'crit' : 'warn';
  const opens = wouldBind(verdict.receiver, settings.data);

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
        {verdict.kind === 'off' && capabilities.settingsWrite && (
          <Button
            variant="primary"
            icon={PlugZap}
            loading={enable.isPending}
            onClick={() =>
              enable.mutate({ section: 'logs', value: enablePatch(verdict.receiver, true) })
            }
            title={opens === null ? 'Opens a listening socket on this machine' : `Opens ${opens}`}
          >
            Turn the receiver on
          </Button>
        )}
      </div>

      <DeviceSetup
        id="logs-setup"
        devices={devices}
        deviceId={deviceId}
        show={verdict.receiver === 'traps' ? 'traps' : 'syslog'}
        capabilities={capabilities}
      />
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
