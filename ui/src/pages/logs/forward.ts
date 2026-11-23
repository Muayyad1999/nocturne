/**
 * The reasoning behind the forwarding pane.
 *
 * A forwarder hands everything that is stored on to a log platform. Two
 * rules of the engine shape everything here:
 *
 * * **The destination of an HTTPS forwarder is write-only.** The URL and the
 *   token are one secret in the vault. `logs.forward.save` takes them and no
 *   method returns them — only `has_secret`. So the form never has a stored
 *   URL to show, and {@link saveParams} sends `url` and `token` only when
 *   something was typed: left out, what is stored stays. A new URL does not
 *   inherit the stored token (it would be sent to whoever is at the new
 *   address), and {@link tokenFate} says so before the save rather than
 *   after.
 * * **The document is replaced by what is sent.** Everything but `enabled`
 *   and the secret takes its default when it is left out, so an edit sends
 *   the whole form again, every time.
 *
 * What a forwarder has done — `sent`, `dropped`, the last error — is not in
 * the document at all: it is in `logs.status`, counted since the application
 * started, and {@link rowsOf} joins the two by id.
 */

import type {
  ForwardKind,
  ForwardStream,
  Forwarder,
  ForwarderSaveParams,
  ForwarderState,
  ForwarderStatus,
} from '@/api/types/logs';
import type { StatusTone } from '@/design/tone';
import { SEVERITY_NAMES } from './query';

export const FORWARD_KINDS: readonly {
  value: ForwardKind;
  label: string;
  help: string;
  /** The port the engine uses when none is given. */
  defaultPort?: number;
}[] = [
  {
    value: 'syslog_udp',
    label: 'Syslog over UDP',
    help: 'One datagram per message, at most 8,192 bytes. Each is tried once; nothing is tried again.',
    defaultPort: 514,
  },
  {
    value: 'syslog_tcp',
    label: 'Syslog over TCP',
    help: 'One message to a line. Not encrypted: what is stored leaves this machine readable.',
    defaultPort: 514,
  },
  {
    value: 'syslog_tls',
    label: 'Syslog over TLS',
    help: 'Octet-counted, as RFC 5425 requires. The collector’s certificate is always checked.',
    defaultPort: 6514,
  },
  {
    value: 'https_json',
    label: 'HTTPS, JSON',
    help: 'One POST per batch, in an HTTP event collector’s envelope. The token is sent as Authorization: Splunk <token> unless it is written with its own scheme.',
  },
];

export const FORWARD_STREAMS: readonly { value: ForwardStream; label: string; help: string }[] = [
  { value: 'syslog', label: 'Device logs', help: 'What devices send to the syslog receiver.' },
  { value: 'trap', label: 'SNMP traps', help: 'What the trap receiver stores.' },
  { value: 'event', label: 'Signals', help: 'What the rules and the detections record.' },
  {
    value: 'audit',
    label: 'Audit trail',
    help: 'What people did here. Forwarding it sends operator activity off this machine.',
  },
];

/** What a forwarder sends when nothing is chosen: never the audit trail. */
export const DEFAULT_STREAMS: readonly ForwardStream[] = ['syslog', 'trap', 'event'];

export function isSyslogKind(kind: ForwardKind): boolean {
  return kind !== 'https_json';
}

/** The kinds that check a certificate, and so can be given an authority. */
export function isEncrypted(kind: ForwardKind): boolean {
  return kind === 'syslog_tls' || kind === 'https_json';
}

/** What the form holds while it is typed in. */
export interface ForwarderDraft {
  /** Empty for a forwarder that does not exist yet. */
  id: string;
  name: string;
  kind: ForwardKind;
  enabled: boolean;
  host: string;
  /** Text; empty means "the kind's default port". */
  port: string;
  format: 'rfc5424' | 'json';
  streams: ForwardStream[];
  /** `''` is "everything"; otherwise 0 … 7. */
  minSeverity: string;
  lab: boolean;
  caPem: string;
  /**
   * `undefined`: nothing typed, so what is stored stays. Anything else is
   * what will be sent as `url`.
   */
  url: string | undefined;
  /**
   * `undefined`: nothing typed. `''`: remove the stored token. Anything else
   * replaces it.
   */
  token: string | undefined;
}

