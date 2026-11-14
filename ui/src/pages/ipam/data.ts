/**
 * Everything this screen reads and writes.
 *
 * One event keeps it current: `ipam.changed {kinds}`, which the feature
 * publishes after every save, delete, import, learn and after the debounced
 * sync that follows `collect.updated`. Nothing here polls — a prefix tree does
 * not change because a second went by — except the reconciliation report,
 * which is a reading of the world at a moment and is re-read only when asked.
 *
 * Two reads come from outside this feature, and both are optional by design
 * (`docs/ARCHITECTURE.md`: a feature calls another by name and copes when it
 * is absent):
 *
 * * `labs.list`, to turn a `lab:<id>` namespace into the lab's name. Without
 *   the lab feature the id is shown instead.
 * * `devices.list`, to turn the device ids the backend puts on both sides of a
 *   `device` disagreement into names. Without it the ids are shown.
 */

import { useMemo } from 'react';
import { useHasMethod, useRpc, useRpcMutation } from '@/api/hooks';
import type { RpcError } from '@/api/errors';
import type { DeviceRow } from '@/api/types/core';
import type {
  AddressInput,
  Conflict,
  DeletedAddress,
  DeletedPrefix,
  DeletedVlan,
  ExportParams,
  ExportResult,
  ImportParams,
  ImportResult,
  IpamSettings,
  LearnParams,
  LearnPlan,
  NextAddresses,
  NextPrefix,
  PrefixDetail,
  PrefixInput,
  PrefixRow,
  Reconciliation,
  ReserveParams,
  SavedAddress,
  SavedPrefix,
  SearchResult,
  SplitParams,
  SplitResult,
  Summary,
  SyncResult,
  VlanInput,
  VlanRow,
} from '@/api/types/ipam';
import type { LabSummary } from '@/api/types/lab';

/** The one topic. Every list on the screen is read again when it fires. */
export const CHANGED = ['ipam.changed'];

/** Is the feature in this build at all? `undefined` while it is being asked. */
export function useIpamInstalled(): boolean | undefined {
  return useHasMethod('ipam.prefixes');
}

export interface PrefixesQuery {
  site?: string;
  query?: string;
}

export function usePrefixes(params: PrefixesQuery) {
  return useRpc<PrefixRow[]>('ipam.prefixes', params, { invalidateOn: CHANGED });
}

export function usePrefix(id: string | null) {
  return useRpc<PrefixDetail>('ipam.prefix.get', id ? { id, addresses: true } : undefined, {
    enabled: id !== null,
    invalidateOn: CHANGED,
  });
}

export function useSummary() {
  return useRpc<Summary>('ipam.summary', undefined, { invalidateOn: CHANGED });
}

export function useVlans(site: string) {
  return useRpc<VlanRow[]>('ipam.vlans', site ? { site } : {}, { invalidateOn: CHANGED });
}

export function useConflicts(enabled: boolean) {
  return useRpc<Conflict[]>('ipam.conflicts', undefined, { enabled, invalidateOn: CHANGED });
}

/**
 * The reconciliation report. It is a comparison of the plan with the last
 * collection, so it is read when the tab is opened and when the data under it
 * changes — never on a timer, which would suggest it is live.
 */
export function useReconcile(site: string, enabled: boolean) {
  return useRpc<Reconciliation>('ipam.reconcile', site ? { site } : {}, {
    enabled,
    invalidateOn: CHANGED,
  });
}

export function useSearch(query: string) {
  const trimmed = query.trim();
  return useRpc<SearchResult>('ipam.search', trimmed ? { query: trimmed } : undefined, {
    enabled: trimmed.length > 0,
    invalidateOn: CHANGED,
  });
}

export function useIpamSettings() {
  return useRpc<IpamSettings>('settings.get', { section: 'ipam' });
}

/** The free block of a given size inside a parent; nothing is created. */
export function useNextPrefix(ask: { parent_id: string; len: number } | null) {
  return useRpc<NextPrefix>('ipam.next_prefix', ask ?? undefined, { enabled: ask !== null });
}

