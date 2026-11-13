/**
 * The Health tab: every monitored device, problems first.
 *
 * Reachability comes from the inventory's own `DeviceState` (monitoring
 * writes it on every poll and the core republishes it as `devices.state`);
 * CPU, memory and loss come from one `metrics.top` call each, which is the
 * only way to get the newest value of a metric across every device without
 * asking per device.
 *
 * The alert column is the open alerts of `alerts.list`, narrowed by device —
 * never a count of problems worked out here. A device that is down has its
 * interface and threshold alerts suppressed by the backend on purpose, and a
 * number invented in the browser would quietly undo that.
 */

import { ServerOff } from 'lucide-react';
import { useMemo, useState } from 'react';
import { Link } from 'react-router';
import { useRpc } from '@/api/hooks';
import type { DeviceRow } from '@/api/types/core';
import type { MetricSeries, TopEntry } from '@/api/types/monitor';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { SearchInput } from '@/design/SearchInput';
import { Sparkline } from '@/design/Sparkline';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { Switch } from '@/design/Switch';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { EMPTY, formatCount } from '@/lib/format';
import { formatDateTime } from '@/lib/time';
import { alertsByDevice, useDevices, useOpenAlerts } from './data';
import styles from './Monitoring.module.css';
import { alignSeries, range } from './series';
import { DEVICE_STATUS, deviceStatus, metricFormat, severityLabel, severityTone, worstSeverity } from './vocab';

/**
 * How far back `metrics.top` looks. A day, not an hour: a reading from this
 * morning is still a reading, and the cell says when it was taken. Beyond
 * that the column says nothing was recorded rather than showing something
 * stale as if it were now.
 */
const RECENT_S = 86_400;
/** `metrics.query_many` takes at most this many series in one call. */
const SPARK_LIMIT = 200;

type LabFilter = 'all' | 'hide' | 'only';

const LAB_FILTERS: readonly { id: LabFilter; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'hide', label: 'No lab' },
  { id: 'only', label: 'Lab only' },
];

/** Newest value of one metric per device, from `metrics.top`. */
function useLatest(metric: string) {
  const query = useRpc<TopEntry[]>(
    'metrics.top',
    { metric, limit: 500, within_s: RECENT_S },
    { invalidateOn: ['monitor.polled'] },
  );
  return useMemo(() => {
    const byDevice = new Map<string, TopEntry>();
    for (const entry of query.data ?? []) {
      if (entry.instance === '' && !byDevice.has(entry.device_id)) byDevice.set(entry.device_id, entry);
    }
    return byDevice;
  }, [query.data]);
}

