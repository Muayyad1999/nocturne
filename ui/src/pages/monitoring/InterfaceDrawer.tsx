/**
 * One interface: what it is, and what it has been doing.
 *
 * Every number here was recorded as a rate over a measured interval. Where
 * monitoring could not measure one it stored nothing, so the charts show a
 * hole and the facts say "not measured" — a port that is quiet and a port
 * nobody could read look nothing alike.
 */

import { useState } from 'react';
import { Link } from 'react-router';
import type { IfStatus } from '@/api/types/monitor';
import { Badge } from '@/design/Badge';
import { Drawer } from '@/design/Drawer';
import type { SeriesSlot } from '@/design/TimeSeriesChart';
import { KeyValue, type KeyValueItem } from '@/design/KeyValue';
import { LabPill } from '@/design/StatusPill';
import { formatRate } from '@/lib/format';
import { formatDateTime } from '@/lib/time';
import { InterfaceState } from './Interfaces';
import { MetricChart } from './MetricChart';
import styles from './Monitoring.module.css';
import { DEFAULT_RANGE, range } from './series';
import { metricFormat } from './vocab';
import { RangePicker } from './RangePicker';

export interface InterfaceDrawerProps {
  row: IfStatus;
  onClose: () => void;
}

export function InterfaceDrawer({ row, onClose }: InterfaceDrawerProps) {
  const [rangeId, setRangeId] = useState(DEFAULT_RANGE);
  const span = range(rangeId);
  const since = Math.floor(Date.now() / 30_000) * 30_000 - span.span;
  const line = (metric: string, label: string, slot: SeriesSlot) => ({
    device_id: row.device_id,
    metric,
    instance: row.name,
    label,
    slot,
  });

  const facts: KeyValueItem[] = [
    { label: 'State', value: <InterfaceState row={row} /> },
    { label: 'Description', value: row.description || 'None set' },
    {
      label: 'Speed',
      value: row.speed_bps == null ? 'The device did not say' : formatRate(row.speed_bps),
    },
    { label: 'In', value: metricFormat('if.in_bps')(row.in_bps) },
    { label: 'Out', value: metricFormat('if.out_bps')(row.out_bps) },
    { label: 'Utilisation', value: metricFormat('if.util_pct')(row.util_pct) },
    {
      label: 'Errors',
      value: `${metricFormat('if.in_err_ps')(row.in_err_ps)} in, ${metricFormat('if.out_err_ps')(row.out_err_ps)} out`,
    },
    {
      label: 'Discards',
      value: `${metricFormat('if.in_disc_ps')(row.in_disc_ps)} in, ${metricFormat('if.out_disc_ps')(row.out_disc_ps)} out`,
    },
    { label: 'Read', value: formatDateTime(row.at) },
  ];

  return (
    <Drawer
      open
      size="lg"
      onClose={onClose}
      title={row.name}
      description={
        <span className={styles.cell}>
          <Link to={`/monitoring/health?device=${encodeURIComponent(row.device_id)}`}>{row.device}</Link>
          {row.lab && <LabPill />}
          {row.uplink && <Badge variant="outline">Uplink</Badge>}
        </span>
      }
    >
      <div className={styles.facts}>
        <KeyValue items={facts} />

        <div className={styles.inline}>
          <RangePicker value={rangeId} onChange={setRangeId} />
        </div>

        <MetricChart
          title="Traffic"
          subtitle={`${row.name} on ${row.device}`}
          lines={[line('if.in_bps', 'In', 1), line('if.out_bps', 'Out', 2)]}
          unitOf="if.in_bps"
          since={since}
          step_s={span.step_s}
          invalidateOn={['monitor.interfaces']}
        />

        <MetricChart
          title="Errors and discards"
          subtitle="Per second, as counted by the device"
          lines={[
            line('if.in_err_ps', 'Errors in', 3),
            line('if.out_err_ps', 'Errors out', 4),
            line('if.in_disc_ps', 'Discards in', 5),
            line('if.out_disc_ps', 'Discards out', 6),
          ]}
          unitOf="if.in_err_ps"
          since={since}
          step_s={span.step_s}
          invalidateOn={['monitor.interfaces']}
        />

        <MetricChart
          title="Utilisation"
          subtitle={row.speed_bps == null ? 'Needs the port speed, which this device did not report' : 'Of the port speed'}
          lines={[line('if.util_pct', 'Utilisation', 1)]}
          since={since}
          step_s={span.step_s}
          invalidateOn={['monitor.interfaces']}
        />
      </div>
    </Drawer>
  );
}
