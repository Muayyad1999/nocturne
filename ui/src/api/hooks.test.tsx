// @vitest-environment happy-dom
import { QueryClientProvider, type QueryClient } from '@tanstack/react-query';
import { act, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useToasts } from '@/design/Toast';
import { RpcError } from './errors';
import { topicMatches, type AppEvent } from './events';
import { INVALIDATION_WINDOW_MS, useEvent, useHasMethod, useRpc, useRpcMutation } from './hooks';
import { createQueryClient } from './queryClient';

// The hooks talk to the client module; here it is replaced by a controllable
// one: `rpc` is a mock, and events are published by the test.
const backend = vi.hoisted(() => {
  const subscriptions = new Set<{ topic: string; handler: (event: AppEvent) => void }>();
  return { rpc: vi.fn<(method: string, params?: unknown) => Promise<unknown>>(), subscriptions };
});

vi.mock('./client', () => ({
  rpc: backend.rpc,
  subscribe: (topic: string, handler: (event: AppEvent) => void) => {
    const subscription = { topic, handler };
    backend.subscriptions.add(subscription);
    return () => backend.subscriptions.delete(subscription);
  },
}));

function publish(topic: string, payload: unknown = null): void {
  for (const { topic: pattern, handler } of [...backend.subscriptions]) {
    if (topicMatches(pattern, topic)) handler({ topic, payload, at: 1 });
  }
}

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let queryClient: QueryClient;
const mounted: (() => void)[] = [];

function mount(node: ReactNode) {
  const root = createRoot(document.createElement('div'));
  const render = (next: ReactNode) =>
    act(() => root.render(<QueryClientProvider client={queryClient}>{next}</QueryClientProvider>));
  const unmount = () => act(() => root.unmount());
  render(node);
  mounted.push(unmount);
  return { render, unmount };
}

const settle = (ms: number) => act(() => new Promise<void>((resolve) => setTimeout(resolve, ms)));
const toastTitles = () => useToasts.getState().toasts.map((toast) => [toast.tone, toast.title, toast.description]);

beforeEach(() => {
  queryClient = createQueryClient();
  backend.rpc.mockReset();
  backend.subscriptions.clear();
  useToasts.setState({ toasts: [], history: [], unread: 0 });
});

afterEach(() => {
  for (const unmount of mounted.splice(0)) unmount();
  queryClient.clear();
});

