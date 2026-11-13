/**
 * Everything the Overview reads and writes.
 *
 * One read answers almost the whole screen: `overview.get` assembles each part
 * from the feature that owns it, independently, and reports the parts it could
 * not fill instead of guessing. The only other read is the inventory, for the
 * *names* of the devices that are down — the overview carries their count but
 * not their names, and "two devices are down" without saying which two is not
 * much use at 7 a.m.
 *
 * **How it stays current.** Nothing here polls for the sake of polling:
 *
 * - every event that can change a number on this screen refetches it
 *   ({@link TOPICS}), collapsed so that a switch reboot which fires forty
 *   events costs one read;
 * - a slow refetch every {@link SLOW_REFETCH_MS} covers the three things no
 *   event announces, because they change with the clock rather than with the
 *   network: the rolling 24-hour counters (`applied_24h`, `failed_24h`,
 *   `changed_24h`), the age of the last compliance run, and the age of the
 *   reading itself. There is no `compliance.changed` topic, and no event fires
 *   when a change simply gets older than a day.
 *
 * **Why `refresh: true` on every read.** The backend caches an overview for a
 * few seconds (`ops.overview_cache_ms`, 5 s by default) so several panes asking
 * at once cost one read. This screen is the only pane asking, and it asks
 * *because something just happened* — a cached copy from before the event is
 * exactly the wrong answer, and would leave the screen a minute behind. So it
 * asks for a fresh read and throttles itself instead ({@link MIN_GAP_MS}),
 * which the server-side cache cannot do on its behalf. The fresh answer is put
 * in that cache, so any other pane still gets it for free.
 */

import { useMutation, useQueryClient, type UseMutationResult } from '@tanstack/react-query';
import { useEffect, useEffectEvent, useRef } from 'react';
import { rpc, subscribe } from '@/api/client';
import type { RpcError } from '@/api/errors';
import { invalidateMethods, useRpc, useRpcMutation } from '@/api/hooks';
import type { DeviceRow } from '@/api/types/core';
import type { Overview, OverviewParams } from '@/api/types/ops';
import { toast } from '@/design/Toast';

/** Every topic that can change something on this screen. */
export const TOPICS = [
  // Up, down and never-polled; and the inventory the names come from.
  'devices.state',
  'devices.changed',
  // Firing, acknowledged, resolved.
  'alerts.changed',
  'alerts.raised',
  // Drafted, planned, approved, applied, failed.
  'changes.changed',
  // Running jobs and the ones that failed.
  'jobs.changed',
  // A new configuration version changes backup coverage.
  'config.changed',
  // Findings appear after a scan.
  'insights.changed',
  // This feature's own records.
  'incidents.changed',
  // The activity feed is mostly the audit trail.
  'audit.appended',
] as const;

/**
 * The slow refetch: only for what changes with the clock. Long enough that a
 * wallboard left open overnight costs 1 440 reads rather than 172 800, short
 * enough that the 24-hour counters are never more than a minute wrong.
 */
export const SLOW_REFETCH_MS = 60_000;

/** An event burst is one read: the last event in this window wins. */
export const SETTLE_MS = 400;

/** However busy the network gets, the screen re-reads at most this often. */
export const MIN_GAP_MS = 3_000;

const PARAMS: OverviewParams = { refresh: true };

export interface OverviewRead {
  data: Overview | undefined;
  error: RpcError | null;
  isPending: boolean;
  isFetching: boolean;
  /** Reads it again now, for the Refresh button. */
  refetch: () => void;
}

export function useOverview(): OverviewRead {
  const query = useRpc<Overview>('overview.get', PARAMS, { refetchInterval: SLOW_REFETCH_MS });

  const refetch = useEffectEvent(() => {
    void query.refetch();
  });
  const lastRead = useRef(0);

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    const nudge = () => {
      if (timer) return;
      // Collapse the burst, and never read more often than MIN_GAP_MS.
      const wait = Math.max(SETTLE_MS, lastRead.current + MIN_GAP_MS - Date.now());
      timer = setTimeout(() => {
        timer = null;
        lastRead.current = Date.now();
        refetch();
      }, wait);
    };
    const stops = TOPICS.map((topic) => subscribe(topic, nudge));
    return () => {
      for (const stop of stops) stop();
      if (timer) clearTimeout(timer);
    };
  }, []);

  return {
    data: query.data,
    error: query.error,
    isPending: query.isPending,
    isFetching: query.isFetching,
    refetch: () => {
      lastRead.current = Date.now();
      void query.refetch();
    },
  };
}

/**
 * The inventory, only so that a device that is down can be named and linked.
 * The same query key every other screen uses, so this costs nothing when the
 * user has already been to Devices. A failure here is not an error on this
 * screen: the counts still come from the overview, and the list says it could
 * not name them.
 */
export function useDevices() {
  return useRpc<DeviceRow[]>('devices.list', undefined, {
    invalidateOn: ['devices.changed', 'devices.state'],
  });
}

// ------------------------------------------------------------------- writes

/** Acknowledging an alert from the attention list: it stays open, but it is seen. */
export function useAcknowledge(): UseMutationResult<unknown, RpcError, { id: string }> {
  return useRpcMutation<unknown, { id: string }>('alerts.ack', {
    invalidates: ['alerts.', 'overview.'],
    errorToast: 'That alert could not be acknowledged',
  });
}

/** Stopping a running job. The dialog in front of this says what it will stop. */
export function useCancelJob(): UseMutationResult<unknown, RpcError, { id: string }> {
  return useRpcMutation<unknown, { id: string }>('jobs.cancel', {
    invalidates: ['jobs.', 'overview.'],
    errorToast: 'That job could not be cancelled',
  });
}

/**
 * The first-run door: builds the demo campus and starts it, which gives the
 * inventory real (simulated) devices, a topology, configurations and findings.
 * Two calls, because `labs.create` makes the lab and `labs.start` boots it;
 * nothing is deployed anywhere — the simulator runs in the engine's process.
 */
export function useDemoLab(): UseMutationResult<{ id: string }, RpcError, void> {
  const queryClient = useQueryClient();
  return useMutation<{ id: string }, RpcError, void>({
    mutationFn: async () => {
      const lab = await rpc<{ id: string }>('labs.create', { example: 'demo' });
      await rpc('labs.start', { id: lab.id });
      return lab;
    },
    onSuccess: async () => {
      toast.success('The demo network is running', {
        description: 'Its devices are simulated and marked LAB everywhere they appear.',
      });
      await invalidateMethods(queryClient, ['overview.', 'devices.', 'labs.', 'topology.']);
    },
    onError: (error) => {
      toast.error('The demo network could not be started', { description: error.message });
    },
  });
}
