/**
 * What "keep as evidence" asks the engine for, with no React in it.
 *
 * The one rule this file exists to keep: **the client never supplies the
 * content.** A request names what to capture — series and a period, a
 * graph, the last lines of a terminal session, a piece of a session log —
 * and the engine reads it again from its own store and keeps what *it*
 * read, redacted, with a source and a hash. Nothing that is on screen is
 * uploaded, so nothing here can make a record of something the engine did
 * not see.
 *
 * There is one request that cannot keep that rule, and it is a kind of its
 * own so that it can never pass for the others: `text`, for text a person
 * holds (a selection in a terminal, a clip). The engine did not read it, so
 * it stores it as supplied by the operator and says so wherever the item is
 * shown; the dialog says so before it is sent.
 */

import { RpcError } from '@/api/errors';
import type {
  EvidenceAddTextParams,
  EvidenceCaptureChartParams,
  EvidenceCaptureLogParams,
  EvidenceCaptureTerminalParams,
  EvidenceCaptureTopologyParams,
  EvidenceLimitDetails,
  EvidenceSeriesRef,
} from '@/api/types/ops';
import { formatBytes, formatCount, plural } from '@/lib/format';
import { formatDateTime } from '@/lib/time';

/** A chart to keep: exactly the series and the period it was drawn from. */
export interface ChartEvidenceRequest {
  kind: 'chart';
  /** `{device_id, metric, instance?}` per line of the chart, in its order. */
  series: readonly EvidenceSeriesRef[];
  /** The start of the range on screen, in milliseconds. */
  since: number;
  /**
   * The end of the range. The engine needs one: a chart that runs "until
   * now" passes the moment the person asked.
   */
  until: number;
  /** The bucket width the chart asked for, so the stored points are the ones that were drawn. */
  step_s?: number;
  /** Offered as the title. Left empty, the engine names the item itself. */
  title?: string;
}

/** A drawing to keep: the same graph `topology.export` is asked for. */
export interface TopologyEvidenceRequest {
  kind: 'topology';
  view?: string;
  layer?: unknown;
  filter?: unknown;
  include_endpoints?: boolean;
  title?: string;
}

/**
 * The last lines of a terminal session. The engine asks the session itself
 * (`terminal.snapshot`); nothing that is on the screen is sent.
 */
export interface TerminalEvidenceRequest {
  kind: 'terminal';
  session_id: string;
  /** Lines from the end, 1 to 10,000. Where the dialog starts; the person can change it there. */
  lines: number;
  /** The session's title, for the dialog. Not sent: the engine looks the session up. */
  session?: string;
  title?: string;
}

/**
 * A piece of a session log: the bytes of a page that was read with
 * `terminal.logs.read`. The engine reads them again itself, redacted.
 */
export interface LogEvidenceRequest {
  kind: 'log';
  /** The log's name, as `terminal.logs.list` gives it. */
  name: string;
  /** Where the page starts: the `offset` of the answer that was shown, which is a line start. */
  offset: number;
  /** The page's length in bytes of the file (`next_offset - offset`), at most 262,144. */
  limit: number;
  /** The session the log is of, for the dialog. Not sent. */
  log?: string;
  title?: string;
}

/**
 * Text the person holds. It is sent as it is, redacted by the engine on
 * arrival and stored as supplied by the operator — the engine did not read
 * it, and the item says so.
 */
export interface TextEvidenceRequest {
  kind: 'text';
  text: string;
  /** The engine requires one. Offered in the form, where it can be changed. */
  title: string;
  /** The session the text is said to be from: recorded as the person's word. */
  session_id?: string;
  device_id?: string;
  /** At most 1,000 characters. */
  note?: string;
  /** Where the person took it from, for the dialog: "the selection in SW1". Not sent. */
  from?: string;
}

export type KeepRequest =
  | ChartEvidenceRequest
  | TopologyEvidenceRequest
  | TerminalEvidenceRequest
  | LogEvidenceRequest
  | TextEvidenceRequest;

export const CAPTURE_METHOD: Record<KeepRequest['kind'], string> = {
  chart: 'evidence.capture_chart',
  topology: 'evidence.capture_topology',
  terminal: 'evidence.capture_terminal',
  log: 'evidence.capture_log',
  text: 'evidence.add_text',
};

export type CaptureParams =
  | EvidenceCaptureChartParams
  | EvidenceCaptureTopologyParams
  | EvidenceCaptureTerminalParams
  | EvidenceCaptureLogParams
  | EvidenceAddTextParams;

/**
 * The method and the parameters for a request. A title that is empty is not
 * sent, so the engine's own name for the item is used; an `undefined` field
 * is not sent either, because the recorded request is what the item says it
 * was asked for.
 */