describe('useRpc', () => {
  function Devices({ site, onResult }: { site: string; onResult: (result: unknown) => void }) {
    const query = useRpc<string[]>('devices.list', { site }, { invalidateOn: ['devices.*'] });
    onResult({ data: query.data, error: query.error, pending: query.isPending });
    return null;
  }

  it('calls the method and caches the result under [method, params]', async () => {
    backend.rpc.mockResolvedValue(['core-sw-01']);
    const results: unknown[] = [];
    mount(<Devices site="dc1" onResult={(result) => results.push(result)} />);

    expect(results[0]).toMatchObject({ pending: true });
    await vi.waitFor(() => expect(results.at(-1)).toMatchObject({ data: ['core-sw-01'], pending: false }));
    expect(backend.rpc).toHaveBeenCalledWith('devices.list', { site: 'dc1' });
    expect(queryClient.getQueryData(['devices.list', { site: 'dc1' }])).toEqual(['core-sw-01']);
  });

  it('exposes a failure as an RpcError', async () => {
    backend.rpc.mockRejectedValue(new RpcError('unavailable', 'the engine is not reachable'));
    const results: { error?: unknown }[] = [];
    mount(<Devices site="dc1" onResult={(result) => results.push(result as { error?: unknown })} />);
    await vi.waitFor(() => expect(results.at(-1)?.error).toMatchObject({ code: 'unavailable' }));
  });

  it('refetches when a matching event arrives, once for a burst', async () => {
    backend.rpc.mockResolvedValue([]);
    mount(<Devices site="dc1" onResult={() => undefined} />);
    await vi.waitFor(() => expect(backend.rpc).toHaveBeenCalledTimes(1));

    publish('devices.changed');
    publish('devices.state');
    publish('devices.state');
    await settle(INVALIDATION_WINDOW_MS * 3);
    expect(backend.rpc).toHaveBeenCalledTimes(2);

    publish('devices.changed');
    await settle(INVALIDATION_WINDOW_MS * 3);
    expect(backend.rpc).toHaveBeenCalledTimes(3);
  });

  it('ignores events on other topics', async () => {
    backend.rpc.mockResolvedValue([]);
    mount(<Devices site="dc1" onResult={() => undefined} />);
    await vi.waitFor(() => expect(backend.rpc).toHaveBeenCalledTimes(1));

    publish('jobs.changed');
    publish('device');
    await settle(INVALIDATION_WINDOW_MS * 3);
    expect(backend.rpc).toHaveBeenCalledTimes(1);
  });

  it('only refetches the query whose params it was asked for', async () => {
    backend.rpc.mockResolvedValue([]);
    const view = mount(<Devices site="dc1" onResult={() => undefined} />);
    await vi.waitFor(() => expect(backend.rpc).toHaveBeenCalledTimes(1));
    view.render(<Devices site="dc2" onResult={() => undefined} />);
    await vi.waitFor(() => expect(backend.rpc).toHaveBeenCalledTimes(2));

    publish('devices.changed');
    await settle(INVALIDATION_WINDOW_MS * 3);
    expect(backend.rpc).toHaveBeenCalledTimes(3);
    expect(backend.rpc).toHaveBeenLastCalledWith('devices.list', { site: 'dc2' });
  });

  it('stops listening when the component unmounts', async () => {
    backend.rpc.mockResolvedValue([]);
    const view = mount(<Devices site="dc1" onResult={() => undefined} />);
    await vi.waitFor(() => expect(backend.rpc).toHaveBeenCalledTimes(1));
    view.unmount();
    expect(backend.subscriptions.size).toBe(0);
  });

  it('holds the call back while disabled', async () => {
    function Device({ id }: { id: string | null }) {
      useRpc('devices.get', { id }, { enabled: id !== null });
      return null;
    }
    backend.rpc.mockResolvedValue({});
    const view = mount(<Device id={null} />);
    await settle(20);
    expect(backend.rpc).not.toHaveBeenCalled();
    view.render(<Device id="7" />);
    await vi.waitFor(() => expect(backend.rpc).toHaveBeenCalledWith('devices.get', { id: '7' }));
  });
});

describe('useRpcMutation', () => {
  type Mutate = (params: { name: string }) => void;

  function Harness({ options, expose }: { options?: Parameters<typeof useRpcMutation>[1]; expose: (mutate: Mutate) => void }) {
    useRpc('devices.list');
    useRpc('devices.facets');
    useRpc('jobs.list');
    const mutation = useRpcMutation<{ id: string }, { name: string }>('devices.create', options);
    expose(mutation.mutate);
    return null;
  }

  const calls = (method: string) => backend.rpc.mock.calls.filter(([name]) => name === method).length;

  async function start(options?: Parameters<typeof useRpcMutation>[1]) {
    let mutate: Mutate = () => undefined;
    mount(<Harness options={options} expose={(next) => (mutate = next)} />);
    await vi.waitFor(() => expect(backend.rpc).toHaveBeenCalledTimes(3));
    return (params: { name: string }) => act(() => mutate(params));
  }

  it('calls the method with the params', async () => {
    backend.rpc.mockResolvedValue({ id: 'd1' });
    const mutate = await start();
    mutate({ name: 'core-sw-01' });
    await vi.waitFor(() => expect(backend.rpc).toHaveBeenCalledWith('devices.create', { name: 'core-sw-01' }));
  });

  it('refetches the queries whose method starts with an invalidated prefix, and no others', async () => {
    backend.rpc.mockResolvedValue({ id: 'd1' });
    const mutate = await start({ invalidates: ['devices.'] });
    mutate({ name: 'core-sw-01' });

    await vi.waitFor(() => expect(calls('devices.list')).toBe(2));
    await vi.waitFor(() => expect(calls('devices.facets')).toBe(2));
    await settle(20);
    expect(calls('jobs.list')).toBe(1);
  });

  it('shows the success toast', async () => {
    backend.rpc.mockResolvedValue({ id: 'd1' });
    const mutate = await start({ successToast: 'Device added' });
    mutate({ name: 'core-sw-01' });
    await vi.waitFor(() => expect(toastTitles()).toEqual([['success', 'Device added', undefined]]));
  });

  it('tells the user about a failure by default', async () => {
    backend.rpc.mockResolvedValue([]);
    const mutate = await start({ successToast: 'Device added' });
    backend.rpc.mockRejectedValue(new RpcError('conflict', 'A device named core-sw-01 already exists.'));
    mutate({ name: 'core-sw-01' });
    await vi.waitFor(() =>
      expect(toastTitles()).toEqual([['error', 'A device named core-sw-01 already exists.', undefined]]),
    );
  });

  it('puts a title in front of the failure when given one', async () => {
    backend.rpc.mockResolvedValue([]);
    const mutate = await start({ errorToast: 'The device was not added' });
    backend.rpc.mockRejectedValue(new RpcError('conflict', 'That name is taken.'));
    mutate({ name: 'core-sw-01' });
    await vi.waitFor(() =>
      expect(toastTitles()).toEqual([['error', 'The device was not added', 'That name is taken.']]),
    );
  });

  it('stays quiet when the screen shows the error itself', async () => {
    backend.rpc.mockResolvedValue([]);
    const mutate = await start({ errorToast: false });
    backend.rpc.mockRejectedValue(new RpcError('invalid', 'The name is required.'));
    mutate({ name: '' });
    await vi.waitFor(() => expect(calls('devices.create')).toBe(1));
    await settle(20);
    expect(toastTitles()).toEqual([]);
  });
});

