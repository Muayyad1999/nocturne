import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from 'vitest';
import { useConnection } from './connection';
import type { AppEvent } from './events';
import { bindQueryClient, createQueryClient } from './queryClient';

const backend = vi.hoisted(() => ({
  handlers: new Map<string, (event: AppEvent) => void>(),
}));

vi.mock('./client', () => ({
  subscribe: (topic: string, handler: (event: AppEvent) => void) => {
    backend.handlers.set(topic, handler);
    return () => backend.handlers.delete(topic);
  },
}));

describe('createQueryClient', () => {
  it('does not depend on the machine being online and does not retry by itself', () => {
    const { queries, mutations } = createQueryClient().getDefaultOptions();
    expect(queries?.networkMode).toBe('always');
    expect(queries?.retry).toBe(false);
    expect(mutations?.networkMode).toBe('always');
    expect(mutations?.retry).toBe(false);
  });
});

describe('bindQueryClient', () => {
  const queryClient = createQueryClient();
  let invalidate: MockInstance<typeof queryClient.invalidateQueries>;
  let unbind: () => void;

  beforeEach(() => {
    invalidate = vi.spyOn(queryClient, 'invalidateQueries');
    useConnection.setState({ state: 'connecting', transport: 'http' });
    unbind = bindQueryClient(queryClient);
  });

  afterEach(() => unbind());

  it('refetches everything when the server says events were dropped', () => {
    backend.handlers.get('events.lagged')?.({ topic: 'events.lagged', payload: { missed: 12 }, at: 1 });
    expect(invalidate).toHaveBeenCalledTimes(1);
    expect(invalidate).toHaveBeenCalledWith();
  });

  it('does nothing on the first connection: there is nothing stale yet', () => {
    useConnection.setState({ state: 'online' });
    expect(invalidate).not.toHaveBeenCalled();
  });

  it('refetches everything when the stream comes back after a break', () => {
    useConnection.setState({ state: 'online' });
    useConnection.setState({ state: 'offline' });
    expect(invalidate).not.toHaveBeenCalled();
    useConnection.setState({ state: 'online' });
    expect(invalidate).toHaveBeenCalledTimes(1);

    // Changes that are not a reconnect do not refetch.
    useConnection.setState({ transport: 'http' });
    expect(invalidate).toHaveBeenCalledTimes(1);
  });

  it('stops reacting once unbound', () => {
    useConnection.setState({ state: 'online' });
    unbind();
    expect(backend.handlers.has('events.lagged')).toBe(false);
    useConnection.setState({ state: 'offline' });
    useConnection.setState({ state: 'online' });
    expect(invalidate).not.toHaveBeenCalled();
  });
});
