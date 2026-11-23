// @vitest-environment happy-dom
/**
 * The thresholds on the Checks view: what a field sends, what it refuses and
 * where it says so, and that a value the engine cannot read never takes away
 * the one place in the application where it can be put right.
 *
 * The client is replaced here so that the engine can be made to answer as it
 * did before writes were checked; `tests/insights.spec.ts` types into the
 * real screen against a real engine.
 */

import { QueryClientProvider, type QueryClient } from '@tanstack/react-query';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { RpcError } from '@/api/errors';
import { createQueryClient } from '@/api/queryClient';
import type { CheckInfo, InsightsSettings } from '@/api/types/insights';
import { useToasts } from '@/design/Toast';
import { ChecksTab } from './Checks';
import type { Scan } from './useScan';

const backend = vi.hoisted(() => ({
  rpc: vi.fn<(method: string, params?: unknown) => Promise<unknown>>(),
}));

vi.mock('@/api/client', () => ({
  rpc: backend.rpc,
  subscribe: () => () => undefined,
}));

const CATALOGUE: CheckInfo[] = [
  {
    id: 'no_recent_backup',
    title: 'No recent backup',
    category: 'config',
    default_severity: 'medium',
    needs: ['config backups'],
    enabled: true,
    description: 'A device whose newest stored configuration is older than the threshold.',
    not_evaluated: 'never.',
  },
];

function settings(backupAgeDays: unknown): InsightsSettings {
  return {
    enabled_checks: { no_recent_backup: true },
    include_lab: true,
    raise_alerts_from: 'high',
    auto_scan: true,
    auto_scan_delay_ms: 5000,
    thresholds: {
      flap_transitions_per_hour: 4,
      interface_error_ppm: 100,
      interface_error_min_delta: 10,
      utilisation_pct: 80,
      cpu_pct: 85,
      memory_pct: 90,
      temperature_c: 65,
      sustained_minutes: 15,
      history_hours: 24,
      backup_age_days: backupAgeDays as number,
      stale_data_hours: 168,
      qos_drop_ps: 10,
      mac_flap_min_macs: 3,
      software_min_group: 3,
      saved_pair_window_minutes: 60,
      arp_window_minutes: 60,
      change_window_minutes: 120,
    },
  };
}

/** What the engine holds, and how it answers. */
let stored: InsightsSettings;
let catalogue: () => Promise<CheckInfo[]>;
let refuseWrites: RpcError | null;

const SCAN: Scan = {
  job: undefined,
  summary: undefined,
  running: false,
  starting: false,
  start: () => undefined,
  cancel: () => undefined,
  dismiss: () => undefined,
};

let queryClient: QueryClient;
let root: Root | null = null;
let host: HTMLElement | null = null;

async function render(): Promise<void> {
  await act(async () => {
    root?.render(
      <QueryClientProvider client={queryClient}>
        <ChecksTab scan={SCAN} />
      </QueryClientProvider>,
    );
  });
  await settle();
}

async function settle(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 20));
  });
}

function field(label: string): HTMLInputElement {
  const name = [...document.querySelectorAll('label')].find((element) => element.textContent === label);
  const input = name ? document.getElementById(name.htmlFor) : null;
  if (!(input instanceof HTMLInputElement)) throw new Error(`no field labelled ${label}`);
  return input;
}

/** The text a field's description says: its hint, or what is wrong. */
function said(input: HTMLInputElement): string {
  return (input.getAttribute('aria-describedby') ?? '')
    .split(' ')
    .map((id) => document.getElementById(id)?.textContent ?? '')
    .join(' ');
}

