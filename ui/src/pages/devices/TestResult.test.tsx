// @vitest-environment happy-dom
/**
 * "Test connection" is where a user decides whether to trust Nocturne with
 * their network, so what it prints is worth a test of its own — including the
 * case this build's engine cannot produce on a machine with no device to
 * reach: a connection that *worked*, but only with legacy SSH algorithms.
 */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import type { ConnectionTest } from '@/api/types/core';
import { TestResult } from './TestResult';

let root: Root | null = null;
let host: HTMLElement | null = null;

function render(result: ConnectionTest): string {
  host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
  act(() => {
    root?.render(<TestResult result={result} profileName={(id) => `Named ${id}`} />);
  });
  return host.textContent ?? '';
}

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
});

const OK: ConnectionTest = {
  ok: true,
  elapsed_ms: 1_234,
  hostname: 'CORE-SW1',
  profile: 'cisco_ios',
  profile_detected: true,
  warnings: [],
  info: {
    kind: 'ssh',
    peer: '10.0.0.1:22',
    legacy_crypto: false,
    via: [],
    kex: 'curve25519-sha256',
    cipher: 'aes256-gcm@openssh.com',
    host_key_algorithm: 'ssh-ed25519',
    host_key_fingerprint: 'SHA256:abc',
  },
  detection: { profile_id: 'cisco_ios', method: 'banner', evidence: 'Cisco IOS Software' },
};

describe('a successful test', () => {
  it('says what answered, what it is, and how it was worked out', () => {
    const text = render(OK);
    expect(text).toContain('Connected and logged in');
    expect(text).toContain('CORE-SW1');
    expect(text).toContain('Named cisco_ios (detected)');
    expect(text).toContain('10.0.0.1:22');
    expect(text).toContain('SHA256:abc');
    expect(text).toContain('login banner');
  });

  it('shows a warning even though the connection worked', () => {
    const text = render({
      ...OK,
      info: { ...OK.info!, legacy_crypto: true },
      warnings: [
        'Connected with legacy SSH algorithms that modern clients refuse; the device’s SSH configuration or software should be updated.',
      ],
    });
    expect(text).toContain('Connected and logged in');
    expect(text).toContain('legacy SSH algorithms');
  });
});

describe('a failed test', () => {
  it('names the stage it stopped at and what to look at', () => {
    const text = render({
      ok: false,
      failed_at: 'login',
      error: { code: 'auth', message: 'core-sw1: authentication failed.' },
      elapsed_ms: 820,
      profile_detected: false,
      warnings: [],
    });
    expect(text).toContain('Could not connect');
    expect(text).toContain('authentication failed');
    expect(text).toContain('check the credential');
    // The stages are listed, with the failed one marked.
    expect(host?.querySelector('[data-state="failed"]')?.textContent).toContain('Log in');
    expect(host?.querySelector('[data-state="done"]')?.textContent).toContain('Reach the device');
  });

  it('does not invent facts it was not given', () => {
    const text = render({
      ok: false,
      failed_at: 'connect',
      error: { code: 'unreachable', message: '192.0.2.1: no route to host.' },
      elapsed_ms: 10_000,
      profile_detected: false,
      warnings: [],
    });
    expect(text).not.toContain('Device type');
    expect(text).not.toContain('Calls itself');
    expect(text).toContain('check the address and port');
  });
});
