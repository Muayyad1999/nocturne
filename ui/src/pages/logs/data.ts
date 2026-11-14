/**
 * Everything the Logs screen reads and writes, in one place.
 *
 * Nothing here polls. `logs.status_changed` is published whenever a receiver
 * starts, stops or fails and after a prune, and `logs.appended` whenever lines
 * are written, so the status line and the histogram follow the engine instead
 * of asking it every few seconds. The counts in `logs.status` are the only
 * numbers on screen that move on their own, and they move because the engine
 * said so.
 */

import { useRpc, useRpcMutation } from '@/api/hooks';
import type { Device } from '@/api/types/core';
import type {
  Bucket,
  DeviceSetup,
  LogFilter,
  LogPage,
  LogSource,
  LogsStatus,
  Pulled,
} from '@/api/types/logs';

/** The first page, and every page back. The backend clamps at 1000. */
export const PAGE = 500;

export function useLogsStatus() {
  return useRpc<LogsStatus>('logs.status', undefined, {
    // A prune, a restart or a failed bind all arrive as this event; `appended`
    // is what moves the stored-row count.
    invalidateOn: ['logs.status_changed', 'logs.appended'],
  });
}

/** The first (newest) page of a search. */
export function useLogPage(filter: LogFilter, enabled: boolean) {
  return useRpc<LogPage>('logs.query', { ...filter, limit: PAGE }, { enabled });
}

/**
 * The histogram, asked for **without** the severity filter: the chart shows
 * what the filter hides rather than hiding it twice, and the counts under it
 * are then an honest denominator.
 */
export function useHistogram(
  filter: LogFilter,
  since: number,
  until: number,
  buckets: number,
  enabled: boolean,
) {
  return useRpc<Bucket[]>(
    'logs.histogram',
    { ...filter, since, until, buckets },
    { enabled, invalidateOn: ['logs.appended'] },
  );
}

/** Every address that has sent, and the device that claims it. */
export function useLogSources(enabled: boolean) {
  return useRpc<LogSource[]>('logs.sources', undefined, {
    enabled,
    invalidateOn: ['logs.appended', 'devices.changed'],
  });
}

/** The inventory, for the device filter and for the setup instructions. */
export function useDevices() {
  return useRpc<Device[]>('devices.list', undefined, { invalidateOn: ['devices.changed'] });
}

/**
 * The configuration lines that would point one device's syslog here.
 *
 * `logs.device_setup` does not apply them and does not touch the device: they
 * are text, meant for the change pipeline or for a person's own session.
 */
export function useDeviceSetup(deviceId: string | null) {
  return useRpc<DeviceSetup>(
    'logs.device_setup',
    { device_id: deviceId },
    { enabled: deviceId !== null, invalidateOn: ['logs.status_changed'] },
  );
}

/** Read a device's own log buffer over the ordinary read-only CLI path. */
export function usePull() {
  return useRpcMutation<Pulled, { device_id: string }>('logs.pull', {
    invalidates: ['logs.'],
    errorToast: 'That device’s log buffer could not be read',
  });
}

/**
 * Switch the receiver on. `settings.set` merges, so this changes the one flag
 * and leaves the ports and the bind address as they are; the receiver follows
 * the setting by itself.
 */
export function useEnableReceiver() {
  return useRpcMutation<unknown, { section: string; value: unknown }>('settings.set', {
    invalidates: ['logs.', 'settings.'],
    errorToast: 'The receiver could not be switched on',
  });
}

/**
 * The `logs` settings section: the ports the receiver would bind.
 *
 * `logs.status` only reports what is *actually* listening, which is nothing at
 * all when the receiver is off — and the hint has to be able to say which port
 * it is about to open before it opens it.
 */
export function useLogSettings() {
  return useRpc<{
    syslog: { enabled: boolean; udp_port: number; tcp_port: number; bind: string };
    retention_days: number;
    max_rows: number;
  }>('settings.get', { section: 'logs' }, { invalidateOn: ['settings.changed'] });
}
