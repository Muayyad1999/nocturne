/**
 * Desktop shell: calls go through Tauri IPC (`invoke('rpc', …)`), events
 * arrive on the `nocturne://event` channel.
 *
 * There is **no envelope here**, unlike HTTP. The command is
 * `Result<Value, AppError>` (`apps/desktop/src-tauri/src/rpc.rs`), and Tauri
 * resolves the promise with the `Ok` value and rejects it with the
 * serialised `Err` — so success is the result itself and failure arrives in
 * the `catch`. HTTP needs an envelope because one reply shape has to carry
 * both; IPC has two channels already.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toRpcError } from '../errors';
import { parseEvent } from '../events';
import type { Transport } from '../transport';

export const TAURI_EVENT = 'nocturne://event';

export function createTauriTransport(): Transport {
  return {
    kind: 'tauri',

    async request(method, params) {
      try {
        return await invoke('rpc', { method, params });
      } catch (error) {
        throw toRpcError(error);
      }
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
