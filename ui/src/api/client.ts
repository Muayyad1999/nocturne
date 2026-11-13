/**
 * The API client: one RPC surface and one event stream, over whichever
 * transport fits where the UI is running.
 *
 *   desktop shell   Tauri IPC                     (transports/tauri.ts)
 *   browser         HTTP + WebSocket              (transports/http.ts)
 *   `?mock`         in-memory handlers, dev only  (dev/mock.ts)
 *
 * Screens normally use the hooks in hooks.ts rather than calling this directly.
 */

import { useConnection } from './connection';
import { toRpcError } from './errors';
import { EventHub, type AppEvent, type EventHandler } from './events';
import { captureToken } from './token';
import { chooseTransport, type ConnectionState, type Transport } from './transport';
import { createHttpTransport } from './transports/http';

export { RpcError, type RpcErrorCode } from './errors';
export type { AppEvent } from './events';
export type { ConnectionState } from './transport';
export { useConnection } from './connection';

export interface Client {
  rpc<T = unknown>(method: string, params?: unknown): Promise<T>;
  subscribe<P = unknown>(topic: string, handler: EventHandler<P>): () => void;
  /** Chooses the transport and opens the event stream. Safe to call repeatedly. */
  connect(): Promise<void>;
}

/**
 * Builds a client around a transport loader. The app uses the single instance
 * below; tests build their own around a fake transport.
 */
export function createClient(
  loadTransport: () => Promise<Transport>,
  connection: Pick<typeof useConnection, 'setState'> = useConnection,
): Client {
  const hub = new EventHub();
  let transport: Promise<Transport> | undefined;

  const ready = (): Promise<Transport> => {
    transport ??= loadTransport().then(
      (loaded) => {
        connection.setState({ transport: loaded.kind });
        loaded.open({
          onEvent: (event) => hub.dispatch(event),
          onState: (state) => connection.setState({ state }),
        });
        return loaded;
      },
      (error: unknown) => {
        // Let the next call try again instead of failing forever.
        transport = undefined;
        connection.setState({ state: 'offline' });
        throw error;
      },
    );
    return transport;
  };

  return {
    async rpc<T = unknown>(method: string, params?: unknown): Promise<T> {
      try {
        const active = await ready();
        return (await active.request(method, params)) as T;
      } catch (error) {
        throw toRpcError(error);
      }
    },

    subscribe(topic, handler) {
      const unsubscribe = hub.subscribe(topic, handler);
      // Subscribing is enough to start the stream; a failure surfaces on the first call.
      ready().catch(() => undefined);
      return unsubscribe;
    },

    async connect() {
      await ready();
    },
  };
}

async function loadTransport(): Promise<Transport> {
  const kind = chooseTransport({
    tauri: '__TAURI_INTERNALS__' in window,
    dev: import.meta.env.DEV,
    mockRequested: new URLSearchParams(window.location.search).has('mock'),
  });

  // The DEV guard lets the bundler drop the mock from production builds.
  if (import.meta.env.DEV && kind === 'mock') {
    const { createMockTransport } = await import('../dev/mock');
    return createMockTransport();
  }
  if (kind === 'tauri') {
    const { createTauriTransport } = await import('./transports/tauri');
    return createTauriTransport();
  }
  return createHttpTransport({ baseUrl: document.baseURI, token: captureToken() });
}

const client = createClient(loadTransport);

/** Calls a backend method. Rejects with an {@link RpcError}. */
export function rpc<T = unknown>(method: string, params?: unknown): Promise<T> {
  return client.rpc<T>(method, params);
}

/**
 * Listens for backend events. `topic` may end in `*` ("devices.*").
 * Returns an unsubscribe function.
 */
export function subscribe<P = unknown>(
  topic: string,
  handler: (event: AppEvent<P>) => void,
): () => void {
  return client.subscribe(topic, handler);
}

/** The state of the link to the engine. `useConnection` is the reactive form. */
export function connectionState(): ConnectionState {
  return useConnection.getState().state;
}

/** Opens the connection at start-up so the shell can show its state at once. */
export function connect(): Promise<void> {
  return client.connect();
}
