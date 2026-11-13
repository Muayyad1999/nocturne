import { describe, expect, it, vi } from 'vitest';
import { captureToken, type TokenEnvironment } from './token';

function environment(hash: string, stored: string | null = null) {
  const items = new Map<string, string>(stored === null ? [] : [['nocturne.token', stored]]);
  const replaceState = vi.fn();
  const env: TokenEnvironment = {
    location: { hash, pathname: '/', search: '' },
    history: { replaceState },
    storage: {
      getItem: (key) => items.get(key) ?? null,
      setItem: (key, value) => void items.set(key, value),
    },
  };
  return { env, items, replaceState };
}

describe('captureToken', () => {
  it('takes the token from the fragment, stores it and cleans the address', () => {
    const { env, items, replaceState } = environment('#token=abc123');
    expect(captureToken(env)).toBe('abc123');
    expect(items.get('nocturne.token')).toBe('abc123');
    // The fragment is gone, so the hash router starts at "/" and the token is not left in the address bar.
    expect(replaceState).toHaveBeenCalledWith(null, '', '/');
  });

  it('keeps the query string when cleaning the address', () => {
    const { env, replaceState } = environment('#token=abc');
    env.location = { hash: '#token=abc', pathname: '/nocturne/', search: '?mock' };
    captureToken(env);
    expect(replaceState).toHaveBeenCalledWith(null, '', '/nocturne/?mock');
  });

  it('decodes an encoded token', () => {
    expect(captureToken(environment('#token=a%2Bb%3D').env)).toBe('a+b=');
  });

  it('prefers a fresh token over a stored one', () => {
    const { env, items } = environment('#token=new', 'old');
    expect(captureToken(env)).toBe('new');
    expect(items.get('nocturne.token')).toBe('new');
  });

  it('falls back to the stored token and leaves routes alone', () => {
    const { env, replaceState } = environment('#/devices/7', 'stored');
    expect(captureToken(env)).toBe('stored');
    expect(replaceState).not.toHaveBeenCalled();
  });

  it('returns null when there is no token anywhere', () => {
    expect(captureToken(environment('').env)).toBeNull();
    expect(captureToken(environment('#/').env)).toBeNull();
  });

  it('cleans the address even if the token is empty', () => {
    const { env, replaceState } = environment('#token=', 'stored');
    expect(captureToken(env)).toBe('stored');
    expect(replaceState).toHaveBeenCalled();
  });

  it('survives storage that throws', () => {
    const { env } = environment('#token=abc');
    env.storage = {
      getItem: () => {
        throw new Error('denied');
      },
      setItem: () => {
        throw new Error('denied');
      },
    };
    expect(captureToken(env)).toBe('abc');
    env.location = { hash: '', pathname: '/', search: '' };
    expect(captureToken(env)).toBeNull();
  });
});
