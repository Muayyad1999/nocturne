// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from 'vitest';
import { storageKey } from '@/lib/storage';
import { prefill } from './data';

const remembered = (id: string): unknown => {
  const raw = window.localStorage.getItem(storageKey(`tools.${id}`));
  return raw === null ? null : JSON.parse(raw);
};

beforeEach(() => {
  window.localStorage.clear();
});

describe('opening a calculator from a row of a reference table', () => {
  it('hands the form the row, as if it had been typed there', () => {
    expect(prefill('tools.qos.marking', { value: '46', as: 'dscp' })).toBe(true);
    expect(remembered('tools.qos.marking')).toEqual({ value: '46', as: 'dscp' });
  });

  it('keeps what the form already remembered, and changes only what the row says', () => {
    // Somebody sized a link for ten G.729 calls, then looked a codec up.
    window.localStorage.setItem(
      storageKey('tools.tools.qos.voice'),
      JSON.stringify({ codec: 'g729', calls: '10', link: 'ppp', crtp: 'on' }),
    );
    expect(prefill('tools.qos.voice', { codec: 'g711' })).toBe(true);
    expect(remembered('tools.qos.voice')).toMatchObject({
      codec: 'g711',
      calls: '10',
      link: 'ppp',
      crtp: 'on',
    });
  });

  it('starts every other field where the form itself would start it', () => {
    prefill('tools.qos.voice', { link: 'ppp' });
    // No number of calls is supplied for the reader here either.
    expect(remembered('tools.qos.voice')).toMatchObject({ link: 'ppp', calls: '', codec: '' });
  });

  it('writes nothing for a tool whose inputs are secrets, whatever it is handed', () => {
    expect(prefill('tools.secret.decode', { text: 'password 7 0822455D0A16' })).toBe(false);
    expect(remembered('tools.secret.decode')).toBeNull();
    expect(JSON.stringify({ ...window.localStorage })).not.toContain('0822455D0A16');
  });

  it('writes nothing for a tool this screen has no form for', () => {
    expect(prefill('tools.something.new', { value: '1' })).toBe(false);
    expect(window.localStorage.length).toBe(0);
  });
});
