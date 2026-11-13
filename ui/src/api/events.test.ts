import { describe, expect, it, vi } from 'vitest';
import { EventHub, parseEvent, topicMatches, type AppEvent } from './events';

const event = (topic: string, payload: unknown = null): AppEvent => ({ topic, payload, at: 1 });

describe('topicMatches', () => {
  it('matches an exact topic', () => {
    expect(topicMatches('devices.changed', 'devices.changed')).toBe(true);
    expect(topicMatches('devices.changed', 'devices.state')).toBe(false);
    expect(topicMatches('devices', 'devices.changed')).toBe(false);
  });

  it('matches a prefix when the pattern ends in *', () => {
    expect(topicMatches('devices.*', 'devices.changed')).toBe(true);
    expect(topicMatches('devices.*', 'devices.state.up')).toBe(true);
    expect(topicMatches('devices.*', 'devices')).toBe(false);
    expect(topicMatches('devices.*', 'devicesx.changed')).toBe(false);
    expect(topicMatches('dev*', 'devices.changed')).toBe(true);
  });

  it('matches everything with a bare *', () => {
    expect(topicMatches('*', 'audit.appended')).toBe(true);
    expect(topicMatches('*', '')).toBe(true);
  });
});

describe('parseEvent', () => {
  it('accepts a well-formed frame', () => {
    expect(parseEvent({ topic: 'jobs.changed', payload: { id: 3 }, at: 1700000000000 })).toEqual({
      topic: 'jobs.changed',
      payload: { id: 3 },
      at: 1700000000000,
    });
  });

  it('rejects anything else', () => {
    for (const frame of [null, 'x', 1, {}, { topic: 1, at: 1 }, { topic: 'a' }, { topic: 'a', at: '1' }]) {
      expect(parseEvent(frame)).toBeNull();
    }
  });
});

describe('EventHub', () => {
  it('delivers to exact and wildcard subscribers, and to no one else', () => {
    const hub = new EventHub();
    const exact = vi.fn();
    const wildcard = vi.fn();
    const everything = vi.fn();
    const other = vi.fn();
    hub.subscribe('devices.changed', exact);
    hub.subscribe('devices.*', wildcard);
    hub.subscribe('*', everything);
    hub.subscribe('jobs.*', other);

    const changed = event('devices.changed', { id: 1 });
    hub.dispatch(changed);
    hub.dispatch(event('devices.state'));

    expect(exact).toHaveBeenCalledTimes(1);
    expect(exact).toHaveBeenCalledWith(changed);
    expect(wildcard).toHaveBeenCalledTimes(2);
    expect(everything).toHaveBeenCalledTimes(2);
    expect(other).not.toHaveBeenCalled();
  });

  it('stops delivering after unsubscribe, and unsubscribing twice is harmless', () => {
    const hub = new EventHub();
    const handler = vi.fn();
    const unsubscribe = hub.subscribe('devices.*', handler);
    hub.dispatch(event('devices.changed'));
    unsubscribe();
    unsubscribe();
    hub.dispatch(event('devices.changed'));
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it('treats each subscription separately, even for the same function', () => {
    const hub = new EventHub();
    const handler = vi.fn();
    const first = hub.subscribe('a', handler);
    hub.subscribe('a', handler);
    hub.dispatch(event('a'));
    expect(handler).toHaveBeenCalledTimes(2);
    first();
    hub.dispatch(event('a'));
    expect(handler).toHaveBeenCalledTimes(3);
  });

  it('lets a handler unsubscribe another one mid-dispatch', () => {
    const hub = new EventHub();
    const second = vi.fn();
    let unsubscribeSecond = () => {};
    hub.subscribe('a', () => unsubscribeSecond());
    unsubscribeSecond = hub.subscribe('a', second);
    hub.dispatch(event('a'));
    expect(second).not.toHaveBeenCalled();
  });

  it('keeps delivering when a handler throws, and reports the failure', () => {
    const reported = vi.fn();
    vi.stubGlobal('reportError', reported);
    const hub = new EventHub();
    const failure = new Error('handler broke');
    const after = vi.fn();
    hub.subscribe('a', () => {
      throw failure;
    });
    hub.subscribe('a', after);

    hub.dispatch(event('a'));

    expect(after).toHaveBeenCalledTimes(1);
    expect(reported).toHaveBeenCalledWith(failure);
  });
});
