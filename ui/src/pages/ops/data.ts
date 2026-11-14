/**
 * Everything the Operations screen reads and writes.
 *
 * All of it is `nocturne-ops` (`docs/dev/features/ops.md`), except the job
 * history, which is the core's own `jobs.*`, and three reads that only exist
 * to turn an id into a name: `devices.list`, `alerts.list`, `changes.list`.
 * Those three belong to other features, so every hook that uses one is
 * written to work without it — the backend already reports a missing source
 * inside its own answers, and `useHasMethod` decides whether a *control* that
 * needs one is offered at all.
 *
 * Nothing here polls on a timer. Each read names the events that can change
 * it, which is enough: an incident's own record moves on `incidents.changed`,
 * and the sources its timeline gathers from announce themselves too, so a
 * note typed in another window appears here without a refresh.
 */

import { useEffect, useRef, useState } from 'react';
import type { UseMutationResult, UseQueryResult } from '@tanstack/react-query';
import { rpc, subscribe } from '@/api/client';
import type { RpcError } from '@/api/errors';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type { AiEventEnvelope, AiStatus, Chat } from '@/api/types/ai';
import type { Alert } from '@/api/types/monitor';
import type { ChangeSummary } from '@/api/types/changes';
import type { Device, Job, JobFilter } from '@/api/types/core';
import type {
  AuditActionsResult,
  AuditSearchParams,
  AuditSearchResult,
  GeneratedReport,
  Handover,
  HandoverHead,
  Incident,
  IncidentsCreateParams,
  IncidentsListParams,
  IncidentsLinkResult,
  IncidentsListResult,
  IncidentsNoteParams,
  IncidentsSuggestResult,
  Note,
  NoteHead,
  Rendered,
  ReportHead,
  ReportsGenerateParams,
  ReportsKindsResult,
  ReportsSaveParams,
  SavedHandover,
  SavedReport,
} from '@/api/types/ops';

/** The topics that can change an incident, including the ones it gathers from. */
const INCIDENT_TOPICS = ['incidents.changed', 'alerts.changed', 'changes.changed', 'audit.appended'];

// ------------------------------------------------------------------ incidents

export function useIncidents(params: IncidentsListParams) {
  return useRpc<IncidentsListResult>('incidents.list', params, { invalidateOn: INCIDENT_TOPICS });
}

export function useIncident(id: string | null) {
  return useRpc<Incident>('incidents.get', id ? { id } : undefined, {
    enabled: id !== null,
    invalidateOn: INCIDENT_TOPICS,
  });
}

/**
 * What is inside the incident's window that nobody has linked yet.
 *
 * This is the one read on the screen that can be slow — it asks the alerting
 * and change features and reads the snapshot table — so it is a read of its
 * own rather than part of the incident, and the timeline draws without
 * waiting for it.
 */
export function useSuggestedLinks(id: string | null, enabled: boolean) {
  return useRpc<IncidentsSuggestResult>('incidents.suggest_links', id ? { id } : undefined, {
    enabled: id !== null && enabled,
    invalidateOn: ['incidents.changed', 'config.changed'],
  });
}

const INCIDENT_WRITES = ['incidents.', 'overview.', 'handover.', 'audit.'];

function useIncidentMutation<P>(method: string, errorToast: string) {
  return useRpcMutation<Incident, P>(method, { invalidates: INCIDENT_WRITES, errorToast });
}

export function useCreateIncident() {
  return useIncidentMutation<IncidentsCreateParams>('incidents.create', 'The incident could not be opened');
}

export function useUpdateIncident() {
  return useIncidentMutation<Record<string, unknown>>('incidents.update', 'That change could not be saved');
}

export function useAddNote() {
  return useIncidentMutation<IncidentsNoteParams>('incidents.note', 'The note could not be added');
}

/** `incidents.link` and `.sync` answer with the incident *and* what the call did. */
export function useLinkToIncident() {
  return useRpcMutation<IncidentsLinkResult, Record<string, unknown>>('incidents.link', {
    invalidates: INCIDENT_WRITES,
    errorToast: 'That could not be linked',
  });
}

export function useSyncIncident() {
  return useRpcMutation<IncidentsLinkResult, { id: string }>('incidents.sync', {
    invalidates: INCIDENT_WRITES,
    errorToast: 'The timeline could not be read again',
  });
}

export function useAcknowledgeIncident() {
  return useIncidentMutation<{ id: string; author?: string }>(
    'incidents.acknowledge',
    'The incident could not be acknowledged',
  );
}

export function useResolveIncident() {
  return useIncidentMutation<{ id: string; root_cause: string; author?: string }>(
    'incidents.resolve',
    'The incident could not be resolved',
  );
}

export function useReopenIncident() {
  return useIncidentMutation<{ id: string; why: string; author?: string }>(
    'incidents.reopen',
    'The incident could not be reopened',
  );
}

