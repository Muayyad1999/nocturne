import { describe, expect, it } from 'vitest';
import { fieldShown, initialValues, listOf, parseHeaders, parseSizes, SPECS, type Values } from './specs';

const spec = (id: string) => {
  const found = SPECS[id];
  if (!found) throw new Error(`no spec for ${id}`);
  return found;
};

/** The form as it starts, plus whatever the test types into it. */
function form(id: string, typed: Values = {}): Values {
  return { ...initialValues(SPECS[id]), ...typed };
}

const params = (id: string, typed: Values = {}) => spec(id).params(form(id, typed));

describe('a calculator is not called until it can be answered', () => {
  it('asks nothing while the required field is empty', () => {
    expect(params('tools.subnet')).toBeNull();
    expect(params('tools.subnet', { input: '  ' })).toBeNull();
    expect(params('tools.contains', { a: '10.0.0.0/8' })).toBeNull();
    expect(params('tools.subnet.split', { cidr: '10.0.0.0/24' })).toBeNull();
  });

  it('asks as soon as it can, and leaves judging the value to the engine', () => {
    expect(params('tools.subnet', { input: '10.0.300.1/24' })).toEqual({ input: '10.0.300.1/24' });
  });

  it('holds back a two-field tool until both are there', () => {
    expect(params('tools.ip.range', { first: '10.0.0.5' })).toBeNull();
    expect(params('tools.ip.range', { first: '10.0.0.5', last: '10.0.0.70' })).toEqual({
      first: '10.0.0.5',
      last: '10.0.0.70',
    });
  });

  it('needs two blocks before it can say whether any overlap', () => {
    expect(params('tools.overlaps', { cidrs: '10.0.0.0/24' })).toBeNull();
    expect(params('tools.overlaps', { cidrs: '10.0.0.0/23\n10.0.1.0/24' })).toEqual({
      cidrs: ['10.0.0.0/23', '10.0.1.0/24'],
    });
  });
});

describe('an empty optional field is left out of the call', () => {
  it('never sends an empty count, because 0 means "until cancelled" for a ping', () => {
    expect(params('probe.ping', { aim: 'host', host: '10.0.0.1', count: '' })).toEqual({
      host: '10.0.0.1',
    });
  });

  it('sends a count of 0 when that is what was asked for', () => {
    expect(params('probe.ping', { aim: 'host', host: '10.0.0.1', count: '0' })).toEqual({
      host: '10.0.0.1',
      count: 0,
    });
  });

  it('leaves out a number that is not one, rather than sending NaN', () => {
    expect(params('probe.ping', { aim: 'host', host: '10.0.0.1', count: 'five' })).toEqual({
      host: '10.0.0.1',
    });
  });

  it('omits an optional text field but keeps a switch that is off, which the backend reads as off', () => {
    expect(params('probe.ports', { aim: 'host', host: '10.0.0.1', ports: '', banners: '' })).toEqual({
      host: '10.0.0.1',
      banners: false,
    });
  });
});

describe('a probe aims at one thing', () => {
  it('sends a host, never a host and a device together (the backend refuses both)', () => {
    const both = form('probe.ping', { aim: 'host', host: '10.0.0.1', device: 'core-sw1' });
    expect(spec('probe.ping').params(both)).toEqual({ host: '10.0.0.1', count: 5 });
  });

  it('sends the device when that is what is aimed at', () => {
    expect(params('probe.ping', { aim: 'device', device: 'core-sw1' })).toEqual({
      device: 'core-sw1',
      count: 5,
    });
  });

  it('asks nothing while the chosen kind of target is empty', () => {
    expect(params('probe.ping', { aim: 'device', host: '10.0.0.1' })).toBeNull();
  });

  it('shows the host field only while a host is what is aimed at', () => {
    const host = spec('probe.ping').fields.find((field) => field.name === 'host');
    const device = spec('probe.ping').fields.find((field) => field.name === 'device');
    const values = form('probe.ping', { aim: 'device' });
    expect(host && fieldShown(host, values)).toBe(false);
    expect(device && fieldShown(device, values)).toBe(true);
  });
});

