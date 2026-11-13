/**
 * The strip above the tabs: what the installation is watching right now, and
 * what is in the way of it watching the rest.
 *
 * Every number here comes straight from one backend answer — the open-alert
 * counts from `alerts.summary`, the interface count from `monitor.status`'s
 * own problem list. Nothing is added up in the browser, so a tile can never
 * disagree with the list it links to.
 */

import { CircleAlert, TriangleAlert } from 'lucide-react';
import { Link } from 'react-router';
import type { UseQueryResult } from '@tanstack/react-query';
import type { RpcError } from '@/api/client';
import type { DeviceRow } from '@/api/types/core';
import type { AlertSummary, MonitorStatus } from '@/api/types/monitor';
import { Badge } from '@/design/Badge';
import { Card } from '@/design/Card';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { Stat } from '@/design/Stat';
import { EMPTY, formatCount, formatDuration } from '@/lib/format';
import styles from './Monitoring.module.css';
import { SEVERITIES, deviceStatus, severityLabel, severityTone } from './vocab';

export interface StatusStripProps {
  status: UseQueryResult<MonitorStatus, RpcError>;
  devices: UseQueryResult<DeviceRow[], RpcError>;
  summary: UseQueryResult<AlertSummary, RpcError>;
}

/** Where an installation problem should send the reader, when there is one. */
const PROBLEM_LINK: Record<string, { to: string; label: string }> = {
  unreachable: { to: '/monitoring/health', label: 'Show the devices' },
  interfaces: { to: '/monitoring/interfaces?problems=1', label: 'Show the interfaces' },
  maintenance: { to: '/monitoring/rules', label: 'Show the windows' },
  channel: { to: '/monitoring/rules', label: 'Show the channels' },
  snmp_credential: { to: '/settings/connections', label: 'Open settings' },
  no_probe: { to: '/devices', label: 'Open the device' },
};

export function StatusStrip({ status, devices, summary }: StatusStripProps) {
  if (status.isError) {
    return (
      <ErrorState
        size="compact"
        title="Monitoring could not be reached"
        error={status.error}
        onRetry={() => void status.refetch()}
      />
    );
  }

  const monitored = (devices.data ?? []).filter((device) => device.monitor.enabled);
  const up = monitored.filter((device) => deviceStatus(device) === 'up').length;
  const down = monitored.filter((device) => deviceStatus(device) === 'down').length;
  const unknown = monitored.filter((device) => deviceStatus(device) === 'unknown').length;
  const interfaces = status.data?.problems.find((problem) => problem.kind === 'interfaces')?.count ?? 0;
  const loading = status.isPending || devices.isPending;
  const open = summary.data?.open ?? 0;

  return (
    <div className={styles.strip}>
      {status.data && !status.data.enabled && (
        <Card className={styles.off} padding="sm">
          <Icon icon={CircleAlert} size={16} />
          <span>
            <strong>Monitoring is switched off.</strong> Nothing is being polled and no alert can fire. Switch
            it back on in Settings → Monitoring; the devices keep their own settings meanwhile.
          </span>
        </Card>
      )}

      <div className={styles.tiles}>
        <Link to="/monitoring/health" className={styles.tile}>
          <Stat
            label="Devices answering"
            value={loading ? EMPTY : `${formatCount(up)} / ${formatCount(monitored.length)}`}
            loading={loading}
            delta={
              down > 0
                ? { text: `${formatCount(down)} down`, direction: 'down', sentiment: 'bad' }
                : unknown > 0
                  ? { text: `${formatCount(unknown)} not polled yet`, direction: 'flat' }
                  : undefined
            }
          />
        </Link>

        <Link to="/monitoring/alerts" className={styles.tile}>
          <Stat
            label="Open alerts"
            value={summary.isPending ? EMPTY : formatCount(open)}
            loading={summary.isPending}
            trend={
              open > 0 ? (
                <span className={styles.severities}>
                  {SEVERITIES.filter((severity) => (summary.data?.by_severity[severity] ?? 0) > 0).map(
                    (severity) => (
                      <Badge key={severity} tone={severityTone(severity)}>
                        {formatCount(summary.data?.by_severity[severity])} {severityLabel(severity).toLowerCase()}
                      </Badge>
                    ),
                  )}
                </span>
              ) : undefined
            }
            delta={open === 0 && !summary.isPending ? { text: 'Nothing open', direction: 'flat' } : undefined}
          />
        </Link>

        <Link
          to="/monitoring/interfaces?problems=1"
          className={styles.tile}
          title="Interfaces that are administratively up while down, taking errors, or at 90 % utilisation and above"
        >
          <Stat
            label="Interfaces needing a look"
            value={loading ? EMPTY : formatCount(interfaces)}
            loading={loading}
            delta={
              interfaces > 0
                ? // Short enough to fit the tile beside the dock at 1280; the
                  // whole sentence is the tile's tooltip.
                  { text: 'down or erroring', direction: 'up', sentiment: 'bad' }
                : { text: 'none right now', direction: 'flat' }
            }
          />
        </Link>

        <div className={styles.tile} data-static="true">
          <Stat
            label="Polling"
            value={loading ? EMPTY : formatCount(status.data?.polls_per_min)}
            unit="per minute"
            loading={loading}
            delta={
              status.data
                ? {
                    text:
                      status.data.last_cycle_ms > 0
                        ? `${formatDuration(status.data.last_cycle_ms)} last round`
                        : 'no round finished yet',
                    direction: 'flat',
                  }
                : undefined
            }
          />
        </div>
      </div>

      {(status.data?.problems ?? [])
        .filter((problem) => problem.kind !== 'interfaces')
        .map((problem) => {
          const link = PROBLEM_LINK[problem.kind];
          return (
            <Card key={`${problem.kind}:${problem.device_id ?? ''}`} className={styles.problem} padding="sm">
              <Icon icon={TriangleAlert} size={16} className={styles.problemIcon} />
              <span className={styles.problemText}>{problem.message}</span>
              {link && (
                <Link className={styles.problemLink} to={link.to}>
                  {link.label}
                </Link>
              )}
            </Card>
          );
        })}
    </div>
  );
}
