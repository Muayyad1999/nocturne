/**
 * What a transport is, and how one is chosen. The client (client.ts) owns the
 * subscription registry and the connection store; a transport only moves
 * requests and events.
 */

import type { AppEvent } from './events';

export type ConnectionState = 'connecting' | 'online' | 'offline';

export type TransportKind = 'tauri' | 'http' | 'mock';

export interface TransportHandlers {
  onEvent(event: AppEvent): void;
  onState(state: ConnectionState): void;
}

export interface Transport {
  readonly kind: TransportKind;
  /** Performs one call. Resolves with the result or rejects with an `RpcError`. */
  request(method: string, params: unknown): Promise<unknown>;
  /** Opens the event stream and reports its state. Returns a function that closes it. */
  open(handlers: TransportHandlers): () => void;
}

export interface Environment {
  /** Running inside the desktop shell (`window.__TAURI_INTERNALS__` exists). */
  tauri: boolean;
  /** A development or test build (`import.meta.env.DEV`). */
  dev: boolean;
  /** The page was opened with `?mock`. */
  mockRequested: boolean;
}

/**
 * Picks the transport. The mock exists in development and test builds only and
 * must be asked for; the desktop shell is detected; everything else is a
 * browser talking to the server over HTTP and WebSocket.
 */
export function chooseTransport({ tauri, dev, mockRequested }: Environment): TransportKind {
  if (dev && mockRequested) return 'mock';
  return tauri ? 'tauri' : 'http';
}
