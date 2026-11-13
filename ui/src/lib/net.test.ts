import { describe, expect, it } from 'vitest';
import { cidrContains, compareIp, compareNatural, normalizeMac, parseCidr, parseIp } from './net';

describe('parseIp', () => {
  it('parses IPv4', () => {
    expect(parseIp('192.0.2.1')).toEqual({ version: 4, value: 0xc0000201n });
    expect(parseIp(' 0.0.0.0 ')).toEqual({ version: 4, value: 0n });
    expect(parseIp('255.255.255.255')?.value).toBe(0xffffffffn);
  });

  it('rejects malformed IPv4', () => {
    for (const text of ['256.0.0.1', '1.2.3', '1.2.3.4.5', '01.2.3.4', '1.2.3.x', '', 'core-sw-01']) {
      expect(parseIp(text)).toBeNull();
    }
  });

  it('parses IPv6 in full, compressed and IPv4-mapped forms', () => {
    expect(parseIp('::')).toEqual({ version: 6, value: 0n });
    expect(parseIp('::1')).toEqual({ version: 6, value: 1n });
    expect(parseIp('2001:db8::1')?.value).toBe(0x20010db8000000000000000000000001n);
    expect(parseIp('2001:0db8:0000:0000:0000:0000:0000:0001')?.value).toBe(parseIp('2001:db8::1')?.value);
    expect(parseIp('::ffff:192.0.2.1')?.value).toBe(0xffffc0000201n);
    expect(parseIp('fe80::')?.value).toBe(0xfe80n << 112n);
  });

  it('rejects malformed IPv6', () => {
    for (const text of ['2001:db8', '1::2::3', '2001:db8::g', '1:2:3:4:5:6:7:8:9', '1:2:3:4:5:6:7::8', '12345::']) {
      expect(parseIp(text)).toBeNull();
    }
  });
});

describe('parseCidr / cidrContains', () => {
  it('computes the network address', () => {
    expect(parseCidr('192.0.2.77/24')).toEqual({
      version: 4,
      value: 0xc000024dn,
      prefix: 24,
      network: 0xc0000200n,
    });
    expect(parseCidr('192.0.2.1')?.prefix).toBe(32);
    expect(parseCidr('2001:db8::1/32')?.network).toBe(0x20010db8n << 96n);
    expect(parseCidr('0.0.0.0/0')?.network).toBe(0n);
  });

  it('rejects invalid prefixes', () => {
    for (const text of ['192.0.2.0/33', '192.0.2.0/-1', '192.0.2.0/', '192.0.2.0/24/1', '2001:db8::/129']) {
      expect(parseCidr(text)).toBeNull();
    }
  });

  it('tests membership', () => {
    const block = parseCidr('198.51.100.0/22');
    const inside = parseIp('198.51.103.255');
    const outside = parseIp('198.51.104.0');
    const v6 = parseIp('2001:db8::1');
    expect(block && inside && cidrContains(block, inside)).toBe(true);
    expect(block && outside && cidrContains(block, outside)).toBe(false);
    expect(block && v6 && cidrContains(block, v6)).toBe(false);
    const everything = parseCidr('0.0.0.0/0');
    expect(everything && inside && cidrContains(everything, inside)).toBe(true);
  });
});

describe('ordering', () => {
  it('sorts addresses numerically, IPv4 first, non-addresses last', () => {
    const input = ['10.0.0.10', '2001:db8::2', 'core-sw-01', '10.0.0.9', '9.255.0.1', '2001:db8::1', 'access-2'];
    expect([...input].sort(compareIp)).toEqual([
      '9.255.0.1',
      '10.0.0.9',
      '10.0.0.10',
      '2001:db8::1',
      '2001:db8::2',
      'access-2',
      'core-sw-01',
    ]);
  });

  it('orders prefixes of the same network by length', () => {
    expect(['10.0.0.0/24', '10.0.0.0/8', '10.0.0.0/16'].sort(compareIp)).toEqual([
      '10.0.0.0/8',
      '10.0.0.0/16',
      '10.0.0.0/24',
    ]);
  });

  it('sorts interface names naturally', () => {
    expect(['Gi1/0/10', 'Gi1/0/2', 'Gi1/0/1', 'Te1/1/1'].sort(compareNatural)).toEqual([
      'Gi1/0/1',
      'Gi1/0/2',
      'Gi1/0/10',
      'Te1/1/1',
    ]);
  });
});

describe('normalizeMac', () => {
  it('accepts the common notations', () => {
    expect(normalizeMac('AA-BB-CC-DD-EE-FF')).toBe('aa:bb:cc:dd:ee:ff');
    expect(normalizeMac('aabb.ccdd.eeff')).toBe('aa:bb:cc:dd:ee:ff');
    expect(normalizeMac('aabbccddeeff')).toBe('aa:bb:cc:dd:ee:ff');
    expect(normalizeMac('aa:bb:cc:dd:ee:ff')).toBe('aa:bb:cc:dd:ee:ff');
  });

  it('rejects anything else', () => {
    expect(normalizeMac('aa:bb:cc:dd:ee')).toBeNull();
    expect(normalizeMac('zz:bb:cc:dd:ee:ff')).toBeNull();
    expect(normalizeMac('')).toBeNull();
  });
});