export function captureCall(request: KeepRequest, title: string): { method: string; params: CaptureParams } {
  const named = title.trim() === '' ? {} : { title: title.trim() };
  if (request.kind === 'terminal') {
    return {
      method: CAPTURE_METHOD.terminal,
      params: { session_id: request.session_id, lines: request.lines, ...named },
    };
  }
  if (request.kind === 'log') {
    return {
      method: CAPTURE_METHOD.log,
      params: { name: request.name, offset: request.offset, limit: request.limit, ...named },
    };
  }
  if (request.kind === 'text') {
    return {
      method: CAPTURE_METHOD.text,
      params: {
        // Exactly as the person holds it: what may be stored is the engine's
        // to decide, and it decides after it has redacted.
        text: request.text,
        // The engine requires a title here and refuses an empty one itself.
        title: title.trim(),
        ...(request.session_id ? { session_id: request.session_id } : {}),
        ...(request.device_id ? { device_id: request.device_id } : {}),
        ...(request.note?.trim() ? { note: request.note.trim() } : {}),
      },
    };
  }
  if (request.kind === 'chart') {
    return {
      method: CAPTURE_METHOD.chart,
      params: {
        series: request.series.map(({ device_id, metric, instance }) =>
          instance === undefined || instance === '' ? { device_id, metric } : { device_id, metric, instance },
        ),
        since: Math.floor(request.since),
        until: Math.floor(request.until),
        ...(request.step_s === undefined ? {} : { step_s: request.step_s }),
        ...named,
      },
    };
  }
  return {
    method: CAPTURE_METHOD.topology,
    params: {
      ...(request.view === undefined ? {} : { view: request.view }),
      ...(request.layer === undefined ? {} : { layer: request.layer }),
      ...(request.filter === undefined ? {} : { filter: request.filter }),
      ...(request.include_endpoints === undefined ? {} : { include_endpoints: request.include_endpoints }),
      ...named,
    },
  };
}

/** What will be kept, as facts a person can check before pressing the button. */
export function describeRequest(request: KeepRequest): { label: string; value: string }[] {
  if (request.kind === 'terminal') {
    return [
      { label: 'Session', value: request.session ?? request.session_id },
      {
        label: 'Lines',
        value: `the last ${formatCount(request.lines)}, as the session holds them at the moment it is kept`,
      },
      { label: 'Read with', value: 'terminal.snapshot' },
    ];
  }
  if (request.kind === 'log') {
    return [
      { label: 'Log', value: request.log ? `${request.log} — ${request.name}` : request.name },
      {
        label: 'Bytes',
        value: `${formatCount(request.offset)} to ${formatCount(request.offset + request.limit)} of the file`,
      },
      { label: 'Read with', value: 'terminal.logs.read' },
    ];
  }
  if (request.kind === 'text') {
    const lines = request.text === '' ? 0 : request.text.replace(/\r\n?/g, '\n').replace(/\n$/, '').split('\n').length;
    return [
      {
        label: 'Text',
        value: `${plural(lines, 'line')}, ${formatBytes(new TextEncoder().encode(request.text).length)}, as you hold it`,
      },
      { label: 'From', value: request.from ?? 'not said' },
      { label: 'Supplied by', value: 'you — the engine did not read this itself, and the item will say so' },
    ];
  }
  if (request.kind === 'chart') {
    const devices = new Set(request.series.map((entry) => entry.device_id));
    const names = request.series
      .slice(0, 4)
      .map((entry) => (entry.instance ? `${entry.metric} · ${entry.instance}` : entry.metric));
    const more = request.series.length - names.length;
    return [
      {
        label: 'Series',
        value: `${names.join(', ')}${more > 0 ? `, and ${formatCount(more)} more` : ''} (${plural(request.series.length, 'series', 'series')} on ${plural(devices.size, 'device')})`,
      },
      {
        label: 'Period',
        value: `${formatDateTime(request.since)} to ${formatDateTime(request.until)}`,
      },
      { label: 'Read with', value: 'metrics.query_many' },
    ];
  }
  const filtered = request.filter !== undefined && request.filter !== null && JSON.stringify(request.filter) !== '{}';
  return [
    {
      label: 'Graph',
      value: [
        request.view ? `view “${request.view}”` : 'the default view',
        typeof request.layer === 'string' ? `${request.layer} layer` : null,
        filtered ? 'with the filter that is on screen' : 'unfiltered',
        request.include_endpoints ? 'endpoints included' : null,
      ]
        .filter(Boolean)
        .join(', '),
    },
    { label: 'Read with', value: 'topology.export' },
  ];
}

/** Where an evidence item is shown. */
export function evidenceLink(id: string): string {
  return `/ops/evidence?item=${encodeURIComponent(id)}`;
}

export interface Refusal {
  /** What kind of "no" it was, in a few words. */
  headline: string;
  /** The engine's own sentence: only the feature that refused knows why. */
  message: string;
  /** Nothing was stored. True for every refusal of a capture. */
  nothingStored: true;
}

function isLimit(details: unknown): details is EvidenceLimitDetails {
  return (
    typeof details === 'object' &&
    details !== null &&
    typeof (details as { limit?: unknown }).limit === 'string' &&
    typeof (details as { max?: unknown }).max === 'number'
  );
}

const LIMIT_WORD: Record<EvidenceLimitDetails['limit'], string> = {
  text_bytes: 'Too much text for one item',
  svg_bytes: 'The drawing is too large for one item',
  points: 'Too many points for one item',
  items: 'The evidence store is full',
  params_bytes: 'The request is too large to record',
};

/**
 * A failed capture, in the engine's words under a headline per kind of
 * refusal. `unavailable` is the one a screen must not paraphrase: it names
 * the method that is missing or did not answer.
 */
export function refusal(error: unknown): Refusal {
  const failure = error instanceof RpcError ? error : null;
  const message = failure?.message ?? (error instanceof Error ? error.message : 'It could not be kept.');
  let headline = 'It could not be kept';
  if (failure?.code === 'unavailable') headline = 'The engine could not read it';
  else if (failure?.code === 'unknown_method') headline = 'Evidence is not in this build';
  else if (failure?.code === 'not_found') headline = 'What it was to be read from is gone';
  else if (failure?.code === 'invalid') {
    headline = isLimit(failure.details) ? (LIMIT_WORD[failure.details.limit] ?? 'It was refused') : 'It was refused';
  }
  return { headline, message, nothingStored: true };
}
