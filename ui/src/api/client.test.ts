import { describe, expect, it, vi } from 'vitest';
import { createClient } from './client';
import type { ConnectionState, Transport, TransportHandlers, TransportKind } from './transport';

/** A transport whose replies and events the test controls. */
function fakeTransport(kind: TransportKind = 'http') {
  let handlers: TransportHandlers | undefined;
  const request = vi.fn<Transport['request']>();
  const closed = vi.fn();
  const transport: Transport = {
    kind,
    request,
    open(next) {
      handlers = next;
      next.onState('connecting');
      return closed;
    },
  };
  return {
    transport,
    request,
    emit: (topic: string, payload: unknown = null) => handlers?.onEvent({ topic, payload, at: Date.now() }),
    setState: (state: ConnectionState) => handlers?.onState(state),
  };
}

function connectionSpy() {
  const state: { state?: ConnectionState; transport?: TransportKind | null } = {};
  return { state, store: { setState: (partial: object) => void Object.assign(state, partial) } };
}

describe('client', () => {
  it('loads the transport once, on first use, and reports which one it is', async () => {
    const fake = fakeTransport('tauri');
    const load = vi.fn(async () => fake.transport);
    const connection = connectionSpy();
    const client = createClient(load, connection.store);
    expect(load).not.toHaveBeenCalled();

    fake.request.mockResolvedValue('pong');
    await expect(client.rpc('ping')).resolves.toBe('pong');
    await client.rpc('ping');
    await client.connect();

    expect(load).toHaveBeenCalledTimes(1);
    expect(connection.state).toEqual({ transport: 'tauri', state: 'connecting' });
    fake.setState('online');
    expect(connection.state.state).toBe('online');
  });

  it('passes method and params to the transport', async () => {
    const fake = fakeTransport();
    fake.request.mockResolvedValue(null);
    const client = createClient(async () => fake.transport, connectionSpy().store);
    await client.rpc('devices.get', { id: 3 });
    await client.rpc('settings.all');
    expect(fake.request.mock.calls).toEqual([
      ['devices.get', { id: 3 }],
      ['settings.all', undefined],
    ]);
  });

  it('always rejects with an RpcError, whatever the transport threw', async () => {
    const fake = fakeTransport();
    const client = createClient(async () => fake.transport, connectionSpy().store);

    fake.request.mockRejectedValueOnce(new TypeError('boom'));
    await expect(client.rpc('x')).rejects.toMatchObject({ name: 'RpcError', code: 'internal', message: 'boom' });

    fake.request.mockRejectedValueOnce({ code: 'timeout', message: 'no answer in 10 s' });
    await expect(client.rpc('x')).rejects.toMatchObject({ name: 'RpcError', code: 'timeout' });
  });

  it('routes events to exact and wildcard subscribers', async () => {
    const fake = fakeTransport();
    const client = createClient(async () => fake.transport, connectionSpy().store);
    const exact = vi.fn();
    const wildcard = vi.fn();
    const all = vi.fn();
    client.subscribe('devices.changed', exact);
    const stopWildcard = client.subscribe('devices.*', wildcard);
    client.subscribe('*', all);
    await client.connect();

    fake.emit('devices.changed', { id: 1 });
    fake.emit('devices.state', { id: 1 });
    fake.emit('jobs.changed');

    expect(exact).toHaveBeenCalledTimes(1);
    expect(exact.mock.calls[0]?.[0]).toMatchObject({ topic: 'devices.changed', payload: { id: 1 } });
    expect(wildcard).toHaveBeenCalledTimes(2);
    expect(all).toHaveBeenCalledTimes(3);

    stopWildcard();
    fake.emit('devices.changed');
    expect(wildcard).toHaveBeenCalledTimes(2);
    expect(exact).toHaveBeenCalledTimes(2);
  });

  it('opens the event stream as soon as something subscribes', async () => {
    const fake = fakeTransport();
    const load = vi.fn(async () => fake.transport);
    const client = createClient(load, connectionSpy().store);
    const handler = vi.fn();

    client.subscribe('audit.*', handler);
    await vi.waitFor(() => expect(load).toHaveBeenCalledTimes(1));
    await client.connect();
    fake.emit('audit.appended');
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it('goes offline if the transport cannot be loaded, and tries again on the next call', async () => {
    const fake = fakeTransport();
    const load = vi
      .fn<() => Promise<Transport>>()
      .mockRejectedValueOnce(new Error('chunk failed to load'))
      .mockResolvedValue(fake.transport);
    const connection = connectionSpy();
    const client = createClient(load, connection.store);

    await expect(client.rpc('ping')).rejects.toMatchObject({ name: 'RpcError', message: 'chunk failed to load' });
    expect(connection.state.state).toBe('offline');

    fake.request.mockResolvedValue('pong');
    await expect(client.rpc('ping')).resolves.toBe('pong');
    expect(load).toHaveBeenCalledTimes(2);
  });
});
