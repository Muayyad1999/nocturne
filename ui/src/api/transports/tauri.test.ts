// @vitest-environment happy-dom
import { emit } from '@tauri-apps/api/event';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AppEvent } from '../events';
import type { ConnectionState } from '../transport';
import { TAURI_EVENT, createTauriTransport } from './tauri';

afterEach(() => {
  clearMocks();
});

describe('tauri transport: calls', () => {
  it('invokes the rpc command and unwraps the result', async () => {
    const calls: unknown[] = [];
    mockIPC((command, payload) => {
      calls.push([command, payload]);
      return { ok: true, result: { id: 'r1' } };
    });

    await expect(createTauriTransport().request('devices.create', { name: 'core-sw-01' })).resolves.toEqual({ id: 'r1' });
    expect(calls).toEqual([['rpc', { method: 'devices.create', params: { name: 'core-sw-01' } }]]);
  });

  it('turns an application error into an RpcError', async () => {
    mockIPC(() => ({ ok: false, error: { code: 'conflict', message: 'that name is taken' } }));
    await expect(createTauriTransport().request('devices.create', {})).rejects.toMatchObject({
      name: 'RpcError',
      code: 'conflict',
      message: 'that name is taken',
    });
  });

  it('understands a rejection that carries an error payload', async () => {
    mockIPC(() => {
      // eslint-disable-next-line @typescript-eslint/only-throw-error
      throw { code: 'forbidden', message: 'read-only mode' };
    });
    await expect(createTauriTransport().request('x', undefined)).rejects.toMatchObject({
      code: 'forbidden',
      message: 'read-only mode',
    });
  });

  it('reports any other IPC failure as internal', async () => {
    mockIPC(() => {
      throw new Error('command rpc not found');
    });
    await expect(createTauriTransport().request('x', undefined)).rejects.toMatchObject({
      code: 'internal',
      message: 'command rpc not found',
    });
  });
});

describe('tauri transport: events', () => {
  it('listens on the event channel and goes online', async () => {
    mockIPC(() => undefined, { shouldMockEvents: true });
    const states: ConnectionState[] = [];
    const events: AppEvent[] = [];

    const close = createTauriTransport().open({
      onState: (state) => states.push(state),
      onEvent: (event) => events.push(event),
    });
    expect(states).toEqual(['connecting']);
    await vi.waitFor(() => expect(states).toEqual(['connecting', 'online']));

    await emit(TAURI_EVENT, { topic: 'devices.state', payload: { id: 2, up: false }, at: 5 });
    await emit(TAURI_EVENT, 'not an event');
    expect(events).toEqual([{ topic: 'devices.state', payload: { id: 2, up: false }, at: 5 }]);

    close();
    await emit(TAURI_EVENT, { topic: 'devices.state', payload: null, at: 6 });
    expect(events).toHaveLength(1);
  });
});
