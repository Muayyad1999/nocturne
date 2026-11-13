// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest';
import { RpcError } from '@/api/errors';
import type { AppEvent } from '@/api/events';
import type { ConnectionState } from '@/api/transport';
import { MOCK_MARKER, createMockTransport, type MockModule } from './mock';

const modules = (handlers: MockModule) => ({ './mocks/test.ts': { default: handlers } });

describe('mock transport', () => {
  it('answers from the registered handlers', async () => {
    const transport = createMockTransport(modules({ 'math.add': (params) => (params as { a: number; b: number }).a + 2 }), 0);
    expect(transport.kind).toBe('mock');
    await expect(transport.request('math.add', { a: 40, b: 2 })).resolves.toBe(42);
  });

  it('rejects an unknown method the way the backend does, and says where to add it', async () => {
    const transport = createMockTransport(modules({}), 0);
    await expect(transport.request('devices.list', undefined)).rejects.toMatchObject({
      name: 'RpcError',
      code: 'unknown_method',
      message: expect.stringContaining('src/dev/mocks'),
    });
  });

  it('lets a handler fail with a backend error', async () => {
    const transport = createMockTransport(
      modules({
        'devices.get': () => {
          throw new RpcError('not_found', 'device 9 was not found');
        },
      }),
      0,
    );
    await expect(transport.request('devices.get', { id: 9 })).rejects.toMatchObject({ code: 'not_found' });
  });

  it('returns what would survive the wire: JSON, with undefined as null', async () => {
    const original = { at: new Date(0), skip: undefined, nested: { n: 1 } };
    const transport = createMockTransport(modules({ 'x.get': () => original, 'x.none': () => undefined }), 0);
    const result = await transport.request('x.get', undefined);
    expect(result).toEqual({ at: '1970-01-01T00:00:00.000Z', nested: { n: 1 } });
    expect(result).not.toBe(original);
    await expect(transport.request('x.none', undefined)).resolves.toBeNull();
  });

  it('is online at once and delivers events published by handlers', async () => {
    const transport = createMockTransport(
      modules({
        'devices.create': (_params, { emit }) => {
          emit('devices.changed', { change: 'created' });
          return { id: 'd1' };
        },
      }),
      0,
    );
    const states: ConnectionState[] = [];
    const events: AppEvent[] = [];
    transport.open({ onState: (state) => states.push(state), onEvent: (event) => events.push(event) });
    expect(states).toEqual(['online']);

    await transport.request('devices.create', {});
    expect(events).toHaveLength(1);
    expect(events[0]).toMatchObject({ topic: 'devices.changed', payload: { change: 'created' } });
    expect(typeof events[0]?.at).toBe('number');
  });

  it('exposes controls for tests and the console', async () => {
    const transport = createMockTransport(modules({}), 0);
    const states: ConnectionState[] = [];
    const events: AppEvent[] = [];
    transport.open({ onState: (state) => states.push(state), onEvent: (event) => events.push(event) });

    const controls = window.__NOCTURNE_MOCK__;
    expect(controls?.marker).toBe(MOCK_MARKER);
    controls?.setConnection('offline');
    controls?.emit('alerts.raised', { id: 1 });
    controls?.handle('late.method', () => 'added later');

    expect(states).toEqual(['online', 'offline']);
    expect(events[0]).toMatchObject({ topic: 'alerts.raised' });
    await expect(transport.request('late.method', undefined)).resolves.toBe('added later');
  });

  it('waits for the configured latency', async () => {
    vi.useFakeTimers();
    try {
      const transport = createMockTransport(modules({ ping: () => 'pong' }), 150);
      const settled = vi.fn();
      void transport.request('ping', undefined).then(settled);
      await vi.advanceTimersByTimeAsync(149);
      expect(settled).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1);
      expect(settled).toHaveBeenCalledWith('pong');
    } finally {
      vi.useRealTimers();
    }
  });

  it('ships the showcase handlers by default', async () => {
    const transport = createMockTransport(undefined, 0);
    await expect(transport.request('dev.echo', { hello: 'world' })).resolves.toEqual({ hello: 'world' });
  });
});