export function useDeleteIncident() {
  return useRpcMutation<{ deleted: boolean }, { id: string }>('incidents.delete', {
    invalidates: INCIDENT_WRITES,
    errorToast: 'The incident could not be deleted',
  });
}

// ------------------------------------------------------------------- handover

export function useHandover(since: number, until: number | null, enabled = true) {
  return useRpc<Handover>(
    'handover.generate',
    until === null ? { since } : { since, until },
    { enabled, invalidateOn: ['incidents.changed', 'alerts.changed', 'changes.changed'] },
  );
}

export function useSavedHandovers() {
  return useRpc<{ handovers: HandoverHead[]; next_cursor: string | null }>(
    'handover.list',
    { limit: 50 },
    { invalidateOn: ['handover.changed'] },
  );
}

export function useSavedHandover(id: string | null) {
  return useRpc<SavedHandover>('handover.get', id ? { id } : undefined, {
    enabled: id !== null,
    invalidateOn: ['handover.changed'],
  });
}

export function useSaveHandover() {
  return useRpcMutation<SavedHandover, { since: number; until: number; notes: string; id?: string }>(
    'handover.save',
    {
      invalidates: ['handover.', 'audit.'],
      successToast: 'The handover is saved',
      errorToast: 'The handover could not be saved',
    },
  );
}

export function useDeleteHandover() {
  return useRpcMutation<{ deleted: boolean }, { id: string }>('handover.delete', {
    invalidates: ['handover.', 'audit.'],
    errorToast: 'That saved handover could not be deleted',
  });
}

// -------------------------------------------------------------------- reports

export function useReportKinds() {
  return useRpc<ReportsKindsResult>('reports.kinds');
}

export function useReport(params: ReportsGenerateParams | null) {
  return useRpc<GeneratedReport>('reports.generate', params ?? undefined, {
    enabled: params !== null,
  });
}

export function useSavedReports() {
  return useRpc<{ reports: ReportHead[]; next_cursor: string | null }>(
    'reports.list',
    { limit: 50 },
    { invalidateOn: ['reports.changed'] },
  );
}

export function useSavedReport(id: string | null) {
  return useRpc<SavedReport>('reports.get', id ? { id } : undefined, { enabled: id !== null });
}

export function useSaveReport() {
  return useRpcMutation<SavedReport, ReportsSaveParams>('reports.save', {
    invalidates: ['reports.', 'audit.'],
    successToast: 'The report is kept',
    errorToast: 'The report could not be kept',
  });
}

export function useDeleteReport() {
  return useRpcMutation<{ deleted: boolean }, { id: string }>('reports.delete', {
    invalidates: ['reports.', 'audit.'],
    errorToast: 'That report could not be deleted',
  });
}

// ---------------------------------------------------------------------- notes

export function useNotes(params: { query?: string; tag?: string; device_id?: string }) {
  return useRpc<{ notes: NoteHead[]; next_cursor: string | null }>(
    'notes.list',
    { ...params, limit: 100 },
    { invalidateOn: ['notes.changed'] },
  );
}

export function useNote(id: string | null) {
  return useRpc<Note>('notes.get', id ? { id } : undefined, {
    enabled: id !== null,
    invalidateOn: ['notes.changed', 'devices.changed'],
  });
}

export function useSaveNote() {
  return useRpcMutation<Note, Record<string, unknown>>('notes.save', {
    invalidates: ['notes.', 'audit.'],
    errorToast: 'The note could not be saved',
  });
}

export function useDeleteNote() {
  return useRpcMutation<{ deleted: boolean }, { id: string }>('notes.delete', {
    invalidates: ['notes.', 'audit.'],
    errorToast: 'The note could not be deleted',
  });
}

// ---------------------------------------------------------------------- audit

export function useAuditTrail(params: AuditSearchParams) {
  return useRpc<AuditSearchResult>('audit.search', params, { invalidateOn: ['audit.appended'] });
}

export function useAuditActions() {
  return useRpc<AuditActionsResult>('audit.actions', {}, { invalidateOn: ['audit.appended'] });
}

// ----------------------------------------------------------------------- jobs

export function useJobs(filter: JobFilter) {
  return useRpc<Job[]>('jobs.list', filter, { invalidateOn: ['jobs.changed'] });
}

export function useJob(id: string | null) {
  return useRpc<Job>('jobs.get', id ? { id } : undefined, {
    enabled: id !== null,
    invalidateOn: ['jobs.changed', 'jobs.log'],
  });
}

export function useCancelJob() {
  return useRpcMutation<{ cancelled: boolean }, { id: string }>('jobs.cancel', {
    invalidates: ['jobs.'],
    errorToast: 'That job could not be cancelled',
  });
}

