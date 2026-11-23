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

import { useQueryClient } from '@tanstack/react-query';
import { useMemo } from 'react';
import { rpcQueryKey, useEvent, useRpc, useRpcMutation } from '@/api/hooks';
import type { Device } from '@/api/types/core';
import type {
  Detection,
  DetectionHit,
  DetectionRecentParams,
  DetectionSetEnabledParams,
  DetectionsHit,
} from '@/api/types/detect';
import type {
  Bucket,
  DeviceSetup,
  FlowExporter,
  FlowTop,
  FlowTopParams,
  Forwarder,
  ForwarderDeleted,
  ForwarderSaveParams,
  ForwarderTested,
  LogFilter,
  LogPage,
  LogRule,
  LogSource,
  LogsSettings,
  LogsStatus,
  Pulled,
  RuleDeleted,
  RuleSaveParams,
  RuleTest,
  RuleTestParams,
} from '@/api/types/logs';
import { withHit } from './detections';
import { capabilitiesOf, type Capabilities } from './panes';

/** The first page, and every page back. The backend clamps at 1000. */
export const PAGE = 500;

/**
 * What this build can do, from `app.methods`. `undefined` while the list has
 * not answered: a pane that flashes "not in this build" and then draws
 * itself is worse than a skeleton for one round trip.
 */
export function useCapabilities(): Capabilities | undefined {
  const methods = useRpc<string[]>('app.methods');
  return useMemo(
    () => (methods.data === undefined ? undefined : capabilitiesOf(methods.data)),
    [methods.data],
  );
}

