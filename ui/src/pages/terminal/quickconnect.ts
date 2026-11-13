/**
 * The quick-connect field: `user@host[:port]`, and the serial and local-shell
 * targets that sit beside it. Parsing is here so the field can say what is
 * wrong before anything is dialled.
 */

import type { Adhoc, AdhocTransport, TransportKind } from '@/api/types/terminal';

export interface QuickConnectTarget {
  transport: AdhocTransport;
  username: string;
  host: string;
  /** Left out when the transport's default is wanted. */
  port?: number;
}

export type QuickConnectParse =
  | { ok: true; target: QuickConnectTarget }
  | { ok: false; problem: string };

const PORT_RANGE = { min: 1, max: 65_535 };

/**
 * `admin@10.0.0.1`, `admin@sw1.example.net:2222`, `[2001:db8::1]:22`, or a
 * bare host. SSH needs a user name; Telnet does not, because console servers
 * rarely have one.
 */
export function parseQuickConnect(text: string, transport: AdhocTransport = 'ssh'): QuickConnectParse {
  const value = text.trim();
  if (value === '') return { ok: false, problem: 'Type a host to connect to.' };

  const at = value.lastIndexOf('@');
  const username = at === -1 ? '' : value.slice(0, at);
  let rest = at === -1 ? value : value.slice(at + 1);

  if (at !== -1 && username === '') return { ok: false, problem: 'Put a user name before the @.' };
  if (rest === '') return { ok: false, problem: 'Put a host after the @.' };

  let host: string;
  let port: number | undefined;

  if (rest.startsWith('[')) {
    // A bracketed IPv6 literal, so its colons are not read as a port.
    const close = rest.indexOf(']');
    if (close === -1) return { ok: false, problem: 'Close the [ ] around the IPv6 address.' };
    host = rest.slice(1, close);
    rest = rest.slice(close + 1);
    if (rest !== '' && !rest.startsWith(':')) return { ok: false, problem: `“${rest}” is not a port.` };
    rest = rest.slice(1);
    if (rest !== '') {
      const parsed = parsePort(rest);
      if (parsed === null) return { ok: false, problem: `“${rest}” is not a port number.` };
      port = parsed;
    }
  } else {
    const colon = rest.indexOf(':');
    if (colon === -1) {
      host = rest;
    } else {
      host = rest.slice(0, colon);
      const tail = rest.slice(colon + 1);
      if (tail.includes(':')) {
        return { ok: false, problem: 'Put [ ] around an IPv6 address so its colons are not a port.' };
      }
      const parsed = parsePort(tail);
      if (parsed === null) return { ok: false, problem: `“${tail}” is not a port number.` };
      port = parsed;
    }
  }

  if (host === '') return { ok: false, problem: 'Type a host to connect to.' };
  if (/\s/.test(host)) return { ok: false, problem: 'A host name has no spaces in it.' };
  if (transport === 'ssh' && username === '') {
    return { ok: false, problem: 'SSH needs a user name: user@host.' };
  }

  return { ok: true, target: { transport, username, host, ...(port === undefined ? {} : { port }) } };
}

function parsePort(text: string): number | null {
  if (!/^\d+$/.test(text)) return null;
  const port = Number(text);
  if (port < PORT_RANGE.min || port > PORT_RANGE.max) return null;
  return port;
}

/** What `terminal.open` is given for a quick-connect target. */
export function adhocFor(target: QuickConnectTarget): Adhoc {
  return {
    transport: target.transport,
    host: target.host,
    ...(target.username === '' ? {} : { username: target.username }),
    ...(target.port === undefined ? {} : { port: target.port }),
  };
}

/** How a target reads in a tab and in the recent list. */
export function targetLabel(target: QuickConnectTarget): string {
  const host = target.host.includes(':') ? `[${target.host}]` : target.host;
  const authority = target.username === '' ? host : `${target.username}@${host}`;
  return target.port === undefined ? authority : `${authority}:${target.port}`;
}

/**
 * The target of a session that is not to an inventory device, rebuilt from
 * what the backend reports, so a closed tab can be dialled again.
 *
 * A password typed into the quick-connect field is used once and never
 * stored, here or in the backend: a session that needed one comes back as
 * `null` and the user is asked for it again.
 */
export function adhocFromSession(session: {
  kind: TransportKind;
  title: string;
  info: { peer: string };
}): Adhoc | null {
  if (session.kind === 'local') return { transport: 'local' };
  if (session.kind === 'serial') return { transport: 'serial', host: session.info.peer };
  if (session.kind !== 'ssh' && session.kind !== 'telnet') return null;

  // `host:port`, or `[v6]:port`.
  const match = /^(.*?)(?::(\d+))?$/.exec(session.info.peer);
  const host = match?.[1]?.replace(/^\[|\]$/g, '') ?? '';
  if (host === '') return null;
  const port = match?.[2] ? Number(match[2]) : undefined;
  const username = session.title.includes('@') ? session.title.split('@')[0] : undefined;
  if (session.kind === 'ssh' && !username) return null;

  return {
    transport: session.kind,
    host,
    ...(username ? { username } : {}),
    ...(port === undefined ? {} : { port }),
  };
}
