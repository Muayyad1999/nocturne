import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AppEvent } from '../events';
import type { ConnectionState } from '../transport';
import { RECONNECT_MAX_MS, RECONNECT_MIN_MS, createHttpTransport, reconnectDelay } from './http';

/** A stand-in for the browser's WebSocket that tests drive by hand. */
class FakeSocket {
  static instances: FakeSocket[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((message: { data: unknown }) => void) | null = null;
  onclose: (() => void) | null = null;
  closed = false;

  constructor(readonly url: URL | string) {
    FakeSocket.instances.push(this);
  }

  close(): void {
    this.closed = true;
  }

  open(): void {
    this.onopen?.();
  }

  receive(data: unknown): void {
    this.onmessage?.({ data });
  }

  drop(): void {
    this.onclose?.();
  }

  static get latest(): FakeSocket {
    const socket = FakeSocket.instances.at(-1);
    if (!socket) throw new Error('no socket was opened');
    return socket;
  }
}

function reply(body: unknown, status = 200): Response {
  return new Response(typeof body === 'string' ? body : JSON.stringify(body), { status });
}

const BASE = 'http://127.0.0.1:7878/';

describe('reconnectDelay', () => {
  it('backs off from 0.5 s to 10 s', () => {
    expect([0, 1, 2, 3, 4, 5, 6, 20].map(reconnectDelay)).toEqual([500, 1000, 2000, 4000, 8000, 10_000, 10_000, 10_000]);
    expect(RECONNECT_MIN_MS).toBe(500);
    expect(RECONNECT_MAX_MS).toBe(10_000);
  });
});

describe('http transport: calls', () => {
  const fetchMock = vi.fn<typeof fetch>();

  beforeEach(() => {
    fetchMock.mockReset();
    vi.stubGlobal('fetch', fetchMock);
  });

  it('posts the method and params with the bearer token', async () => {
    fetchMock.mockResolvedValue(reply({ ok: true, result: [{ id: 1 }] }));
    const transport = createHttpTransport({ baseUrl: BASE, token: 's3cret' });

    await expect(transport.request('devices.list', { site: 'dc1' })).resolves.toEqual([{ id: 1 }]);

    const [url, init] = fetchMock.mock.calls[0] ?? [];
    expect(String(url)).toBe('http://127.0.0.1:7878/api/rpc');
    expect(init?.method).toBe('POST');
    expect(init?.headers).toEqual({ 'Content-Type': 'application/json', Authorization: 'Bearer s3cret' });
    expect(JSON.parse(String(init?.body))).toEqual({ method: 'devices.list', params: { site: 'dc1' } });
  });

  it('omits params and the Authorization header when there are none', async () => {
    fetchMock.mockResolvedValue(reply({ ok: true, result: null }));
    await createHttpTransport({ baseUrl: BASE, token: null }).request('settings.all', undefined);

    const [, init] = fetchMock.mock.calls[0] ?? [];
    expect(init?.headers).toEqual({ 'Content-Type': 'application/json' });
    expect(JSON.parse(String(init?.body))).toEqual({ method: 'settings.all' });
  });

  it('resolves API paths against the address the app was served from', async () => {
    fetchMock.mockResolvedValue(reply({ ok: true, result: null }));
    await createHttpTransport({ baseUrl: 'https://noc.example.net/nocturne/', token: null }).request('x', undefined);
    expect(String(fetchMock.mock.calls[0]?.[0])).toBe('https://noc.example.net/nocturne/api/rpc');
  });

  it('turns an application error into an RpcError', async () => {
    fetchMock.mockResolvedValue(
      reply({ ok: false, error: { code: 'not_found', message: 'device 9 was not found', details: { id: 9 } } }),
    );
    await expect(createHttpTransport({ baseUrl: BASE, token: null }).request('devices.get', { id: 9 })).rejects.toMatchObject({
      name: 'RpcError',
      code: 'not_found',
      message: 'device 9 was not found',
      details: { id: 9 },
    });
  });

  it('prefers the error envelope of a transport-level failure over the bare status', async () => {
    fetchMock.mockResolvedValue(reply({ ok: false, error: { code: 'unauthorized', message: 'missing or wrong token' } }, 401));
    await expect(createHttpTransport({ baseUrl: BASE, token: 'bad' }).request('x', undefined)).rejects.toMatchObject({
      code: 'unauthorized',
      message: 'missing or wrong token',
    });
  });

  it('maps a status without an envelope', async () => {
    fetchMock.mockResolvedValue(reply('<html>Bad Gateway</html>', 502));
    await expect(createHttpTransport({ baseUrl: BASE, token: null }).request('x', undefined)).rejects.toMatchObject({
      code: 'unavailable',
    });
  });

  it('reports a reply it cannot read as internal', async () => {
    fetchMock.mockResolvedValue(reply('not json'));
    await expect(createHttpTransport({ baseUrl: BASE, token: null }).request('x', undefined)).rejects.toMatchObject({
      code: 'internal',
    });
  });

  it('reports a network failure as unavailable', async () => {
    fetchMock.mockRejectedValue(new TypeError('Failed to fetch'));
    await expect(createHttpTransport({ baseUrl: BASE, token: null }).request('x', undefined)).rejects.toMatchObject({
      name: 'RpcError',
      code: 'unavailable',
    });
  });
});

describe('http transport: events', () => {
  let states: ConnectionState[];
  let events: AppEvent[];

  const open = (baseUrl = BASE, token: string | null = 's3cret') =>
    createHttpTransport({ baseUrl, token }).open({
      onState: (state) => states.push(state),
      onEvent: (event) => events.push(event),
    });

  beforeEach(() => {
    vi.useFakeTimers();
    FakeSocket.instances = [];
    vi.stubGlobal('WebSocket', FakeSocket);
    states = [];
    events = [];
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('connects to /api/events with the token in the query', () => {
    open();
    expect(String(FakeSocket.latest.url)).toBe('ws://127.0.0.1:7878/api/events?token=s3cret');
    expect(states).toEqual(['connecting']);
  });

  it('uses wss when the app is served over https, and no token parameter without a token', () => {
    open('https://noc.example.net/nocturne/', null);
    expect(String(FakeSocket.latest.url)).toBe('wss://noc.example.net/nocturne/api/events');
  });

  it('goes online when the stream opens and delivers events', () => {
    open();
    FakeSocket.latest.open();
    FakeSocket.latest.receive(JSON.stringify({ topic: 'devices.changed', payload: { id: 4 }, at: 99 }));
    expect(states).toEqual(['connecting', 'online']);
    expect(events).toEqual([{ topic: 'devices.changed', payload: { id: 4 }, at: 99 }]);
  });

  it('ignores frames that are not events', () => {
    open();
    FakeSocket.latest.open();
    FakeSocket.latest.receive('not json');
    FakeSocket.latest.receive(JSON.stringify({ hello: 'world' }));
    FakeSocket.latest.receive(new ArrayBuffer(4));
    expect(events).toEqual([]);
  });

  it('reconnects with back-off after the stream drops', () => {
    open();
    FakeSocket.latest.open();
    FakeSocket.latest.drop();
    expect(states).toEqual(['connecting', 'online', 'offline']);
    expect(FakeSocket.instances).toHaveLength(1);

    // First retry after 0.5 s, and no sooner.
    vi.advanceTimersByTime(499);
    expect(FakeSocket.instances).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(FakeSocket.instances).toHaveLength(2);

    // Each failed attempt doubles the wait: 1 s, 2 s, 4 s, 8 s, then 10 s at most.
    for (const [wait, sockets] of [
      [1000, 3],
      [2000, 4],
      [4000, 5],
      [8000, 6],
      [10_000, 7],
      [10_000, 8],
    ] as const) {
      FakeSocket.latest.drop();
      vi.advanceTimersByTime(wait - 1);
      expect(FakeSocket.instances).toHaveLength(sockets - 1);
      vi.advanceTimersByTime(1);
      expect(FakeSocket.instances).toHaveLength(sockets);
    }
  });

  it('starts the back-off again after a successful reconnect, and keeps delivering', () => {
    open();
    FakeSocket.latest.open();
    FakeSocket.latest.drop();
    vi.advanceTimersByTime(500);
    FakeSocket.latest.drop();
    vi.advanceTimersByTime(1000);

    FakeSocket.latest.open();
    expect(states.at(-1)).toBe('online');
    FakeSocket.latest.receive(JSON.stringify({ topic: 'jobs.changed', payload: null, at: 1 }));
    expect(events).toHaveLength(1);

    FakeSocket.latest.drop();
    const before = FakeSocket.instances.length;
    vi.advanceTimersByTime(500);
    expect(FakeSocket.instances).toHaveLength(before + 1);
  });

  it('stops for good when closed', () => {
    const close = open();
    FakeSocket.latest.open();
    const socket = FakeSocket.latest;
    close();
    expect(socket.closed).toBe(true);

    socket.drop();
    vi.advanceTimersByTime(60_000);
    expect(FakeSocket.instances).toHaveLength(1);
    expect(states).toEqual(['connecting', 'online']);
  });

  it('does not reconnect if closed while waiting to retry', () => {
    const close = open();
    FakeSocket.latest.drop();
    close();
    vi.advanceTimersByTime(60_000);
    expect(FakeSocket.instances).toHaveLength(1);
  });

  it('treats an address WebSocket cannot use as offline, and keeps trying', () => {
    vi.stubGlobal(
      'WebSocket',
      class {
        constructor() {
          throw new SyntaxError('invalid URL');
        }
      },
    );
    open('file:///opt/nocturne/ui/index.html', null);
    expect(states).toEqual(['connecting', 'offline']);
    vi.advanceTimersByTime(500);
    expect(states).toEqual(['connecting', 'offline', 'offline']);
  });
});