export function draftOfForwarder(forwarder?: Forwarder): ForwarderDraft {
  return {
    id: forwarder?.id ?? '',
    name: forwarder?.name ?? '',
    kind: forwarder?.kind ?? 'syslog_tls',
    enabled: forwarder?.enabled ?? true,
    host: forwarder?.host ?? '',
    port: forwarder === undefined || forwarder.port === 0 ? '' : String(forwarder.port),
    format: forwarder?.format ?? 'rfc5424',
    streams: forwarder ? [...forwarder.streams] : [...DEFAULT_STREAMS],
    minSeverity: forwarder?.min_severity == null ? '' : String(forwarder.min_severity),
    lab: forwarder?.lab ?? false,
    caPem: forwarder?.ca_pem ?? '',
    url: undefined,
    token: undefined,
  };
}

export type ForwardField =
  | 'name'
  | 'kind'
  | 'host'
  | 'port'
  | 'format'
  | 'streams'
  | 'min_severity'
  | 'ca_pem'
  | 'url'
  | 'token'
  | 'form';

export type ForwardErrors = Partial<Record<ForwardField, string>>;

/**
 * What to hand to `logs.forward.save`. The only refusal made here is a port
 * that is not a number; everything else a forwarder can be wrong about is
 * the engine's to say.
 */
export function saveParams(draft: ForwarderDraft): { params: ForwarderSaveParams; errors: ForwardErrors } {
  const errors: ForwardErrors = {};
  const params: ForwarderSaveParams = {
    ...(draft.id === '' ? {} : { id: draft.id }),
    name: draft.name.trim(),
    kind: draft.kind,
    enabled: draft.enabled,
    streams: FORWARD_STREAMS.map((stream) => stream.value).filter((stream) => draft.streams.includes(stream)),
    min_severity: draft.minSeverity === '' ? null : Number(draft.minSeverity),
    lab: draft.lab,
  };
  if (isEncrypted(draft.kind) && draft.caPem.trim() !== '') params.ca_pem = draft.caPem.trim();

  if (isSyslogKind(draft.kind)) {
    params.host = draft.host.trim();
    params.format = draft.format;
    const typed = draft.port.trim();
    if (typed !== '') {
      if (!/^\d+$/.test(typed) || Number(typed) > 65_535 || Number(typed) === 0) {
        errors.port = 'A port is a whole number from 1 to 65535. Leave it empty for the kind’s own port.';
      } else {
        params.port = Number(typed);
      }
    }
  } else {
    // Sent only when typed: left out, what the vault holds stays as it is.
    if (draft.url !== undefined && draft.url.trim() !== '') params.url = draft.url.trim();
    if (draft.token !== undefined) params.token = draft.token.trim();
  }
  return { params, errors };
}

export type TokenFate =
  /** No destination is or will be stored, so there is nothing to say. */
  | 'none'
  /** Nothing typed: whatever token is stored stays, unseen. */
  | 'kept'
  | 'replaced'
  | 'removed'
  /** A new URL and no token: the stored one is not carried over. */
  | 'dropped-with-new-url';

/**
 * What will become of the stored token when this draft is saved.
 *
 * The one that matters is `dropped-with-new-url`: a token was given for one
 * destination, and the engine will not send it to another. Somebody who
 * changes only the URL would otherwise find, later, a forwarder that is
 * refused by its collector.
 */
export function tokenFate(draft: ForwarderDraft, existing: Forwarder | undefined): TokenFate {
  if (draft.kind !== 'https_json') return 'none';
  const newUrl = draft.url !== undefined && draft.url.trim() !== '';
  if (draft.token !== undefined) {
    if (draft.token.trim() === '') return existing?.has_secret || newUrl ? 'removed' : 'none';
    return 'replaced';
  }
  if (newUrl) return existing?.has_secret ? 'dropped-with-new-url' : 'none';
  return existing?.has_secret ? 'kept' : 'none';
}

/** Whether saving would leave an HTTPS forwarder with a destination. */
export function willHaveDestination(draft: ForwarderDraft, existing: Forwarder | undefined): boolean {
  if (draft.kind !== 'https_json') return true;
  if (draft.url !== undefined && draft.url.trim() !== '') return true;
  // The stored secret belongs to the HTTPS kind: a forwarder that was a
  // syslog one has none to keep.
  return existing?.kind === 'https_json' && existing.has_secret;
}

/**
 * Which field one of the engine's refusals is about
 * (`crates/nocturne-logs/src/forward/mod.rs`). The message is shown beside
 * that field, unchanged. No refusal repeats the host, the URL or the token
 * it was given, so showing it is safe.
 */
