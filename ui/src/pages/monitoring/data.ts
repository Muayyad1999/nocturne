/**
 * The reads every tab shares, and the one write they all offer.
 *
 * Each query names the events that make it stale, so the screen follows the
 * backend instead of polling it: a poll publishes `monitor.polled` and
 * `devices.state`, an alert publishes `alerts.changed`, and editing a rule,
 * a window or a channel publishes `monitor.status_changed`.
 */

import { useRpc, useRpcMutation } from '@/api/hooks';
import type { DeviceRow } from '@/api/types/core';
import type {
  Alert,
  AlertSummary,
  Channel,
  JobStarted,
  MaintenanceWindow,
  MonitorStatus,
  PollNowParams,
} from '@/api/types/monitor';
import { confirm } from '@/design/Dialog';
import { formatCount } from '@/lib/format';

/**
 * Which RPC methods this build answers. Nocturne is assembled from feature
 * crates, so monitoring may simply not be there; asking once lets the screen
 * say so instead of showing controls that fail.
 */

export function useMonitorStatus() {
  return useRpc<MonitorStatus>('monitor.status', undefined, {
    invalidateOn: ['monitor.status_changed', 'devices.changed'],
    // `polls_per_min` and the last cycle time move on their own; there is no
    // event for a number that changes because time passed.
    refetchInterval: 15_000,
  });
}

export function useDevices() {
  return useRpc<DeviceRow[]>('devices.list', undefined, {
    invalidateOn: ['devices.changed', 'devices.state'],
  });
}

export function useAlertSummary() {
  return useRpc<AlertSummary>('alerts.summary', undefined, { invalidateOn: ['alerts.changed'] });
}

/**
 * Every alert that has not resolved yet. The tabs that want to know what is
 * open for one device narrow *this* list rather than counting problems of
 * their own: the backend raises one alert for one cause — a device that is
 * down suppresses its interface and threshold alerts — and a count worked out
 * from anything else would contradict `alerts.summary`.
 */
export function useOpenAlerts() {
  return useRpc<Alert[]>('alerts.list', { state: 'open', limit: 2000 }, { invalidateOn: ['alerts.changed'] });
}

export function useMaintenance() {
  return useRpc<MaintenanceWindow[]>('maintenance.list', undefined, {
    invalidateOn: ['monitor.status_changed'],
  });
}

export function useChannels() {
  return useRpc<Channel[]>('notify.channels.list', undefined, {
    invalidateOn: ['monitor.status_changed'],
  });
}

/** Open alerts by device id, worst first — the same rows the Alerts tab shows. */
export function alertsByDevice(alerts: readonly Alert[] | undefined): Map<string, Alert[]> {
  const byDevice = new Map<string, Alert[]>();
  for (const alert of alerts ?? []) {
    if (!alert.device_id) continue;
    const list = byDevice.get(alert.device_id);
    if (list) list.push(alert);
    else byDevice.set(alert.device_id, [alert]);
  }
  return byDevice;
}

/**
 * "Poll now": probing devices touches them, so it says how many and which
 * before it does. The work runs as a job; the status bar shows it running.
 */
export function usePollNow() {
  const poll = useRpcMutation<JobStarted, PollNowParams>('monitor.poll_now', {
    errorToast: 'The poll could not be started',
  });

  return {
    pending: poll.isPending,
    /** `what` names the devices for the question: "CORE-SW1", "6 devices". */
    run: async (deviceIds: readonly string[], what: string) => {
      if (deviceIds.length === 0) return;
      const ok = await confirm({
        title: `Poll ${what} now?`,
        description:
          deviceIds.length === 1
            ? 'Nocturne probes the device immediately instead of waiting for its next turn.'
            : `Nocturne probes all ${formatCount(deviceIds.length)} of them immediately instead of waiting for their next turn.`,
        confirmLabel: 'Poll now',
      });
      if (!ok) return;
      poll.mutate({ device_ids: [...deviceIds] });
    },
  };
}
