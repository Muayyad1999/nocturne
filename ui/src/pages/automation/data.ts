/**
 * The reads every tab shares.
 *
 * Each query names the events that make it stale, so the screen follows the
 * backend rather than polling it: a run starting or ending publishes
 * `automation.runs.changed`, saving a task, template, playbook, schedule or
 * trigger publishes its own `*.changed`, and a schedule or trigger firing
 * publishes `schedules.fired` / `triggers.fired`.
 */

import { useRpc } from '@/api/hooks';
import type {
  AnsibleStatus,
  Playbook,
  Run,
  RunDetail,
  Schedule,
  SelectedDevices,
  Selector,
  Task,
  Template,
  Trigger,
} from '@/api/types/automation';
import type { DeviceRow, Facets } from '@/api/types/core';

/**
 * Which RPC methods this build answers. Nocturne is assembled from feature
 * crates, so automation may simply not be there; asking once lets the screen
 * say so instead of showing controls that fail.
 */

/** Every device, for names, lab pills and the filter's own vocabulary. */
export function useDevices() {
  return useRpc<DeviceRow[]>('devices.list', undefined, {
    invalidateOn: ['devices.changed', 'devices.state'],
  });
}

export function useFacets() {
  return useRpc<Facets>('devices.facets', undefined, { invalidateOn: ['devices.changed'] });
}

/** Lab device ids, so anything from a simulator can carry its pill. */
export function useLabDevices(): Set<string> {
  const devices = useDevices();
  return new Set(
    (devices.data ?? []).filter((device) => device.lab !== undefined && device.lab !== null).map((device) => device.id),
  );
}

export function useRuns(limit = 50) {
  return useRpc<Run[]>('runner.runs', { limit }, { invalidateOn: ['automation.runs.changed'] });
}

export function useRun(runId: string | null) {
  return useRpc<RunDetail>(
    'runner.get',
    { run_id: runId },
    { enabled: runId !== null, invalidateOn: ['automation.runs.changed'] },
  );
}

/**
 * One playbook run. `playbooks.run_get` answers the same `RunDetail` as
 * `runner.get`; a playbook run is read through its own feature's method so a
 * build that gains or loses one of the two still works.
 */
export function usePlaybookRun(runId: string | null) {
  return useRpc<RunDetail>(
    'playbooks.run_get',
    { run_id: runId },
    { enabled: runId !== null, invalidateOn: ['automation.runs.changed'] },
  );
}

export function useTasks() {
  return useRpc<Task[]>('tasks.list', undefined, {
    invalidateOn: ['tasks.changed', 'automation.runs.changed'],
  });
}

export function useTemplates() {
  return useRpc<Template[]>('templates.list', undefined, { invalidateOn: ['templates.changed'] });
}

export function usePlaybooks() {
  return useRpc<Playbook[]>('playbooks.list', undefined, { invalidateOn: ['playbooks.changed'] });
}

export function useSchedules() {
  return useRpc<Schedule[]>('schedules.list', undefined, {
    invalidateOn: ['schedules.changed', 'schedules.fired'],
  });
}

export function useTriggers() {
  return useRpc<Trigger[]>('triggers.list', undefined, {
    invalidateOn: ['triggers.changed', 'triggers.fired'],
  });
}

export function useAnsibleStatus() {
  return useRpc<AnsibleStatus>('ansible.status');
}

/**
 * What a selection means right now (`tasks.devices`). The same method answers
 * for a saved task and for one being written, which is exactly what a filter
 * editor needs: the count under the form is the truth, not an estimate.
 */
export function useSelectedDevices(selector: Selector | null) {
  return useRpc<SelectedDevices>(
    'tasks.devices',
    selector === null ? undefined : selector,
    { enabled: selector !== null, invalidateOn: ['devices.changed'] },
  );
}