/** Free addresses, with the basis of the answer. Nothing is created. */
export function useNextAddress(ask: { prefix_id: string; count: number } | null) {
  return useRpc<NextAddresses>('ipam.next_address', ask ?? undefined, {
    enabled: ask !== null,
    invalidateOn: CHANGED,
  });
}

export function useExport(params: ExportParams | null) {
  return useRpc<ExportResult>('ipam.export', params ?? undefined, { enabled: params !== null });
}

/**
 * Lab names, for the labelled lab roots. A build without the lab feature
 * answers `unknown_method`; the map is then empty and the ids are shown.
 */
export function useLabNames(): ReadonlyMap<string, string> {
  const labs = useRpc<LabSummary[]>('labs.list', undefined, { invalidateOn: ['labs.changed'] });
  return useMemo(
    () => new Map((labs.data ?? []).map((lab) => [lab.id, lab.name])),
    [labs.data],
  );
}

/** Device names by id, for resolving the ids inside a disagreement. */
export function useDeviceNames(): (id: string) => string | undefined {
  const devices = useRpc<DeviceRow[]>('devices.list', undefined, {
    invalidateOn: ['devices.changed'],
  });
  const names = useMemo(
    () => new Map((devices.data ?? []).map((device) => [device.id, device.name])),
    [devices.data],
  );
  return (id: string) => names.get(id);
}

// -------------------------------------------------------------------- writes

export function useSavePrefix() {
  return useRpcMutation<SavedPrefix, PrefixInput>('ipam.prefix.save', {
    invalidates: ['ipam.'],
    errorToast: false,
  });
}

export function useDeletePrefix() {
  return useRpcMutation<DeletedPrefix, { id: string }>('ipam.prefix.delete', {
    invalidates: ['ipam.'],
    errorToast: 'That block could not be deleted',
  });
}

export function useSplitPrefix() {
  return useRpcMutation<SplitResult, SplitParams>('ipam.prefix.split', {
    invalidates: ['ipam.'],
    errorToast: false,
  });
}

export function useSaveAddress() {
  return useRpcMutation<SavedAddress, AddressInput>('ipam.address.save', {
    invalidates: ['ipam.'],
    errorToast: false,
  });
}

export function useDeleteAddress() {
  return useRpcMutation<DeletedAddress, { ip: string; scope: string }>('ipam.address.delete', {
    invalidates: ['ipam.'],
    errorToast: 'That record could not be deleted',
  });
}

export function useReserveAddress() {
  return useRpcMutation<SavedAddress, ReserveParams>('ipam.address.reserve', {
    invalidates: ['ipam.'],
    errorToast: false,
  });
}

export function useSaveVlan() {
  return useRpcMutation<VlanRow, VlanInput>('ipam.vlan.save', {
    invalidates: ['ipam.'],
    errorToast: false,
  });
}

export function useDeleteVlan() {
  return useRpcMutation<DeletedVlan, { id: string }>('ipam.vlan.delete', {
    invalidates: ['ipam.'],
    errorToast: 'That VLAN record could not be deleted',
  });
}

/**
 * Re-reads the collected data now. It writes **evidence only**: the single
 * record it can create is a `discovered` prefix, which says in itself that
 * nobody has confirmed it.
 */
export function useSync() {
  return useRpcMutation<SyncResult, { device_ids?: string[] }>('ipam.sync', {
    invalidates: ['ipam.'],
    errorToast: 'The collected data could not be re-read',
  });
}

/**
 * The only call that turns evidence into records, and it previews unless it is
 * told otherwise: `dry_run` defaults to `true` in the backend and this screen
 * always sends it explicitly.
 */
export function useLearn() {
  return useRpcMutation<LearnPlan, LearnParams>('ipam.learn', {
    invalidates: ['ipam.'],
    errorToast: false,
  });
}

export function useImport() {
  return useRpcMutation<ImportResult, ImportParams>('ipam.import', {
    invalidates: ['ipam.'],
    errorToast: false,
  });
}

export type { RpcError };