// ------------------------------------------------------------- other features

/**
 * The inventory, for names and for the device picker. A failure is not an
 * error on this screen: the records carry their own device references, and
 * the picker simply says it cannot offer a list.
 */
export function useDevices() {
  return useRpc<Device[]>('devices.list', undefined, {
    invalidateOn: ['devices.changed', 'devices.state'],
  });
}

/** Open alerts, for opening an incident from one. Only called where it exists. */
export function useOpenAlerts(enabled: boolean) {
  return useRpc<Alert[]>('alerts.list', { state: 'open', limit: 200 }, {
    enabled,
    invalidateOn: ['alerts.changed', 'alerts.raised'],
  });
}

/** Recent changes, for linking one to an incident by hand. */
export function useRecentChanges(enabled: boolean) {
  return useRpc<ChangeSummary[]>('changes.list', { limit: 100 }, {
    enabled,
    invalidateOn: ['changes.changed'],
  });
}

// ------------------------------------------------------------- the exports

export type ExportMethod = 'incidents.export' | 'handover.export';

/** A rendered document, asked for only when a dialog is open. */
export function useRendered(
  method: ExportMethod,
  params: Record<string, unknown> | null,
): UseQueryResult<Rendered, RpcError> {
  return useRpc<Rendered>(method, params ?? undefined, { enabled: params !== null });
}

/** Saves text as a file. What is saved is exactly what was shown. */
export function download(filename: string, text: string): void {
  const url = URL.createObjectURL(new Blob([text], { type: 'text/plain;charset=utf-8' }));
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = filename;
  anchor.click();
  URL.revokeObjectURL(url);
}

// ------------------------------------------------------------------ assistant

export function useAiStatus(enabled: boolean) {
  return useRpc<AiStatus>('ai.status', undefined, {
    enabled,
    invalidateOn: ['ai.providers.changed'],
  });
}

export interface TidyRequest {
  /** The prose to tidy. Nothing else of the handover is sent. */
  notes: string;
  /** What the period was, so the model can write about the right shift. */
  period: { since: number; until: number };
}

export interface TidyState {
  /** What the model has written so far. Empty until it starts. */
  text: string;
  running: boolean;
  error: RpcError | string | null;
  /** The chat it is happening in, so the dock can be opened on it. */
  chatId: string | null;
  ask: (request: TidyRequest) => void;
  discard: () => void;
}

/**
 * *Ask the assistant to tidy my notes.*
 *
 * The model is given **only the prose** — never the generated sections — and
 * what comes back is a *proposal*: it is shown tinted `--ai` beside the
 * original, and nothing replaces the notes until a person presses Use. That
 * is not politeness: `handover.save` regenerates every data section on the
 * server, so a model could not write a number into a handover even if it
 * tried, and this keeps the same rule over the one field it can write.
 *
 * `ai.send` answers at once and the turn arrives on `ai.event`, so this
 * follows that stream to the `done` event and then stops.
 */
export function useTidyNotes(): TidyState {
  const [text, setText] = useState('');
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<RpcError | string | null>(null);
  const [chatId, setChatId] = useState<string | null>(null);
  const turn = useRef<string | null>(null);

  useEffect(() => {
    if (!running || chatId === null) return;
    return subscribe<AiEventEnvelope>('ai.event', ({ payload }) => {
      if (payload.chat_id !== chatId) return;
      if (turn.current !== null && payload.turn_id !== turn.current) return;
      const event = payload.event;
      if (event.type === 'text') setText((before) => before + event.delta);
      else if (event.type === 'error') setError(event.message);
      else if (event.type === 'done') {
        setRunning(false);
        if (event.stop === 'refused') setError('The model declined to answer.');
      }
    });
  }, [running, chatId]);

  const ask = (request: TidyRequest) => {
    setText('');
    setError(null);
    setRunning(true);
    void (async () => {
      try {
        const chat = await rpc<Chat>('ai.chats.create', {});
        setChatId(chat.id);
        const started = await rpc<{ turn_id: string }>('ai.send', {
          chat_id: chat.id,
          text:
            'Tidy the wording of these shift-handover notes. Keep every fact, every ' +
            'device name and every number exactly as written; do not add anything, do ' +
            'not look anything up, and answer with the tidied notes only.\n\n' +
            `The shift ran from ${new Date(request.period.since).toISOString()} to ` +
            `${new Date(request.period.until).toISOString()}.\n\n` +
            request.notes,
          context: { page: '/ops/handover' },
        });
        turn.current = started.turn_id;
      } catch (failure) {
        setRunning(false);
        setError(failure as RpcError);
      }
    })();
  };

  return {
    text,
    running,
    error,
    chatId,
    ask,
    discard: () => {
      setText('');
      setError(null);
      setRunning(false);
    },
  };
}

export type { UseMutationResult };