describe('the lines a person types', () => {
  it('reads a VLSM request whichever way round the name and the number are', () => {
    expect(parseSizes('Staff 200\n50 Voice\n2\n\n  ')).toEqual([
      { name: 'Staff', hosts: 200 },
      { name: 'Voice', hosts: 50 },
      { hosts: 2 },
    ]);
  });

  it('keeps a name made of several words', () => {
    expect(parseSizes('Guest wireless 120')).toEqual([{ name: 'Guest wireless', hosts: 120 }]);
  });

  it('ignores a line with no number in it instead of guessing one', () => {
    expect(parseSizes('Staff\nVoice 10')).toEqual([{ name: 'Voice', hosts: 10 }]);
  });

  it('reads headers as a device’s documentation writes them, and ignores a line that is not one', () => {
    expect(parseHeaders('Authorization: Bearer abc\nX-Trace:  1 \nnot a header\n: nothing')).toEqual([
      ['Authorization', 'Bearer abc'],
      ['X-Trace', '1'],
    ]);
  });

  it('splits a list on commas, semicolons and lines alike', () => {
    expect(listOf('22,80 ; 443\n8080')).toEqual(['22', '80', '443', '8080']);
  });

  it('sends the encapsulations as a list, outermost first', () => {
    expect(params('tools.mtu', { link_mtu: '1500', encapsulations: 'gre, ipsec-esp-tunnel' })).toEqual({
      link_mtu: 1500,
      encapsulations: ['gre', 'ipsec-esp-tunnel'],
    });
  });
});

describe('whitespace is part of the input where it is part of the answer', () => {
  it('keeps the text of a digest exactly as typed, trailing spaces and all', () => {
    expect(params('tools.hash', { mode: 'text', text: 'nocturne ' })).toEqual({ text: 'nocturne ' });
  });

  it('sends Base64 bytes trimmed, because a pasted blob carries a newline', () => {
    expect(params('tools.hash', { mode: 'base64', text: ' bm9jdHVybmU=\n' })).toEqual({
      base64: 'bm9jdHVybmU=',
    });
  });
});

describe('the tools whose input is a secret', () => {
  it('are marked so their inputs are never written to this computer', () => {
    for (const id of [
      'tools.secret.decode',
      'tools.secret.encode',
      'tools.secret.identify',
      'tools.password',
      'tools.keys.generate',
    ]) {
      expect(spec(id).secret, id).toBe(true);
    }
  });

  it('are the only ones that are', () => {
    const secret = Object.entries(SPECS)
      .filter(([, entry]) => entry.secret)
      .map(([id]) => id)
      .sort();
    expect(secret).toEqual([
      'tools.keys.generate',
      'tools.password',
      'tools.secret.decode',
      'tools.secret.encode',
      'tools.secret.identify',
    ]);
  });

  it('asks for a decode as soon as there is a line, and lets the engine recognise the encoding', () => {
    expect(params('tools.secret.decode', { text: 'password 7 0822455D0A16' })).toEqual({
      text: 'password 7 0822455D0A16',
    });
    expect(params('tools.secret.decode', { text: 'x', kind: 'junos9' })).toEqual({
      text: 'x',
      kind: 'junos9',
    });
  });

  it('asks for a password with the classes as switches, not as absent fields', () => {
    expect(params('tools.password')).toEqual({
      length: 20,
      count: 5,
      upper: true,
      lower: true,
      digits: true,
      symbols: true,
      avoid_ambiguous: false,
    });
  });
});

describe('the servers', () => {
  it('can be started with nothing said, which uses the settings’ own defaults', () => {
    expect(params('tftp.start')).toEqual({ allow_write: false, allow_overwrite: false });
  });

  it('never asks to overwrite when uploads themselves are off', () => {
    expect(params('tftp.start', { root: '/srv/tftp', allow_overwrite: 'on' })).toEqual({
      root: '/srv/tftp',
      allow_write: false,
      allow_overwrite: false,
    });
  });

  it('allows overwriting only when both switches are on', () => {
    expect(params('tftp.start', { allow_write: 'on', allow_overwrite: 'on' })).toEqual({
      allow_write: true,
      allow_overwrite: true,
    });
  });
});

describe('a device probe', () => {
  it('waits for both the device and the target: this text ends up on a command line', () => {
    expect(params('probe.device.ping', { device_id: 'core-sw1' })).toBeNull();
    expect(params('probe.device.ping', { target: '10.0.0.1' })).toBeNull();
    expect(params('probe.device.ping', { device_id: 'core-sw1', target: '10.0.0.1' })).toEqual({
      device_id: 'core-sw1',
      target: '10.0.0.1',
    });
  });

  it('passes an injection attempt through untouched, for the backend to refuse by name', () => {
    expect(params('probe.device.ping', { device_id: 'sw1', target: '8.8.8.8; reload' })).toEqual({
      device_id: 'sw1',
      target: '8.8.8.8; reload',
    });
  });
});
