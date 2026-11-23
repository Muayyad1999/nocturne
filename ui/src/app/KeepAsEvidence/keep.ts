/**
 * What "keep as evidence" asks the engine for, with no React in it.
 *
 * The one rule this file exists to keep: **the client never supplies the
 * content.** A request names what to capture — series and a period, or a
 * graph — and the engine reads it again from its own store and keeps what
 * *it* read, redacted, with a source and a hash. Nothing that is on screen is
 * uploaded, so nothing here can make a record of something the engine did not
 * see.
 */

import { RpcError } from '@/api/errors';
import type {
  EvidenceCaptureChartParams,
  EvidenceCaptureTopologyParams,
  EvidenceLimitDetails,
  EvidenceSeriesRef,
} from '@/api/types/ops';
import { formatCount, plural } from '@/lib/format';
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

export type KeepRequest = ChartEvidenceRequest | TopologyEvidenceRequest;

export const CAPTURE_METHOD: Record<KeepRequest['kind'], string> = {
  chart: 'evidence.capture_chart',
  topology: 'evidence.capture_topology',
};

/**
 * The method and the parameters for a request. A title that is empty is not
 * sent, so the engine's own name for the item is used; an `undefined` field
 * is not sent either, because the recorded request is what the item says it
 * was asked for.
 */
export function captureCall(
  request: KeepRequest,
  title: string,
): { method: string; params: EvidenceCaptureChartParams | EvidenceCaptureTopologyParams } {
  const named = title.trim() === '' ? {} : { title: title.trim() };
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
