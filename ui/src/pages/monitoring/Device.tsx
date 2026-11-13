/**
 * One device, as monitoring sees it: is it answering, what did it say, and
 * what is each of its ports doing.
 *
 * When the device is down this page says so once, at the top, and says why
 * everything else is quiet: the backend deliberately does not raise interface
 * or threshold alerts for a device that is not answering — one cause, one
 * alert — so a page that listed thirty dead ports as thirty problems would be
 * arguing with it.
 */

import { ArrowLeft, ExternalLink, OctagonAlert, Waves } from 'lucide-react';
import { useState } from 'react';
import { Link } from 'react-router';
import { useRpc } from '@/api/hooks';
import type { DeviceRow } from '@/api/types/core';
import type { Alert, MonitorDeviceView, SeriesSummary, SourceStatus } from '@/api/types/monitor';
import { Page } from '@/app/Page';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Card } from '@/design/Card';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { KeyValue, type KeyValueItem } from '@/design/KeyValue';
import { Panel } from '@/design/Panel';
import { Skeleton } from '@/design/Skeleton';
import { StatusDot } from '@/design/StatusDot';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { TimeAgo } from '@/design/TimeAgo';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { EMPTY, formatCount } from '@/lib/format';
import { formatDateTime } from '@/lib/time';
import { AlertDrawer } from './AlertDrawer';
import { usePollNow } from './data';
import { InterfaceTable } from './Interfaces';
import { MetricChart } from './MetricChart';
import styles from './Monitoring.module.css';
import { DEFAULT_RANGE, range } from './series';
import { RangePicker } from './RangePicker';
import { DEVICE_STATUS, deviceStatus, metricFormat, metricLabel, severityLabel, severityTone } from './vocab';

/** The device-level series worth a chart of their own, in this order. */
const DEVICE_CHARTS = ['icmp.rtt_ms', 'cpu.pct', 'mem.pct', 'icmp.loss_pct', 'temp.c', 'up'];

export interface DeviceViewProps {
  deviceId: string;
  onBack: () => void;
}