describe('useEvent', () => {
  function Listener({ topic, onEvent }: { topic: string; onEvent: (payload: unknown, event: AppEvent) => void }) {
    useEvent(topic, onEvent);
    return null;
  }

  it('hands the payload and the event to the handler', () => {
    const handler = vi.fn();
    mount(<Listener topic="jobs.*" onEvent={handler} />);
    act(() => publish('jobs.changed', { id: 12 }));
    expect(handler).toHaveBeenCalledWith({ id: 12 }, { topic: 'jobs.changed', payload: { id: 12 }, at: 1 });
  });

  it('always calls the latest handler without resubscribing', () => {
    const first = vi.fn();
    const second = vi.fn();
    const view = mount(<Listener topic="jobs.changed" onEvent={first} />);
    const subscription = [...backend.subscriptions][0];
    view.render(<Listener topic="jobs.changed" onEvent={second} />);

    expect([...backend.subscriptions][0]).toBe(subscription);
    act(() => publish('jobs.changed'));
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledTimes(1);
  });

  it('follows a change of topic and unsubscribes on unmount', () => {
    const handler = vi.fn();
    const view = mount(<Listener topic="jobs.changed" onEvent={handler} />);
    view.render(<Listener topic="alerts.*" onEvent={handler} />);
    act(() => publish('jobs.changed'));
    act(() => publish('alerts.raised'));
    expect(handler).toHaveBeenCalledTimes(1);

    view.unmount();
    expect(backend.subscriptions.size).toBe(0);
  });
});

describe('useHasMethod', () => {
  function Gate({ method, onResult }: { method: string; onResult: (has: boolean | undefined) => void }) {
    onResult(useHasMethod(method));
    return null;
  }

  it('says it does not know yet, which is not the same as absent', async () => {
    backend.rpc.mockResolvedValue(['devices.list', 'config.backup']);
    const seen: (boolean | undefined)[] = [];
    mount(<Gate method="config.backup" onResult={(has) => seen.push(has)} />);

    // The first answer must be `undefined`. Were it `false`, a build that has
    // the feature would briefly claim not to, and the control that depends on
    // it would appear only after the round trip.
    expect(seen[0]).toBeUndefined();
    await vi.waitFor(() => expect(seen.at(-1)).toBe(true));
  });

  it('is false only once the backend has actually listed its methods', async () => {
    backend.rpc.mockResolvedValue(['devices.list']);
    const seen: (boolean | undefined)[] = [];
    mount(<Gate method="ipam.prefixes" onResult={(has) => seen.push(has)} />);

    expect(seen[0]).toBeUndefined();
    await vi.waitFor(() => expect(seen.at(-1)).toBe(false));
  });

  it('asks `app.methods` once however many components want it', async () => {
    backend.rpc.mockResolvedValue(['insights.list', 'insights.fix']);
    const noop = () => {};
    mount(
      <>
        <Gate method="insights.list" onResult={noop} />
        <Gate method="insights.fix" onResult={noop} />
        <Gate method="tools.list" onResult={noop} />
      </>,
    );
    await vi.waitFor(() => expect(backend.rpc).toHaveBeenCalled());
    await settle(20);
    expect(backend.rpc.mock.calls.filter(([name]) => name === 'app.methods')).toHaveLength(1);
  });
});
