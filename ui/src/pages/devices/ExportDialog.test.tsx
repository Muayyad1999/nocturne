// @vitest-environment happy-dom
/**
 * Which devices the export dialog offers to export when it opens.
 *
 * The device list mounts the dialog once, with nothing selected, and only
 * opens and closes it afterwards: the default has to follow the selection at
 * the moment it opens, not the one it was first mounted with. The engine's
 * own answer is driven in Chromium by `tests/devices.spec.ts`; here the
 * client is replaced, so what is checked is what the dialog asks for.
 */

import { QueryClientProvider, type QueryClient } from '@tanstack/react-query';
import { act, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createQueryClient } from '@/api/queryClient';
import { ExportDialog } from './ExportDialog';

const backend = vi.hoisted(() => ({
  rpc: vi.fn<(method: string, params?: unknown) => Promise<unknown>>(),
}));

vi.mock('@/api/client', () => ({
  rpc: backend.rpc,
  subscribe: () => () => undefined,
}));

let queryClient: QueryClient;
let root: Root | null = null;
let host: HTMLElement | null = null;

function render(node: ReactNode): void {
  act(() => root?.render(<QueryClientProvider client={queryClient}>{node}</QueryClientProvider>));
}

function scope(value: 'selected' | 'all'): HTMLInputElement | null {
  return document.querySelector<HTMLInputElement>(`input[name="export-scope"][value="${value}"]`);
}

/** The `ids` of every `devices.export` call so far, `undefined` for "every device". */
function askedFor(): (string[] | undefined)[] {
  return backend.rpc.mock.calls
    .filter(([method]) => method === 'devices.export')
    .map(([, params]) => (params as { ids?: string[] }).ids);
}

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  queryClient = createQueryClient();
  backend.rpc.mockReset();
  backend.rpc.mockImplementation(async (_method, params) => {
    const ids = (params as { ids?: string[] }).ids;
    return { text: `name,address\n${(ids ?? ['access-c', 'core-a', 'core-b']).join('\n')}\n` };
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

it('offers the devices that are selected when it opens, though it was mounted with none', async () => {
  // How the device list renders it: mounted at once, closed, nothing ticked.
  render(<ExportDialog open={false} selection={[]} onClose={() => undefined} />);
  // A device is ticked, then Export in the selection bar is pressed.
  render(<ExportDialog open selection={['core-a']} onClose={() => undefined} />);

  expect(scope('selected')?.checked).toBe(true);
  expect(scope('all')?.checked).toBe(false);
  await vi.waitFor(() => expect(askedFor()).toEqual([['core-a']]));
});

it('starts from the selection each time it opens, not from the last choice', async () => {
  render(<ExportDialog open selection={['core-a']} onClose={() => undefined} />);
  act(() => scope('all')?.click());
  expect(scope('all')?.checked).toBe(true);
  render(<ExportDialog open={false} selection={['core-a']} onClose={() => undefined} />);

  render(<ExportDialog open selection={['core-a', 'core-b']} onClose={() => undefined} />);
  expect(scope('selected')?.checked).toBe(true);
  await vi.waitFor(() => expect(askedFor().at(-1)).toEqual(['core-a', 'core-b']));
});

it('exports every device, and offers no choice, when nothing is selected', async () => {
  render(<ExportDialog open={false} selection={['core-a']} onClose={() => undefined} />);
  render(<ExportDialog open selection={[]} onClose={() => undefined} />);
  expect(scope('selected')).toBeNull();
  await vi.waitFor(() => expect(askedFor()).toEqual([undefined]));
});
