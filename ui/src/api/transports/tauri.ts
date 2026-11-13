/**
 * Desktop shell: calls go through Tauri IPC (`invoke('rpc', …)`), events
 * arrive on the `nocturne://event` channel. The `rpc` command resolves with
 * the same `{ok, result | error}` envelope the server sends over HTTP.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toRpcError, unwrapEnvelope } from '../errors';
import { parseEvent } from '../events';
import type { Transport } from '../transport';

export const TAURI_EVENT = 'nocturne://event';

export function createTauriTransport(): Transport {
  return {
    kind: 'tauri',

    async request(method, params) {
      let reply: unknown;
      try {
        reply = await invoke('rpc', { method, params });
      } catch (error) {
        throw toRpcError(error);
      }
      return unwrapEnvelope(reply);
    },

    open({ onEvent, onState }) {
      let stop: (() => void) | undefined;
      let closed = false;

      onState('connecting');
      listen<unknown>(TAURI_EVENT, (message) => {
        const event = parseEvent(message.payload);
        if (event) onEvent(event);
      }).then(
        (unlisten) => {
          if (closed) {
            unlisten();
            return;
          }
          stop = unlisten;
          onState('online');
        },
        () => {
          if (!closed) onState('offline');
        },
      );

      return () => {
        closed = true;
        stop?.();
      };
    },
  };
}
