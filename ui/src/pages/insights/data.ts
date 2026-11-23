/**
 * Everything the Insights screen reads, in one place.
 *
 * Every query is invalidated by `insights.changed` (published after each
 * scan, dismissal and reopen) rather than polled, and the catalogue and the
 * thresholds by `settings.changed`, because switching a check off changes
 * what the next scan will even look at.
 */

import { useRpc, useRpcMutation } from '@/api/hooks';
import type { Device } from '@/api/types/core';
import type {
  CheckInfo,
  Coverage,
  Insight,
  InsightsDismissParams,
  InsightsFixResult,
  InsightsIdParams,
  InsightsListParams,
  InsightsListResult,
  InsightsSettings,
} from '@/api/types/insights';
import type { RpcError } from '@/api/errors';
import type { UseMutationResult } from '@tanstack/react-query';

/** The page size, and the ceiling the backend clamps to. */
export const PAGE = 100;
export const MAX_PAGE = 500;

export interface Filter {
  /** `open` by default; `any` includes dismissed and resolved findings. */
  state: InsightsListParams['state'];
  severity: InsightsListParams['severity'];
  category: InsightsListParams['category'];
  check: string | null;
  deviceId: string | null;
  /** Narrows to the devices of one site; applied here, not by the backend. */
  site: string | null;
  query: string;
}

export const EMPTY_FILTER: Filter = {
  state: 'open',
  severity: undefined,
  category: undefined,
  check: null,
  deviceId: null,
  site: null,
  query: '',
};

export function filterParams(filter: Filter, limit: number): InsightsListParams {
  return {
    state: filter.state,
    ...(filter.severity ? { severity: filter.severity } : {}),
    ...(filter.category ? { category: filter.category } : {}),
    ...(filter.check ? { check: filter.check } : {}),
    ...(filter.deviceId ? { device_id: filter.deviceId } : {}),
    ...(filter.query.trim() === '' ? {} : { query: filter.query.trim() }),
    limit,
  };
}

/** Whether anything is narrowing the list beyond the default state. */
export function isNarrowed(filter: Filter): boolean {
  return (
    filter.severity !== undefined ||
    filter.category !== undefined ||
    filter.check !== null ||
    filter.deviceId !== null ||
    filter.site !== null ||
    filter.query.trim() !== ''
  );
}

// ----------------------------------------------------------------- queries

export function useInsights(filter: Filter, limit: number) {
  return useRpc<InsightsListResult>('insights.list', filterParams(filter, limit), {
    invalidateOn: ['insights.changed'],
  });
}

/** `null` before the first scan: nothing has been examined yet. */
export function useCoverage() {
  return useRpc<Coverage | null>('insights.coverage', undefined, {
    invalidateOn: ['insights.changed', 'settings.changed'],
  });
}

export function useChecks() {
  return useRpc<CheckInfo[]>('insights.checks', undefined, {
    invalidateOn: ['settings.changed'],
  });
}

export function useInsightsSettings() {
  return useRpc<InsightsSettings>(
    'settings.get',
    { section: 'insights' },
    { invalidateOn: ['settings.changed'] },
  );
}

/**
 * The inventory, for the device and site filters and to say how much of it
 * the last scan covered. Sites are not on a finding, so they are looked up
 * here; the list is shared with every other screen through the query cache.
 */
export function useDevices() {
  return useRpc<Device[]>('devices.list', undefined, { invalidateOn: ['devices.changed'] });
}

export function useInsight(id: string | null) {
  return useRpc<Insight>(
    'insights.get',
    { id },
    { enabled: id !== null, invalidateOn: ['insights.changed'] },
  );
}

/** Whether this build answers a method at all (features are optional crates). */

// --------------------------------------------------------------- mutations

export function useDismiss(): UseMutationResult<Insight, RpcError, InsightsDismissParams> {
  // The dialog shows the error itself: a toast would open behind it.
  return useRpcMutation<Insight, InsightsDismissParams>('insights.dismiss', {
    invalidates: ['insights.'],
    errorToast: false,
  });
}

export function useReopen(): UseMutationResult<Insight, RpcError, InsightsIdParams> {
  return useRpcMutation<Insight, InsightsIdParams>('insights.reopen', {
    invalidates: ['insights.'],
    errorToast: 'The finding could not be reopened',
  });
}

/**
 * `insights.fix` does not touch a device: it hands the suggested lines to the
 * change pipeline as a draft, which then does what it always does. The
 * screen's wording is the other half of that promise.
 */
export function useDraftChange(): UseMutationResult<InsightsFixResult, RpcError, InsightsIdParams> {
  return useRpcMutation<InsightsFixResult, InsightsIdParams>('insights.fix', {
    invalidates: ['changes.'],
    errorToast: 'No change could be drafted',
  });
}

export interface SettingsWrite {
  section: string;
  value: Record<string, unknown>;
}

export function useSaveSettings(): UseMutationResult<InsightsSettings, RpcError, SettingsWrite> {
  return useRpcMutation<InsightsSettings, SettingsWrite>('settings.set', {
    invalidates: ['settings.', 'insights.'],
    errorToast: 'That setting could not be saved',
  });
}

/**
 * One threshold field's own save. The engine's refusal is said beside the
 * field that was refused, so there is no toast, and each field has its own
 * answer: a mutation shared by sixteen fields reports only the last call.
 */
export function useSaveThreshold(): UseMutationResult<InsightsSettings, RpcError, SettingsWrite> {
  return useRpcMutation<InsightsSettings, SettingsWrite>('settings.set', {
    invalidates: ['settings.', 'insights.'],
    errorToast: false,
  });
}

// --------------------------------------------------------------- the index

export interface DeviceIndex {
  /** Device id → the device, for the site of a finding. */
  byId: Map<string, Device>;
  /** Sites that have at least one device, alphabetically. */
  sites: string[];
  /** Devices that a scan would cover, given the `include_lab` setting. */
  scannable: number;
  /** Simulated devices that the `include_lab` setting is keeping out. */
  labExcluded: number;
}

export function indexDevices(devices: readonly Device[], includeLab: boolean): DeviceIndex {
  const byId = new Map<string, Device>();
  const sites = new Set<string>();
  let scannable = 0;
  let labExcluded = 0;
  for (const device of devices) {
    byId.set(device.id, device);
    if (device.site !== '') sites.add(device.site);
    if (device.lab && !includeLab) labExcluded += 1;
    else scannable += 1;
  }
  return { byId, sites: [...sites].sort((a, b) => a.localeCompare(b)), scannable, labExcluded };
}
