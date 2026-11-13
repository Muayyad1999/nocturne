/**
 * The mock transport: an in-memory backend for developing and screenshot-
 * testing the UI without the engine. Development and test builds only; open
 * the app with `?mock`.
 *
 * This module is reached through a dynamic import behind `import.meta.env.DEV`
 * and never ships: the Playwright suite fails if MOCK_MARKER appears in the
 * production bundle.
 *
 * Handlers live in `./mocks/<module>.ts`, one file per backend module, each
 * default-exporting a map of method name to handler. A handler receives the
 * params and may publish events; it returns the result or throws an RpcError.
 */

import { RpcError } from '@/api/errors';
import type { AppEvent } from '@/api/events';
import type { ConnectionState, Transport, TransportHandlers } from '@/api/transport';

/** A string that exists only here, for the production-bundle check. */
export const MOCK_MARKER = 'nocturne-mock-transport';

export interface MockContext {
  /** Publishes an event to the UI, as the backend would. */
  emit(topic: string, payload: unknown): void;
}

export type MockHandler = (params: unknown, context: MockContext) => unknown;

export type MockModule = Record<string, MockHandler>;

/** Controls exposed on `window.__NOCTURNE_MOCK__` for Playwright and the console. */
export interface MockControls {
  marker: string;
  emit(topic: string, payload: unknown): void;
  setConnection(state: ConnectionState): void;
  /** Adds or replaces a handler at run time. */
  handle(method: string, handler: MockHandler): void;
}

declare global {
  interface Window {
    __NOCTURNE_MOCK__?: MockControls;
  }
}

const DEFAULT_LATENCY_MS = 150;

function latencyFromUrl(): number {
  const value = new URLSearchParams(window.location.search).get('latency');
  const latency = value === null ? Number.NaN : Number(value);
  return Number.isFinite(latency) && latency >= 0 ? latency : DEFAULT_LATENCY_MS;
}

export function createMockTransport(
  modules: Record<string, { default: MockModule }> = import.meta.glob<{ default: MockModule }>(
    './mocks/*.ts',
    { eager: true },
  ),
  latency: number = latencyFromUrl(),
): Transport {
  const handlers = new Map<string, MockHandler>();
  for (const module of Object.values(modules)) {
    for (const [method, handler] of Object.entries(module.default)) handlers.set(method, handler);
  }

  let listeners: TransportHandlers | undefined;
  const emit = (topic: string, payload: unknown) => {
    const event: AppEvent = { topic, payload, at: Date.now() };
    listeners?.onEvent(event);
  };

  window.__NOCTURNE_MOCK__ = {
    marker: MOCK_MARKER,
    emit,
    setConnection: (state) => listeners?.onState(state),
    handle: (method, handler) => handlers.set(method, handler),
  };

  return {
    kind: 'mock',

    async request(method, params) {
      // A little latency, so loading states are seen while developing.
      if (latency > 0) await new Promise((resolve) => setTimeout(resolve, latency));
      const handler = handlers.get(method);
      if (!handler) {
        throw new RpcError(
          'unknown_method',
          `there is no method named \`${method}\` in the mock. Add a handler in src/dev/mocks/.`,
        );
      }
      // Round-trip through JSON like a real transport would, so a handler
      // cannot hand the UI something that could never cross the wire.
      const result: unknown = await handler(params, { emit });
      return result === undefined ? null : (JSON.parse(JSON.stringify(result)) as unknown);
    },

    open(next) {
      listeners = next;
      next.onState('online');
      return () => {
        listeners = undefined;
      };
    },
  };
}