export function DeviceView({ deviceId, onBack }: DeviceViewProps) {
  const [rangeId, setRangeId] = useState(DEFAULT_RANGE);
  const [openAlert, setOpenAlert] = useState<Alert | null>(null);
  const poll = usePollNow();

  const device = useRpc<DeviceRow>('devices.get', { id: deviceId }, { invalidateOn: ['devices.changed'] });
  const view = useRpc<MonitorDeviceView>(
    'monitor.device',
    { device_id: deviceId },
    { invalidateOn: ['monitor.polled', 'devices.state'] },
  );
  const series = useRpc<SeriesSummary[]>(
    'metrics.series',
    { device_id: deviceId },
    { invalidateOn: ['monitor.polled'] },
  );
  const alerts = useRpc<Alert[]>(
    'alerts.list',
    { device_id: deviceId, state: 'open', limit: 100 },
    { invalidateOn: ['alerts.changed'] },
  );

  if (view.isError || device.isError) {
    return (
      <Page>
        <Toolbar>
          <Button size="sm" icon={ArrowLeft} onClick={onBack}>
            Back
          </Button>
        </Toolbar>
        <ErrorState
          title="This device could not be read"
          error={view.error ?? device.error}
          onRetry={() => {
            void view.refetch();
            void device.refetch();
          }}
        />
      </Page>
    );
  }

  const span = range(rangeId);
  const since = Math.floor(Date.now() / 30_000) * 30_000 - span.span;
  const has = new Set((series.data ?? []).map((entry) => entry.metric));
  const sensors = (series.data ?? []).filter((entry) => entry.metric === 'temp.c').map((entry) => entry.instance);
  const down = view.data?.down ?? false;
  const open = alerts.data ?? [];
  const cause = open.find((alert) => alert.rule_name.toLowerCase().includes('down')) ?? open[0];
  const status = device.data ? DEVICE_STATUS[deviceStatus(device.data)] : undefined;

  const health: KeyValueItem[] = [
    { label: 'CPU', value: reading(view.data?.health.cpu_pct, 'cpu.pct') },
    { label: 'Memory', value: reading(view.data?.health.mem_pct, 'mem.pct') },
    { label: 'Hottest sensor', value: reading(view.data?.health.temp_c, 'temp.c') },
    { label: 'Uptime', value: reading(view.data?.health.uptime_s, 'uptime_s') },
    { label: 'Round-trip', value: reading(view.data?.state.rtt_ms, 'icmp.rtt_ms') },
    {
      label: 'Last answered',
      value: view.data?.state.last_seen == null ? 'Never' : formatDateTime(view.data.state.last_seen),
    },
    {
      label: 'Last polled',
      value: view.data?.last_poll_at == null ? 'Not yet' : formatDateTime(view.data.last_poll_at),
    },
    ...(view.data?.state.last_error ? [{ label: 'Last error', value: view.data.state.last_error }] : []),
  ];

  return (
    <Page>
      <div className={styles.deviceHead}>
        <Button size="sm" icon={ArrowLeft} onClick={onBack}>
          Back
        </Button>
        <h2 className={styles.deviceName}>{device.data?.name ?? deviceId}</h2>
        {status && <StatusPill tone={status.tone}>{status.label}</StatusPill>}
        {device.data?.lab && <LabPill />}
        {device.data?.address && <span className={styles.muted}>{device.data.address}</span>}
        <ToolbarSpacer />
        <Button
          size="sm"
          icon={Waves}
          loading={poll.pending}
          onClick={() => void poll.run([deviceId], device.data?.name ?? 'this device')}
        >
          Poll now…
        </Button>
        <Link className={styles.deviceLink} to={`/devices/${encodeURIComponent(deviceId)}`}>
          <Icon icon={ExternalLink} size={13} />
          Open in Devices
        </Link>
      </div>

      {down && (
        <Card className={styles.cause} padding="sm">
          <Icon icon={OctagonAlert} size={18} className={styles.causeIcon} />
          <div>
            <div className={styles.causeTitle}>
              {cause ? cause.title : `${device.data?.name ?? 'This device'} is not answering`}
            </div>
            <p className={styles.causeText}>
              {sentence(view.data?.state.last_error) ?? 'Several polls in a row went unanswered.'}
            </p>
            <p className={styles.causeText}>
              While a device is down its interface and threshold alerts are held back, so this is the one
              alert you get for it — the ports below are what was last read, not thirty separate problems.
            </p>
          </div>
          {cause && (
            <Button size="sm" onClick={() => setOpenAlert(cause)}>
              Open the alert
            </Button>
          )}
        </Card>
      )}

      <div className={styles.columns}>
        <Panel
          title="Health"
          actions={
            view.data?.health.at ? (
              <span className={styles.count}>
                read <TimeAgo at={view.data.health.at} />
              </span>
            ) : undefined
          }
        >
          {view.isPending ? <Skeleton shape="block" height={160} /> : <KeyValue items={health} />}
        </Panel>

        <Panel title="Where the numbers come from">
          <div className={styles.sources}>
            {(['icmp', 'snmp', 'cli'] as const).map((kind) => (
              <Source key={kind} kind={kind} source={view.data?.sources[kind]} lab={view.data?.lab ?? false} />
            ))}
          </div>
          <p className={styles.hint}>
            A source that is switched off reads nothing; a source that failed says what went wrong. Neither is
            a measurement of zero.
          </p>
        </Panel>

        <Panel
          title="Open alerts"
          actions={<span className={styles.count}>{formatCount(open.length)}</span>}
        >
          {open.length === 0 ? (
            <p className={styles.hint}>Nothing is open for this device.</p>
          ) : (
            <ul className={styles.matches}>
              {open.map((alert) => (
                <li key={alert.id} className={styles.match}>
                  <span className={styles.cell}>
                    <Badge tone={severityTone(alert.severity)}>{severityLabel(alert.severity)}</Badge>
                    <button type="button" className={styles.linkButton} onClick={() => setOpenAlert(alert)}>
                      {alert.title}
                    </button>
                  </span>
                  <TimeAgo at={alert.started_at} className={styles.muted} />
                </li>
              ))}
            </ul>
          )}
        </Panel>
      </div>

      <Toolbar className={styles.filters}>
        <RangePicker value={rangeId} onChange={setRangeId} />
        <span className={styles.count}>
          {series.isPending
            ? ''
            : `${formatCount(series.data?.length ?? 0)} series recorded for this device`}
        </span>
        <ToolbarSpacer />
      </Toolbar>

      <div className={styles.charts}>
        {DEVICE_CHARTS.filter((metric) => has.has(metric)).map((metric) =>
          metric === 'temp.c' ? (
            <MetricChart
              key={metric}
              title={metricLabel(metric)}
              subtitle={`${formatCount(sensors.length)} sensors`}
              lines={sensors.map((sensor) => ({ device_id: deviceId, metric, instance: sensor, label: sensor }))}
              since={since}
              step_s={span.step_s}
            />
          ) : (
            <MetricChart
              key={metric}
              title={metricLabel(metric)}
              lines={[{ device_id: deviceId, metric, instance: '' }]}
              since={since}
              step_s={span.step_s}
            />
          ),
        )}
        {series.data?.length === 0 && (
          <p className={styles.hint}>
            Nothing has been recorded for this device yet. The first poll writes its reachability; readings
            follow once SNMP or the command line answers.
          </p>
        )}
      </div>

      <Panel title="Interfaces" padding="none">
        <InterfaceTable deviceId={deviceId} compact />
      </Panel>

      {openAlert && (
        <AlertDrawer
          alert={open.find((alert) => alert.id === openAlert.id) ?? openAlert}
          onClose={() => setOpenAlert(null)}
        />
      )}
    </Page>
  );
}

/** A device's own error message, ended properly so it can sit in a paragraph. */
function sentence(text: string | null | undefined): string | undefined {
  if (!text) return undefined;
  return /[.!?]$/.test(text.trim()) ? text.trim() : `${text.trim()}.`;
}

function reading(value: number | null | undefined, metric: string) {
  if (value == null) {
    return (
      <span className={styles.missing} title="Nothing was read for this: a gap, not a zero.">
        {EMPTY}
      </span>
    );
  }
  return metricFormat(metric)(value);
}

const SOURCE_NAME = { icmp: 'ICMP', snmp: 'SNMP', cli: 'CLI' } as const;

function Source({
  kind,
  source,
  lab,
}: {
  kind: 'icmp' | 'snmp' | 'cli';
  source: SourceStatus | undefined;
  lab: boolean;
}) {
  if (!source) return null;
  const name = kind === 'icmp' && lab ? 'Simulator' : SOURCE_NAME[kind];
  const tone = !source.enabled ? 'idle' : source.ok == null ? 'idle' : source.ok ? 'ok' : 'crit';
  const text = !source.enabled
    ? 'Off'
    : source.ok == null
      ? 'Not tried'
      : source.ok
        ? (source.via ?? 'Answering')
        : 'Failed';
  return (
    <span className={styles.source} title={source.error ?? (source.at ? `Last tried ${formatDateTime(source.at)}` : undefined)}>
      <StatusDot tone={tone} label={`${name}: ${text}`} />
      <span className={styles.sourceName}>{name}</span>
      <span className={styles.muted}>{text}</span>
    </span>
  );
}