export function fieldOfForwardError(message: string): ForwardField {
  const text = message.trim();
  if (text.startsWith('give the forwarder a name')) return 'name';
  if (text.startsWith('say what kind of forwarder')) return 'kind';
  if (text.includes('there is no stream called') || text.startsWith('choose at least one stream')) {
    return 'streams';
  }
  if (text.startsWith('a severity is a number')) return 'min_severity';
  if (text.includes('`ca_pem`') || text.includes('certificate is not valid PEM') || text.startsWith('invalid input')) {
    return 'ca_pem';
  }
  if (text.startsWith('the token') || text.startsWith('a token needs the URL')) return 'token';
  if (
    text.startsWith('the collector’s URL') ||
    text.startsWith("the collector's URL") ||
    text.startsWith('an HTTPS forwarder needs the URL') ||
    text.startsWith('the URL has to be text')
  ) {
    return 'url';
  }
  if (text.startsWith('an HTTPS forwarder posts JSON')) return 'format';
  if (
    text.startsWith('a syslog forwarder needs the collector') ||
    text.startsWith('that is this application’s own syslog receiver') ||
    text.startsWith("that is this application's own syslog receiver")
  ) {
    return 'host';
  }
  return 'form';
}

// ------------------------------------------------------------------ the list

/** Where a forwarder sends, as far as that may be shown. Never a URL. */
export function destinationOf(forwarder: Forwarder): string {
  if (forwarder.kind === 'https_json') {
    return forwarder.has_secret ? 'A URL in the vault (never shown)' : 'No destination stored';
  }
  return `${forwarder.host}:${forwarder.port}`;
}

export function kindLabel(kind: string): string {
  return FORWARD_KINDS.find((entry) => entry.value === kind)?.label ?? kind;
}

export function streamLabel(stream: string): string {
  return FORWARD_STREAMS.find((entry) => entry.value === stream)?.label ?? stream;
}

/** `6` → `informational and worse`; none → `everything`. */
export function floorPhrase(minSeverity: number | null | undefined): string {
  if (minSeverity == null) return 'Everything';
  return `${SEVERITY_NAMES[minSeverity] ?? `severity ${minSeverity}`} and worse`;
}

export function sendsAudit(streams: readonly string[]): boolean {
  return streams.includes('audit');
}

const STATE: Record<ForwarderState, { tone: StatusTone; label: string; meaning: string }> = {
  disabled: { tone: 'idle', label: 'Switched off', meaning: 'It sends nothing until it is switched on.' },
  idle: { tone: 'info', label: 'Nothing sent yet', meaning: 'On, with nothing sent or tried since it was saved or started.' },
  ok: { tone: 'ok', label: 'Sending', meaning: 'The last send worked.' },
  failing: { tone: 'crit', label: 'Failing', meaning: 'The last send did not work.' },
};

export interface StateView {
  tone: StatusTone;
  label: string;
  meaning: string;
}

/** A forwarder's state, or "not reported" when the status does not have it. */
export function stateView(status: ForwarderStatus | undefined): StateView {
  if (!status) {
    return {
      tone: 'idle',
      label: 'Not reported',
      meaning: 'The status does not list this forwarder, so what it has done is not known.',
    };
  }
  return (
    STATE[status.state] ?? {
      tone: 'idle',
      label: status.state,
      meaning: 'A state this screen does not know.',
    }
  );
}

export interface ForwarderRow {
  forwarder: Forwarder;
  /** `undefined` when `logs.status` does not list it: unknown, not zero. */
  status: ForwarderStatus | undefined;
}

/** Each forwarder with what the status says it has done, joined by id. */
export function rowsOf(
  forwarders: readonly Forwarder[],
  statuses: readonly ForwarderStatus[] | undefined,
): ForwarderRow[] {
  const byId = new Map((statuses ?? []).map((status) => [status.id, status]));
  return forwarders.map((forwarder) => ({ forwarder, status: byId.get(forwarder.id) }));
}

export interface Waiting {
  /** Entries waiting to be sent, not counting the batch being sent. */
  entries: number;
  /** What they and the batch being sent take up, when the engine says. */
  bytes: number | null;
  /** What they may take up, when the engine says. */
  budget: number | null;
}

/**
 * What waits in a forwarder's queue, or `null` from a build whose status does
 * not report it — which is "not reported", not an empty queue.
 */
export function waitingOf(status: ForwarderStatus | undefined): Waiting | null {
  if (typeof status?.queued !== 'number') return null;
  return {
    entries: status.queued,
    bytes: typeof status.queued_bytes === 'number' ? status.queued_bytes : null,
    budget: typeof status.max_queued_bytes === 'number' ? status.max_queued_bytes : null,
  };
}

/**
 * What "sent" means for this kind, because it does not mean "arrived" for
 * any syslog transport.
 */
export function sentMeaning(kind: ForwardKind): string {
  return kind === 'https_json'
    ? 'Sent is what the collector answered 2xx to.'
    : 'Sent is what was handed to the network. No syslog transport acknowledges anything, so it does not mean “arrived”.';
}
