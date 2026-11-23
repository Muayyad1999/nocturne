// @vitest-environment happy-dom
/**
 * The chart's export menu: off unless a screen asks for it, and what it
 * offers when one does.
 *
 * A DOM without a browser has no canvas, so the 2D context is stood in for by
 * something that accepts every call — enough for uPlot to build its plot and
 * for these tests to reach the menu. What a real canvas produces (a PNG with
 * the title on it, a download a browser takes) is driven in Chromium by
 * `tests/ops.spec.ts`.
 */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const saved = vi.hoisted(() => ({ files: [] as { name: string; content: string | Blob; type?: string }[] }));

vi.mock('@/lib/download', () => ({
  download: (name: string, content: string | Blob, type?: string) => {
    saved.files.push({ name, content, type });
    return true;
  },
  tellSaved: () => true,
  saveFile: (name: string) => ({ ok: true, filename: name, confirmed: false, via: 'browser' }),
}));

const T0 = Date.UTC(2026, 10, 22, 21, 0, 0);
const DATA = {
  title: 'Uplink traffic',
  subtitle: 'core-sw1 · Te1/1/1',
  timestamps: [T0, T0 + 60_000, T0 + 120_000],
  series: [
    { id: 'in', label: 'Inbound', values: [10, null, 30] },
    { id: 'out', label: 'Outbound', values: [1, 2, null] },
  ],
};

let root: Root | null = null;
let host: HTMLElement | null = null;

async function render(props: Record<string, unknown> = {}): Promise<void> {
  // Imported here, after the canvas has been stood in for.
  const { TimeSeriesChart } = await import('./TimeSeriesChart');
  host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
  await act(async () => {
    root?.render(<TimeSeriesChart {...DATA} {...props} />);
  });
}

function exportButton(): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>('button[aria-label="Export this chart"]');
}

function menuItems(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>('[role="menu"] [role="menuitem"]')];
}

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  saved.files = [];
  // A context that takes any call and measures every text as its length.
  const pen = new Proxy(
    { measureText: (text: string) => ({ width: text.length * 6 }) } as Record<string, unknown>,
    { get: (target, name) => (name in target ? target[name as string] : () => undefined), set: () => true },
  );
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(pen as never);
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      disconnect() {}
      unobserve() {}
    },
  );
  if (!('fonts' in document)) {
    Object.defineProperty(document, 'fonts', { value: { ready: Promise.resolve() }, configurable: true });
  }
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
  vi.unstubAllGlobals();
});

describe('a chart that did not ask for an export menu', () => {
  it('has none: no existing chart changed', async () => {
    await render();
    expect(document.querySelector('figure')).not.toBeNull();
    expect(exportButton()).toBeNull();
    // The one header control it always had is still there.
    expect(document.querySelector('button[aria-label="Show as table"]')).not.toBeNull();
  });
});

describe('a chart that asked for one', () => {
  it('offers a picture, the picture on the clipboard and the numbers, then whatever the screen added', async () => {
    const kept: string[] = [];
    await render({
      exportMenu: { extra: [{ label: 'Keep as evidence…', onSelect: () => kept.push('asked') }] },
    });
    const button = exportButton();
    expect(button).not.toBeNull();
    act(() => button?.click());

    expect(menuItems().map((item) => item.textContent?.trim())).toEqual([
      'Save as PNG',
      'Copy image',
      'Save numbers as CSV',
      'Keep as evidence…',
    ]);
    expect(document.querySelectorAll('[role="menu"] [role="separator"]')).toHaveLength(1);

    act(() => menuItems()[3]?.click());
    expect(kept).toEqual(['asked']);
  });

  it('has no separator and no extra entry when the screen added nothing', async () => {
    await render({ exportMenu: {} });
    act(() => exportButton()?.click());
    expect(menuItems()).toHaveLength(3);
    expect(document.querySelectorAll('[role="menu"] [role="separator"]')).toHaveLength(0);
  });

  it('saves every series as CSV, a gap as an empty cell, under the name the screen gave', async () => {
    await render({ exportMenu: { filename: 'core-sw1 uplink' } });
    act(() => exportButton()?.click());
    act(() => menuItems()[2]?.click());

    expect(saved.files).toHaveLength(1);
    expect(saved.files[0]?.name).toBe('core-sw1-uplink.csv');
    expect(saved.files[0]?.type).toBe('text/csv;charset=utf-8');
    expect(saved.files[0]?.content).toBe(
      [
        'time_utc,Inbound,Outbound',
        '2026-11-22T21:00:00.000Z,10,1',
        '2026-11-22T21:01:00.000Z,,2',
        '2026-11-22T21:02:00.000Z,30,',
        '',
      ].join('\r\n'),
    );
  });

  it('saves every series in the file, the ones past the eight a chart draws too', async () => {
    const ports = Array.from({ length: 10 }, (_, index) => ({
      id: `port-${index + 1}`,
      label: `if.oper · FastEthernet0/${index + 1}`,
      values: [0, 0, index === 9 ? 1 : 0],
    }));
    await render({ series: ports, exportMenu: {} });
    // The chart says it draws eight of them…
    expect(document.body.textContent).toContain('2 more not shown');
    act(() => exportButton()?.click());
    act(() => menuItems()[2]?.click());

    // …and the file still has all ten.
    const [header, ...rows] = String(saved.files[0]?.content).trimEnd().split('\r\n');
    expect(header?.split(',')).toEqual(['time_utc', ...ports.map((port) => port.label)]);
    expect(rows.at(-1)).toBe('2026-11-22T21:02:00.000Z,0,0,0,0,0,0,0,0,0,1');
  });

  it('names the files after the title when the screen gave no name', async () => {
    await render({ exportMenu: {} });
    act(() => exportButton()?.click());
    act(() => menuItems()[2]?.click());
    expect(saved.files[0]?.name).toBe('uplink-traffic.csv');
  });

  it('does not offer a picture of the table: there is no plot to take one of', async () => {
    await render({ exportMenu: {} });
    act(() => document.querySelector<HTMLButtonElement>('button[aria-label="Show as table"]')?.click());
    act(() => exportButton()?.click());
    const disabled = menuItems().map((item) => item.getAttribute('aria-disabled') === 'true');
    // PNG and copy are off; the numbers are the same numbers either way.
    expect(disabled).toEqual([true, true, false]);
  });

  it('is not offered on a chart with nothing in it', async () => {
    await render({ timestamps: [], series: [{ id: 'in', label: 'Inbound', values: [] }], exportMenu: {} });
    expect(exportButton()).toBeNull();
  });
});