async function type(input: HTMLInputElement, text: string): Promise<void> {
  await act(async () => {
    input.focus();
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set?.call(input, text);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

async function leave(input: HTMLInputElement): Promise<void> {
  await act(async () => input.blur());
  await settle();
}

function writes(): unknown[] {
  return backend.rpc.mock.calls.filter(([method]) => method === 'settings.set').map(([, params]) => params);
}

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  queryClient = createQueryClient();
  useToasts.setState({ toasts: [], history: [], unread: 0 });
  stored = settings(7);
  catalogue = async () => CATALOGUE;
  refuseWrites = null;
  backend.rpc.mockReset();
  backend.rpc.mockImplementation(async (method, params) => {
    if (method === 'insights.checks') return catalogue();
    if (method === 'insights.coverage') return null;
    if (method === 'settings.get') return stored;
    if (method === 'settings.set') {
      if (refuseWrites) throw refuseWrites;
      const patch = (params as { value: { thresholds?: Record<string, number> } }).value;
      stored = { ...stored, thresholds: { ...stored.thresholds, ...patch.thresholds } };
      return stored;
    }
    throw new RpcError('unknown_method', `${method} is not in this test`);
  });
  host = document.createElement('div');
  host.id = 'root';
  document.body.append(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
  queryClient.clear();
});

describe('a whole-number threshold', () => {
  it('refuses a fraction beside the field and sends nothing', async () => {
    await render();
    const days = field('Backup age');
    await type(days, '7.5');
    await leave(days);

    expect(writes()).toEqual([]);
    expect(days.getAttribute('aria-invalid')).toBe('true');
    expect(said(days)).toContain('Must be a whole number (0 or more), not 7.5.');
    // What was typed stays, so it can be corrected rather than typed again.
    expect(days.value).toBe('7.5');
  });

  it('steps in whole units', async () => {
    await render();
    expect(field('Backup age').step).toBe('1');
    expect(field('CPU').step).toBe('any');
  });

  it('saves a whole number', async () => {
    await render();
    const days = field('Backup age');
    await type(days, '30');
    await leave(days);
    expect(writes()).toEqual([{ section: 'insights', value: { thresholds: { backup_age_days: 30 } } }]);
    expect(field('Backup age').value).toBe('30');
  });
});

it('keeps what is typed while the last value is still on its way', async () => {
  let answer: () => void = () => undefined;
  await render();
  const days = field('Backup age');
  backend.rpc.mockImplementationOnce(
    (_method, params) =>
      new Promise((resolve) => {
        answer = () => {
          const patch = (params as { value: { thresholds: Record<string, number> } }).value;
          stored = { ...stored, thresholds: { ...stored.thresholds, ...patch.thresholds } };
          resolve(stored);
        };
      }),
  );
  await type(days, '30');
  await leave(days);
  await type(days, '31');
  await act(async () => answer());
  await settle();

  expect(field('Backup age').value).toBe('31');
  await leave(field('Backup age'));
  expect(writes()).toEqual([
    { section: 'insights', value: { thresholds: { backup_age_days: 30 } } },
    { section: 'insights', value: { thresholds: { backup_age_days: 31 } } },
  ]);
});

it('takes a fraction for a threshold that is one', async () => {
  await render();
  const cpu = field('CPU');
  await type(cpu, '85.5');
  await leave(cpu);
  expect(writes()).toEqual([{ section: 'insights', value: { thresholds: { cpu_pct: 85.5 } } }]);
});

it('leaves a threshold as it was when its field is cleared', async () => {
  await render();
  const days = field('Backup age');
  await type(days, '');
  await leave(days);
  expect(writes()).toEqual([]);
  expect(days.value).toBe('7');
});

it("shows the engine's refusal beside the field, not in a toast", async () => {
  // As the engine refuses (`docs/dev/features/insights.md`, Settings): the
  // message names every setting, and `details.problems` has a line for each.
  const line = '`thresholds.backup_age_days` must be a whole number of days (0 or more), not 9';
  refuseWrites = new RpcError('invalid', line, {
    section: 'insights',
    problems: [{ key: 'thresholds.backup_age_days', value: 9, message: line }],
  });
  await render();
  const days = field('Backup age');
  await type(days, '9');
  await leave(days);

  expect(writes()).toHaveLength(1);
  expect(said(days)).toContain('`thresholds.backup_age_days` must be a whole number of days (0 or more), not 9');
  expect(days.getAttribute('aria-invalid')).toBe('true');
  expect(useToasts.getState().toasts).toEqual([]);

  // Leaving it again without a change does not send the refused value twice.
  await act(async () => days.focus());
  await leave(days);
  expect(writes()).toHaveLength(1);
});

describe('a value stored that the engine cannot read', () => {
  beforeEach(() => {
    stored = settings(7.5);
    // How an engine from before writes were checked answers.
    catalogue = async () => {
      throw new RpcError(
        'internal',
        'settings section `insights` cannot be read: invalid type: floating point `7.5`, expected u32',
      );
    };
  });

  it('keeps the thresholds on screen, open, with the value that is wrong', async () => {
    await render();
    expect(document.body.textContent).toContain('The catalogue could not be read');

    const toggle = [...document.querySelectorAll('button')].find((button) => button.textContent === 'Thresholds');
    expect(toggle?.getAttribute('aria-expanded')).toBe('true');
    const days = field('Backup age');
    expect(days.value).toBe('7.5');
    expect(days.getAttribute('aria-invalid')).toBe('true');
    expect(said(days)).toContain('Stored as 7.5, which is not a whole number.');
    // The rest of the section is still there to be changed.
    expect(document.body.textContent).toContain('Scan automatically after a collection');
  });

  it('is put right there, and the catalogue comes back', async () => {
    await render();
    catalogue = async () => CATALOGUE;
    const days = field('Backup age');
    await type(days, '7');
    await leave(days);
    await settle();

    expect(writes()).toEqual([{ section: 'insights', value: { thresholds: { backup_age_days: 7 } } }]);
    expect(field('Backup age').getAttribute('aria-invalid')).toBeNull();
    // It does not fold away under the field that was just put right.
    const toggle = [...document.querySelectorAll('button')].find((button) => button.textContent === 'Thresholds');
    expect(toggle?.getAttribute('aria-expanded')).toBe('true');
    expect(document.body.textContent).not.toContain('The catalogue could not be read');
    expect(document.body.textContent).toContain('No recent backup');
  });
});
