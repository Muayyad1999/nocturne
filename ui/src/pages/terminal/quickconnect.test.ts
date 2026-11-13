import { describe, expect, it } from 'vitest';
import { adhocFor, parseQuickConnect, targetLabel } from './quickconnect';

function target(text: string, transport: 'ssh' | 'telnet' = 'ssh') {
  const parsed = parseQuickConnect(text, transport);
  if (!parsed.ok) throw new Error(`expected a target, got: ${parsed.problem}`);
  return parsed.target;
}

function problem(text: string, transport: 'ssh' | 'telnet' = 'ssh') {
  const parsed = parseQuickConnect(text, transport);
  if (parsed.ok) throw new Error('expected a problem');
  return parsed.problem;
}

describe('parseQuickConnect', () => {
  it('reads user@host', () => {
    expect(target('admin@10.0.0.1')).toEqual({ transport: 'ssh', username: 'admin', host: '10.0.0.1' });
  });

  it('reads a port', () => {
    expect(target('admin@sw1.example.net:2222')).toEqual({
      transport: 'ssh',
      username: 'admin',
      host: 'sw1.example.net',
      port: 2222,
    });
  });

  it('takes a bare host for Telnet, where console servers have no user name', () => {
    expect(target('console.example.net:4001', 'telnet')).toEqual({
      transport: 'telnet',
      username: '',
      host: 'console.example.net',
      port: 4001,
    });
  });

  it('keeps the colons of a bracketed IPv6 address out of the port', () => {
    expect(target('admin@[2001:db8::1]:2200')).toEqual({
      transport: 'ssh',
      username: 'admin',
      host: '2001:db8::1',
      port: 2200,
    });
    expect(target('admin@[fe80::1]')).toEqual({ transport: 'ssh', username: 'admin', host: 'fe80::1' });
  });

  it('says what is missing instead of guessing', () => {
    expect(problem('')).toMatch(/host/);
    expect(problem('10.0.0.1')).toMatch(/user name/);
    expect(problem('@10.0.0.1')).toMatch(/before the @/);
    expect(problem('admin@')).toMatch(/after the @/);
    expect(problem('admin@host:0')).toMatch(/port number/);
    expect(problem('admin@host:99999')).toMatch(/port number/);
    expect(problem('admin@host:ssh')).toMatch(/port number/);
    expect(problem('admin@2001:db8::1')).toMatch(/\[ \]/);
    expect(problem('admin@two hosts')).toMatch(/spaces/);
  });

  it('keeps an @ in the user name out of the host', () => {
    expect(target('DOMAIN\\user@host')).toMatchObject({ username: 'DOMAIN\\user', host: 'host' });
  });
});

describe('adhocFor', () => {
  it('leaves out what the backend defaults', () => {
    expect(adhocFor(target('admin@10.0.0.1'))).toEqual({
      transport: 'ssh',
      host: '10.0.0.1',
      username: 'admin',
    });
    expect(adhocFor(target('10.0.0.1:23', 'telnet'))).toEqual({
      transport: 'telnet',
      host: '10.0.0.1',
      port: 23,
    });
  });
});

describe('targetLabel', () => {
  it('reads back the way it was typed', () => {
    expect(targetLabel(target('admin@10.0.0.1'))).toBe('admin@10.0.0.1');
    expect(targetLabel(target('admin@host:2222'))).toBe('admin@host:2222');
    expect(targetLabel(target('[2001:db8::1]:23', 'telnet'))).toBe('[2001:db8::1]:23');
  });
});
