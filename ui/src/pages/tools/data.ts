/**
 * Everything the Tools screen reads and writes.
 *
 * The screen is in three halves (the catalogue says which is which):
 *
 * * **Calculators** are read like any other query — `useRpc(method, params)`
 *   with the form's values as the parameters, debounced. React Query keys
 *   queries by `[method, params]`, so typing `10.0.0.0/2` and then `/24` and
 *   then going back to `/2` costs two calls, not three, and the answer for an
 *   input already seen appears without a round trip. That is what makes
 *   "answers as you type" affordable against a real backend.
 * * **Probes** are jobs or single calls. A job's progress comes from
 *   `jobs.changed` and its echoes from the feature's own topics; its failure
 *   *code* comes from the probe history record, which is the only place it
 *   survives (see `probes.ts`).
 * * **The servers** keep running when this screen is closed, so their state is
 *   read from the backend (`tftp.status`, `probe.syslog.status`) rather than
 *   held here, and the screen re-attaches to whatever it finds.
 *
 * The last inputs of each tool are remembered in `localStorage` — except for
 * the tools whose input is a secret somebody typed, which are never written
 * anywhere ({@link useRemembered}).
 */

import { useQueries } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { rpc } from '@/api/client';
import { rpcQueryKey, useRpc, type UseRpcOptions } from '@/api/hooks';
import type { DeviceRow, Job } from '@/api/types/core';
import type {
  History,
  HistoryParams,
  Messages,
  ProbeRun,
  ReferenceTable,
  SyslogStatus,
  TableInfo,
  TftpStatus,
  Tool,
} from '@/api/types/toolbox';
import { formatCount } from '@/lib/format';
import { readStored, storageKey, writeStored } from '@/lib/storage';
import {
  initialValues,
  sourcesOf,
  SPECS,
  type Field,
  type Spec,
  type TableRead,
  type Values,
} from './specs';

export type { TableRead } from './specs';

/** The catalogue. One read, cached for the session: it cannot change at runtime. */
export function useCatalogue() {
  return useRpc<Tool[]>('tools.list');
}

/**
 * `app.info` carries `local_access`: whether whoever is calling sits at the
 * machine the engine runs on. The two servers are refused without it, so the
 * screen asks once and says so instead of offering a button that is always
 * refused. (`AppInfo` in `api/types/core.ts` does not declare the field yet —
 * see NOTES.md — so it is read through a narrow type of its own.)
 */
interface LocalAccess {
  local_access?: boolean;
}

export function useLocalAccess() {
  const info = useRpc<LocalAccess>('app.info');
  return {
    /** `undefined` until it is known: a server panel waits rather than guessing. */
    local: info.data === undefined ? undefined : info.data.local_access !== false,
    pending: info.isPending,
  };
}

/** One tool's answer. `params: null` holds the call back until the form is answerable. */
export function useToolResult<T>(
  method: string,
  params: Record<string, unknown> | null,
  options: UseRpcOptions = {},
) {
  return useRpc<T>(method, params ?? undefined, { ...options, enabled: params !== null });
}

/** The inventory, for the device pickers. Asked for only where one is shown. */
export function useDevices(enabled: boolean) {
  return useRpc<DeviceRow[]>('devices.list', undefined, {
    enabled,
    invalidateOn: ['devices.changed'],
  });
}

/** The reference tables there are, for the Reference tool's chooser. */
export function useReferenceTables(enabled: boolean) {
  return useRpc<TableInfo[]>('tools.reference.tables', undefined, { enabled });
}

/**
 * How many rows of a reference table are asked for at once: by the Reference
 * panel, and by a form whose choices are a table's rows. The same number in
 * both places, so the two share one cached read.
 */
export const TABLE_ROWS = 2000;

/**
 * The reference tables a form's choices are read from, by table id.
 *
 * A codec picker is the engine's `voice_codecs` table and not a list kept
 * here, for the reason the drawer is `tools.list`: the calculator and the
 * picker then cannot disagree about what there is. A form with no such field
 * asks for nothing.
 */
export function useTables(fields: readonly Field[]): Record<string, TableRead> {
  const tables = sourcesOf(fields);
  const reads = useQueries({
    queries: tables.map((table) => {
      const params = { table, limit: TABLE_ROWS };
      return {
        queryKey: rpcQueryKey('tools.reference', params),
        queryFn: () => rpc<ReferenceTable>('tools.reference', params),
      };
    }),
  });
  return Object.fromEntries(
    tables.map((table, index) => {
      const read = reads[index];
      const data = read?.data;
      return [
        table,
        {
          rows: data?.rows,
          error: read?.error ? read.error.message : undefined,
          partial:
            data?.truncated === true
              ? `The engine has ${formatCount(data.total)} of these; the first ${formatCount(data.rows.length)} are offered here.`
              : undefined,
        },
      ];
    }),
  );
}

