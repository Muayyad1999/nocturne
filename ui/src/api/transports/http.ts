/**
 * Browser / server mode: `POST /api/rpc` for calls and a WebSocket on
 * `/api/events` for events, reconnecting with back-off when the stream drops.
 *
 * The stream is opened without a topic filter and events are routed in the
 * client, so subscribing and unsubscribing never has to reconnect the socket.
 */

import { errorFromStatus, isEnvelope, unreachableError, unwrapEnvelope } from '../errors';
import { parseEvent } from '../events';
import type { Transport } from '../transport';

export const RECONNECT_MIN_MS = 500;
export const RECONNECT_MAX_MS = 10_000;

/** Delay before reconnect attempt `attempt` (0-based): 0.5 s, doubling, capped at 10 s. */
export function reconnectDelay(attempt: number): number {
  return Math.min(RECONNECT_MIN_MS * 2 ** attempt, RECONNECT_MAX_MS);
}

export interface HttpTransportOptions {
  /** The address the app was loaded from; API paths resolve against it. */
  baseUrl: string;
  /** Access token from the `#token=…` link, if any. */
  token: string | null;
}

export function createHttpTransport({ baseUrl, token }: HttpTransportOptions): Transport {
  const rpcUrl = new URL('api/rpc', baseUrl);
  const eventsUrl = new URL('api/events', baseUrl);
  eventsUrl.protocol = eventsUrl.protocol === 'https:' ? 'wss:' : 'ws:';
  // Browsers cannot set headers on a WebSocket, so the token travels in the query.
  if (token) eventsUrl.searchParams.set('token', token);

  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  if (token) headers.Authorization = `Bearer ${token}`;

  return {
    kind: 'http',

    async request(method, params) {
      let response: Response;
      try {
        response = await fetch(rpcUrl, {
          method: 'POST',
          headers,
          body: JSON.stringify({ method, params }),
        });
      } catch {
        throw unreachableError();
      }

      const reply: unknown = await response.json().catch(() => undefined);
      // Transport-level failures carry the same envelope as application errors.
      if (isEnvelope(reply)) return unwrapEnvelope(reply);
      if (!response.ok) throw errorFromStatus(response.status);
      return unwrapEnvelope(reply);
    },

    open({ onEvent, onState }) {
      let socket: WebSocket | undefined;
      let retryTimer: ReturnType<typeof setTimeout> | undefined;
      let attempt = 0;
      let closed = false;

      const retry = () => {
        if (closed) return;
        onState('offline');
        retryTimer = setTimeout(connect, reconnectDelay(attempt));
        attempt += 1;
      };

      const connect = () => {
        try {
          socket = new WebSocket(eventsUrl);
        } catch {
          // An address WebSocket cannot use (the page was opened from disk).
          retry();
          return;
        }
        socket.onopen = () => {
          attempt = 0;
          onState('online');
        };
        socket.onmessage = (message: MessageEvent<unknown>) => {
          if (typeof message.data !== 'string') return;
          let frame: unknown;
          try {
            frame = JSON.parse(message.data);
          } catch {
            return;
          }
          const event = parseEvent(frame);
          if (event) onEvent(event);
        };
        // A failed connection fires `error` and then `close`; `close` alone drives the retry.
        socket.onclose = retry;
      };

      onState('connecting');
      connect();

      return () => {
        closed = true;
        clearTimeout(retryTimer);
        if (socket) {
          socket.onclose = null;
          socket.close();
        }
      };
    },
  };
}