export function HealthTab() {
  const [text, setText] = useState('');
  const [lab, setLab] = useState<LabFilter>('all');
  const [onlyProblems, setOnlyProblems] = useState(false);

  const devices = useDevices();
  const alerts = useOpenAlerts();
  const cpu = useLatest('cpu.pct');
  const memory = useLatest('mem.pct');
  const loss = useLatest('icmp.loss_pct');
  const byDevice = useMemo(() => alertsByDevice(alerts.data), [alerts.data]);

  const needle = text.trim().toLowerCase();
  const rows = (devices.data ?? [])
    .filter((device) => device.monitor.enabled)
    .filter((device) => (lab === 'all' ? true : lab === 'only' ? Boolean(device.lab) : !device.lab))
    .filter((device) => !onlyProblems || deviceStatus(device) !== 'up' || (byDevice.get(device.id)?.length ?? 0) > 0)
    .filter(
      (device) =>
        !needle ||
        [device.name, device.address, device.site, device.role].some((field) =>
          field.toLowerCase().includes(needle),
        ),
    );

  // One call for every sparkline on screen. Beyond what the backend will take
  // in one go the column is left empty rather than fetched device by device.
  const pinged = rows.filter((device) => device.monitor.icmp).slice(0, SPARK_LIMIT);
  const rttWindow = range('1h');
  // Pinned to the minute so the query key is stable between renders.
  const since = Math.floor(Date.now() / 60_000) * 60_000 - rttWindow.span;
  const spark = useRpc<MetricSeries[]>(
    'metrics.query_many',
    {
      series: pinged.map((device) => ({ device_id: device.id, metric: 'icmp.rtt_ms', instance: '' })),
      since,
      step_s: rttWindow.step_s,
    },
    { enabled: pinged.length > 0, invalidateOn: ['monitor.polled'] },
  );
  const sparks = useMemo(() => {
    const aligned = alignSeries(spark.data ?? []);
    const byId = new Map<string, (number | null)[]>();
    (spark.data ?? []).forEach((answer, index) => {
      byId.set(answer.device_id, aligned.values[index] ?? []);
    });
    return byId;
  }, [spark.data]);

  const columns: TableColumn<DeviceRow>[] = [
    {
      id: 'status',
      header: 'Status',
      width: 124,
      required: true,
      // Problems first: down, then never polled, then answering.
      sortValue: (device) => ['down', 'unknown', 'unmonitored', 'up'].indexOf(deviceStatus(device)),
      cell: (device) => {
        const status = DEVICE_STATUS[deviceStatus(device)];
        return (
          <StatusPill tone={status.tone} title={device.state.last_error ?? status.help}>
            {status.label}
          </StatusPill>
        );
      },
    },
    {
      id: 'name',
      header: 'Device',
      required: true,
      sortValue: (device) => device.name,
      cell: (device) => (
        <span className={styles.cell}>
          <Link className={styles.cellText} to={`/monitoring/health?device=${encodeURIComponent(device.id)}`}>
            {device.name}
          </Link>
          {device.lab && <LabPill />}
        </span>
      ),
    },
    {
      id: 'alerts',
      header: 'Open alerts',
      width: 150,
      sortValue: (device) => -(byDevice.get(device.id)?.length ?? 0),
      cell: (device) => {
        const open = byDevice.get(device.id) ?? [];
        if (open.length === 0) return <span className={styles.missing}>None</span>;
        const worst = worstSeverity(open.map((alert) => alert.severity));
        return (
          <span className={styles.cell} title={open.map((alert) => alert.title).join('\n')}>
            <Badge tone={worst ? severityTone(worst) : 'neutral'}>
              {worst ? severityLabel(worst) : 'Open'}
            </Badge>
            {open.length > 1 && <span className={styles.muted}>+{formatCount(open.length - 1)}</span>}
          </span>
        );
      },
    },
    {
      id: 'rtt',
      header: 'Round-trip',
      width: 150,
      sortValue: (device) => device.state.rtt_ms ?? null,
      cell: (device) => {
        if (!device.monitor.icmp) {
          return (
            <span
              className={styles.missing}
              title={
                device.lab
                  ? 'A simulated device is reached through the simulator, not pinged: a lab address is not a real address.'
                  : 'ICMP is switched off for this device, so no round-trip time is measured.'
              }
            >
              Not pinged
            </span>
          );
        }
        const values = sparks.get(device.id);
        return (
          <span className={styles.cell}>
            <span>{metricFormat('icmp.rtt_ms')(device.state.rtt_ms)}</span>
            {values && values.some((value) => value != null) && (
              <Sparkline
                values={values}
                width={64}
                height={16}
                label={`Round-trip time over the last hour, latest ${metricFormat('icmp.rtt_ms')(device.state.rtt_ms)}`}
              />
            )}
          </span>
        );
      },
    },
    {
      id: 'loss',
      header: 'Loss',
      width: 96,
      align: 'right',
      sortValue: (device) => loss.get(device.id)?.value ?? null,
      cell: (device) => <Reading entry={loss.get(device.id)} metric="icmp.loss_pct" never="No echo has been sent to this device in the last day." />,
    },
    {
      id: 'cpu',
      header: 'CPU',
      width: 96,
      align: 'right',
      sortValue: (device) => cpu.get(device.id)?.value ?? null,
      cell: (device) => <Reading entry={cpu.get(device.id)} metric="cpu.pct" never="No CPU reading has been recorded for this device in the last day." />,
    },
    {
      id: 'memory',
      header: 'Memory',
      width: 104,
      align: 'right',
      sortValue: (device) => memory.get(device.id)?.value ?? null,
      cell: (device) => (
        <Reading entry={memory.get(device.id)} metric="mem.pct" never="No memory reading has been recorded for this device in the last day." />
      ),
    },
    {
      id: 'site',
      header: 'Site',
      width: 140,
      defaultHidden: true,
      sortValue: (device) => device.site,
      cell: (device) => <span className={styles.cellText}>{device.site || EMPTY}</span>,
    },
    {
      id: 'last_seen',
      header: 'Last seen',
      width: 120,
      sortValue: (device) => device.state.last_seen ?? null,
      cell: (device) =>
        device.state.last_seen == null ? (
          <span className={styles.missing} title="This device has never answered a poll.">
            Never
          </span>
        ) : (
          <TimeAgo at={device.state.last_seen} />
        ),
    },
  ];

  if (devices.isError) {
    return (
      <ErrorState title="The devices could not be loaded" error={devices.error} onRetry={() => void devices.refetch()} />
    );
  }

  return (
    <>
      <Toolbar className={styles.filters}>
        <SearchInput
          className={styles.search}
          size="sm"
          aria-label="Search devices"
          placeholder="Name, address, site or role"
          value={text}
          onChange={setText}
        />
        <div className={styles.segmented} role="group" aria-label="Lab devices">
          {LAB_FILTERS.map((entry) => (
            <button
              key={entry.id}
              type="button"
              className={styles.segment}
              aria-pressed={lab === entry.id}
              onClick={() => setLab(entry.id)}
            >
              {entry.label}
            </button>
          ))}
        </div>
        <Switch checked={onlyProblems} onChange={(event) => setOnlyProblems(event.target.checked)}>
          Problems only
        </Switch>
        <span className={styles.count}>
          {devices.isPending ? '' : `${formatCount(rows.length)} ${rows.length === 1 ? 'device' : 'devices'}`}
        </span>
        <ToolbarSpacer />
      </Toolbar>

      <Table
        className={styles.table}
        id="monitoring.health"
        aria-label="Device health"
        fill
        columns={columns}
        rows={rows}
        rowKey={(device) => device.id}
        loading={devices.isPending}
        defaultSort={{ columnId: 'status', direction: 'asc' }}
        empty={
          <EmptyState
            size="compact"
            icon={ServerOff}
            title={onlyProblems || needle || lab !== 'all' ? 'Nothing matches' : 'No device is monitored'}
            description={
              onlyProblems || needle || lab !== 'all'
                ? 'No monitored device matches these filters.'
                : 'Monitoring polls the devices that have it switched on. Turn it on for a device from its page in Devices.'
            }
            action={
              onlyProblems || needle || lab !== 'all' ? (
                <Button
                  size="sm"
                  onClick={() => {
                    setText('');
                    setLab('all');
                    setOnlyProblems(false);
                  }}
                >
                  Clear filters
                </Button>
              ) : undefined
            }
          />
        }
      />
    </>
  );
}

/**
 * One reading from `metrics.top`: the value and, when it is not fresh, when
 * it was taken. A device with no reading at all says so — it is not a zero.
 */
function Reading({ entry, metric, never }: { entry: TopEntry | undefined; metric: string; never: string }) {
  if (!entry) {
    return (
      <span className={styles.missing} title={never}>
        {EMPTY}
      </span>
    );
  }
  return <span title={`Measured ${formatDateTime(entry.at)}`}>{metricFormat(metric)(entry.value)}</span>;
}
