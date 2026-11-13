/**
 * Putting a real engine into the state a test needs, through the same RPC
 * endpoint the app uses. No fixtures, no database writing: if a screen can be
 * set up this way, a person can get there too.
 */

import type { APIRequestContext } from '@playwright/test';
import type { Backend } from './backend';

export interface Rpc {
  <T = unknown>(method: string, params?: unknown): Promise<T>;
}

/** An `rpc(method, params)` bound to one backend, that throws what it is told. */
export function rpcFor(request: APIRequestContext, backend: Backend): Rpc {
  return async <T,>(method: string, params?: unknown): Promise<T> => {
    const response = await request.post(`${backend.url}/api/rpc`, {
      headers: { Authorization: `Bearer ${backend.token}` },
      data: params === undefined ? { method } : { method, params },
      timeout: 60_000,
    });
    const body = (await response.json()) as
      | { ok: true; result: T }
      | { ok: false; error: { code: string; message: string } };
    if (!body.ok) throw new Error(`${method} failed: ${body.error.code} — ${body.error.message}`);
    return body.result;
  };
}

const SITES = ['HQ', 'Branch North', 'Branch South', 'DC1', 'DC2', 'Campus East'];
const ROLES = ['core', 'distribution', 'access', 'edge', 'firewall', 'wireless'];
/** Only device types the built-in pack actually ships, plus "detect it". */
const PROFILES = ['cisco_ios', 'mikrotik_routeros', 'linux', 'cisco_ios', 'mikrotik_routeros', ''];

/**
 * Adds `count` devices spread over sites, roles and device types. Devices are
 * created one call at a time because that is the only way the backend offers;
 * `concurrency` keeps a few in flight so two thousand do not take all day.
 */
export async function seedDevices(
  rpc: Rpc,
  count: number,
  { concurrency = 16, prefix = 'sw' } = {},
): Promise<void> {
  const numbers = Array.from({ length: count }, (_value, index) => index + 1);
  let next = 0;
  const worker = async () => {
    for (let index = next++; index < numbers.length; index = next++) {
      const serial = numbers[index] ?? 0;
      const at = serial % 6;
      await rpc('devices.create', {
        name: `${ROLES[at]}-${prefix}${serial}`,
        address: `10.${Math.floor(serial / 250)}.${serial % 250}.${(serial % 200) + 1}`,
        site: SITES[at],
        role: ROLES[at],
        profile: PROFILES[at],
        tags: [`rack-${serial % 9}`, serial % 3 === 0 ? 'critical' : 'managed'],
      });
    }
  };
  await Promise.all(Array.from({ length: concurrency }, worker));
}

/** A credential with a password and an enable secret, as a real one is. */
export async function seedCredential(rpc: Rpc, name = 'Campus switches'): Promise<string> {
  const credential = await rpc<{ id: string }>('credentials.create', {
    name,
    username: 'netops',
    password: 'not-a-real-password',
    enable_secret: 'not-a-real-secret',
  });
  return credential.id;
}