export function useLogsStatus(enabled = true) {
  return useRpc<LogsStatus>('logs.status', undefined, {
    enabled,
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
export function useDevices(enabled = true) {
  return useRpc<Device[]>('devices.list', undefined, { enabled, invalidateOn: ['devices.changed'] });
}

/**
 * The configuration lines that would point one device's syslog here.
 *
 * `logs.device_setup` does not apply them and does not touch the device: they
 * are text, meant for the change pipeline or for a person's own session.
 */
export function useDeviceSetup(deviceId: string | null, enabled = true) {
  return useRpc<DeviceSetup>(
    'logs.device_setup',
    { device_id: deviceId },
    // The lines name the ports the receivers listen on, and the trap note
    // says whether the device's notifications would be kept: both change
    // with the receivers' state.
    { enabled: enabled && deviceId !== null, invalidateOn: ['logs.status_changed'] },
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
 * Change the `logs` settings section. `settings.set` merges, so a patch of
 * one flag or one port leaves everything else as it is, and the receiver
 * whose socket the change concerns follows the setting by itself.
 */
export function useSetLogSettings(errorToast: string | false = 'The receiver settings could not be changed') {
  return useRpcMutation<LogsSettings, { section: 'logs'; value: unknown }>('settings.set', {
    invalidates: ['logs.', 'settings.'],
    errorToast,
  });
}

/**
 * The `logs` settings section: what each receiver would bind.
 *
 * `logs.status` only reports what is *actually* listening, which is nothing at
 * all when a receiver is off — and a card has to be able to say which port it
 * is about to open before it opens it.
 */
export function useLogSettings(enabled = true) {
  return useRpc<LogsSettings>('settings.get', { section: 'logs' }, {
    enabled,
    invalidateOn: ['settings.changed'],
  });
}

/**
 * Re-apply the receiver settings: a receiver that could not bind is tried
 * again (whatever held the port may have gone), one that is running is left
 * alone. The answer is the status afterwards, so it goes straight into the
 * cache instead of being asked for a second time.
 */
export function useRestartReceivers() {
  const queryClient = useQueryClient();
  const restart = useRpcMutation<LogsStatus, void>('logs.restart', {
    errorToast: 'The receivers could not be restarted',
  });
  return {
    pending: restart.isPending,
    run: () =>
      restart.mutate(undefined, {
        onSuccess: (status) => queryClient.setQueryData(rpcQueryKey('logs.status', undefined), status),
      }),
  };
}

// -------------------------------------------------------------------- rules

export function useLogRules(enabled = true) {
  return useRpc<LogRule[]>('logs.rules.list', undefined, { enabled });
}

export function useSaveRule() {
  return useRpcMutation<LogRule, RuleSaveParams>('logs.rules.save', {
    invalidates: ['logs.rules.', 'logs.status'],
    // The editor shows the refusal beside the field it is about.
    errorToast: false,
  });
}

export function useDeleteRule() {
  return useRpcMutation<RuleDeleted, { id: string }>('logs.rules.delete', {
    invalidates: ['logs.rules.', 'logs.status'],
    errorToast: 'The rule could not be deleted',
  });
}

/** What a rule would do with one pasted line. Stores nothing. */
export function useTestRule() {
  return useRpcMutation<RuleTest, RuleTestParams>('logs.rules.test', { errorToast: false });
}

// --------------------------------------------------------------- forwarding

export function useForwarders(enabled = true) {
  return useRpc<Forwarder[]>('logs.forward.list', undefined, {
    enabled,
    // A save or a delete from anywhere publishes the status.
    invalidateOn: ['logs.status_changed'],
  });
}

export function useSaveForwarder() {
  return useRpcMutation<Forwarder, ForwarderSaveParams>('logs.forward.save', {
    invalidates: ['logs.forward.', 'logs.status'],
    errorToast: false,
  });
}

export function useDeleteForwarder() {
  return useRpcMutation<ForwarderDeleted, { id: string }>('logs.forward.delete', {
    invalidates: ['logs.forward.', 'logs.status'],
    errorToast: 'The forwarder could not be deleted',
  });
}

/** One event that says it is a test, sent at once. A failure is an error that says why. */
export function useTestForwarder() {
  return useRpcMutation<ForwarderTested, { id: string }>('logs.forward.test', { errorToast: false });
}

// -------------------------------------------------------------------- flows

/** How often the Flows pane asks again. The collector publishes nothing per flow. */
export const FLOWS_REFRESH_MS = 30_000;

export function useFlowTop(params: FlowTopParams, enabled = true) {
  return useRpc<FlowTop>('flows.top', params, {
    enabled,
    // There is no event to follow: nothing of a flow is published, so the
    // only way to see the next minute is to ask for it.
    refetchInterval: FLOWS_REFRESH_MS,
    invalidateOn: ['logs.status_changed'],
  });
}

export function useFlowExporters(enabled = true) {
  return useRpc<FlowExporter[]>('flows.exporters', undefined, {
    enabled,
    refetchInterval: FLOWS_REFRESH_MS,
    invalidateOn: ['logs.status_changed', 'devices.changed'],
  });
}

// --------------------------------------------------------------- detections

export function useDetections(enabled = true) {
  return useRpc<Detection[]>('detections.list', undefined, {
    enabled,
    // A hit moves `hits_24h` and `last_hit_at`; `changed` is a switch or a
    // failed check. The whole `detect` section can also be switched in
    // Settings, which arrives as `settings.changed`.
    invalidateOn: ['detections.changed', 'detections.hit', 'settings.changed'],
  });
}

/** How many recent hits the pane holds. */
export const RECENT_HITS = 100;

/**
 * The recent hits, kept live: `detections.hit` carries the hit itself, so it
 * goes in front of the list at once and the list is not asked for again.
 */
export function useRecentHits(detection: string, enabled = true) {
  const queryClient = useQueryClient();
  const params: DetectionRecentParams = {
    ...(detection === '' ? {} : { id: detection }),
    limit: RECENT_HITS,
  };
  const query = useRpc<DetectionHit[]>('detections.recent', params, { enabled });
  const key = rpcQueryKey('detections.recent', params);
  useEvent<DetectionsHit>('detections.hit', (payload) => {
    if (!enabled || !payload?.hit) return;
    queryClient.setQueryData<DetectionHit[]>(key, (hits) =>
      hits === undefined ? hits : withHit(hits, payload.hit, detection === '' ? null : detection, RECENT_HITS),
    );
  });
  return query;
}

export function useSetDetectionEnabled() {
  return useRpcMutation<Detection, DetectionSetEnabledParams>('detections.set_enabled', {
    invalidates: ['detections.list'],
    // The engine's refusal is shown where the switch is.
    errorToast: false,
  });
}