/** A job, while it runs and once it has stopped. */
export function useJob(id: string | null) {
  return useRpc<Job>('jobs.get', { id }, { enabled: id !== null, invalidateOn: ['jobs.changed'] });
}

/**
 * One probe's history record. This is where a failure's `error_code` is, so a
 * failed probe is read back rather than reported from the job's sentence
 * alone.
 */
export function useRun(id: string | null) {
  return useRpc<ProbeRun>('probe.history.get', { id }, { enabled: id !== null, invalidateOn: ['probe.ran'] });
}

export function useHistory(params: HistoryParams) {
  return useRpc<History>('probe.history', params, { invalidateOn: ['probe.ran'] });
}

export function useTftpStatus() {
  return useRpc<TftpStatus>('tftp.status', undefined, {
    invalidateOn: ['tftp.transfer', 'jobs.changed'],
  });
}

export function useSyslogStatus() {
  return useRpc<SyslogStatus>('probe.syslog.status', undefined, {
    invalidateOn: ['probe.syslog.message', 'jobs.changed'],
  });
}

export function useSyslogMessages(enabled: boolean, limit = 200) {
  return useRpc<Messages>('probe.syslog.messages', { limit }, {
    enabled,
    invalidateOn: ['probe.syslog.message'],
  });
}

// ------------------------------------------------------------- the form

/** Settles after `delay` without a change: what "answers as you type" needs. */
export function useDebounced<T>(value: T, delay = 180): T {
  const [settled, setSettled] = useState(value);
  const key = JSON.stringify(value);
  const latest = useRef(value);
  latest.current = value;
  useEffect(() => {
    const timer = setTimeout(() => setSettled(latest.current), delay);
    return () => clearTimeout(timer);
  }, [key, delay]);
  return settled;
}

const REMEMBERED = (id: string) => storageKey(`tools.${id}`);

function isValues(value: unknown): value is Values {
  return (
    typeof value === 'object' &&
    value !== null &&
    Object.values(value).every((entry) => typeof entry === 'string')
  );
}

export interface Form {
  values: Values;
  set: (name: string, value: string) => void;
  /** Back to the tool's own defaults, and forget what was remembered. */
  clear: () => void;
  /** True while the fields hold nothing but the defaults. */
  untouched: boolean;
}

/**
 * One tool's form, remembered between visits — unless its input is a secret.
 *
 * A pasted `password 7 …` line, a password being generated and a key's comment
 * are never written to storage: `Spec.secret` says so, and the only way to
 * keep that promise is to not have the code that would break it. Everything
 * else is remembered, because typing a /22 again to see its subnets is the
 * kind of thing that makes a tool drawer annoying.
 */
export function useForm(id: string, spec: Spec | undefined): Form {
  const defaults = initialValues(spec);
  const remember = spec !== undefined && spec.secret !== true;
  const load = (): Values => ({
    ...defaults,
    ...(remember ? (readStored(REMEMBERED(id), isValues) ?? {}) : {}),
  });

  const [held, setHeld] = useState<{ id: string; values: Values }>(() => ({ id, values: load() }));
  // The tool changed: start from its own defaults and from what was remembered
  // for *that* tool, so switching tools never carries values across. Setting
  // state during render on a changed key is React's own pattern for this; it
  // re-renders before paint.
  const values = held.id === id ? held.values : load();
  if (held.id !== id) setHeld({ id, values });

  const write = (next: Values) => {
    setHeld({ id, values: next });
    if (remember) writeStored(REMEMBERED(id), next);
  };

  return {
    values,
    set: (name, value) => write({ ...values, [name]: value }),
    clear: () => write(defaults),
    untouched: Object.entries(defaults).every(([name, value]) => (values[name] ?? '') === value),
  };
}

/**
 * Hands a tool's form these values before it is opened, as if they had been
 * typed there: how a row of a reference table leads to the calculator that
 * takes it. What the form already remembers is kept, so opening the voice
 * calculator from a codec row does not forget the number of calls.
 *
 * `false` when nothing was handed over: no such form, or a form whose inputs
 * are secrets — those are never written anywhere, from here either.
 */
export function prefill(id: string, values: Values): boolean {
  const spec = SPECS[id];
  if (spec === undefined || spec.secret === true) return false;
  const remembered = readStored(REMEMBERED(id), isValues) ?? {};
  writeStored(REMEMBERED(id), { ...initialValues(spec), ...remembered, ...values });
  return true;
}

/** The spec of a tool, when this screen has one. */
export function specFor(id: string): Spec | undefined {
  return SPECS[id];
}
