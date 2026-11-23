// @vitest-environment happy-dom
/**
 * What a chart reads out, in its table view, its tooltip and on its axis.
 *
 * A screen that aligns several series on one time axis puts one extra time
 * inside each hole, with no value in any series, so that the line is lifted
 * there rather than drawn across it (`alignSeries`, `storedChart`). No sample
 * was taken at that time: the table and the tooltip show the hole as a hole,
 * the CSV leaves it out (`lib/capture.ts`).
 *
 * A DOM without a browser has no canvas, so the 2D context is stood in for by
 * one that notes the text it is asked to draw: that is how the axis labels
 * are read here. `tests/ops.spec.ts` and `tests/monitoring.spec.ts` look at
 * real charts in Chromium.
 */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { formatDateTime } from '@/lib/time';
import type { TimeSeriesChartProps } from './TimeSeriesChart';

const T0 = Date.UTC(2026, 10, 23, 8, 4, 0);

let root: Root | null = null;
let host: HTMLElement | null = null;
let drawn: string[] = [];

async function render(
  props: Pick<TimeSeriesChartProps, 'timestamps' | 'series'> & Partial<TimeSeriesChartProps>,
): Promise<void> {
  const { TimeSeriesChart } = await import('./TimeSeriesChart');
  host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
  await act(async () => {
    root?.render(<TimeSeriesChart title="Reachability" utc {...props} />);
  });
}

function showTable(): void {
  act(() => document.querySelector<HTMLButtonElement>('button[aria-label="Show as table"]')?.click());
}

/** The table's rows as the text of their cells, newest first (its default order). */
function tableRows(): string[][] {
  return [...document.querySelectorAll('tbody tr:not([aria-hidden])')].map((row) =>
    [...row.querySelectorAll('td')].map((cell) => cell.textContent?.trim() ?? ''),
  );
}

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  drawn = [];
  const pen = new Proxy(
    {
      measureText: (text: string) => ({ width: text.length * 6 }),
      fillText: (text: string) => {
        drawn.push(String(text));
      },
    } as Record<string, unknown>,
    { get: (target, name) => (name in target ? target[name as string] : () => undefined), set: () => true },
  );
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(pen as never);
  // A plot needs a width to lay its axes out.
  vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(640);
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      disconnect() {}
      unobserve() {}
    },
  );
  // The lines are drawn as paths: one that takes any call, like the pen.
  vi.stubGlobal(
    'Path2D',
    class {
      constructor() {
        return new Proxy(this, { get: (target, name) => (name in target ? target[name as never] : () => undefined) });
      }
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

describe('the table view', () => {
  it('shows a hole as a hole, not as a time that was sampled', async () => {
    await render({
      // The third time is the one put inside the hole to lift the pen.
      timestamps: [T0, T0 + 60_000, T0 + 120_000, T0 + 600_000],
      series: [{ id: 'up', label: 'up', values: [1, 1, null, 0] }],
    });
    showTable();

    expect(tableRows()).toEqual([
      [formatDateTime(T0 + 600_000, { utc: true }), '0'],
      ['9m, no samples', '—'],
      [formatDateTime(T0 + 60_000, { utc: true }), '1'],
      [formatDateTime(T0, { utc: true }), '1'],
    ]);
    expect(document.body.textContent).not.toContain(formatDateTime(T0 + 120_000, { utc: true }));
  });

  it('shows a run of times with nothing recorded as one hole', async () => {
    await render({
      // A screen with a slot for every minute leaves the missing ones null.
      timestamps: [T0, T0 + 60_000, T0 + 120_000, T0 + 180_000, T0 + 240_000],
      series: [{ id: 'up', label: 'up', values: [1, null, null, null, 1] }],
    });
    showTable();
    expect(tableRows()).toEqual([
      [formatDateTime(T0 + 240_000, { utc: true }), '1'],
      ['4m, no samples', '—'],
      [formatDateTime(T0, { utc: true }), '1'],
    ]);
  });

  it('lists a time at which one series was read and another was not as a sample', async () => {
    await render({
      timestamps: [T0, T0 + 60_000],
      series: [
        { id: 'in', label: 'Inbound', values: [10, null] },
        { id: 'out', label: 'Outbound', values: [1, 2] },
      ],
    });
    showTable();
    expect(tableRows()).toEqual([
      [formatDateTime(T0 + 60_000, { utc: true }), '—', '2'],
      [formatDateTime(T0, { utc: true }), '10', '1'],
    ]);
  });

  it('keeps the precision of what was read when the screen gave no format', async () => {
    await render({
      timestamps: [T0, T0 + 60_000, T0 + 120_000],
      series: [{ id: 'loss', label: 'Loss', values: [0.3, 45.5, 0.02] }],
    });
    showTable();
    expect(tableRows().map((row) => row[1])).toEqual(['0.02', '45.5', '0.3']);
  });
});

describe('the tooltip', () => {
  /** What the crosshair's tooltip says: it is read out while the arrow keys move it. */
  function tooltip(): string | undefined {
    return document.querySelector('[aria-live]')?.textContent ?? undefined;
  }

  function step(plot: HTMLElement, key: string): void {
    act(() => {
      plot.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));
    });
  }

  it('reads a hole out as the hole, not as the time put in it to lift the pen', async () => {
    await render({
      timestamps: [T0, T0 + 60_000, T0 + 120_000, T0 + 600_000],
      series: [{ id: 'up', label: 'up', values: [1, 1, null, 0] }],
    });
    const plot = document.querySelector<HTMLElement>('[role="group"][tabindex="0"]');
    if (!plot) throw new Error('no plot to move the crosshair on');
    // Focus puts the crosshair on the last sample…
    act(() => plot.focus());
    expect(tooltip()).toContain(formatDateTime(T0 + 600_000, { utc: true }));

    // …one step back is the time inside the hole, which nobody sampled…
    step(plot, 'ArrowLeft');
    expect(tooltip()).toBe('9m, no samples');
    expect(tooltip()).not.toContain(formatDateTime(T0 + 120_000, { utc: true }));

    // …and one more is a sample again, read out with its time and value.
    step(plot, 'ArrowLeft');
    expect(tooltip()).toContain(formatDateTime(T0 + 60_000, { utc: true }));
    expect(tooltip()).toContain('1');
  });
});

describe('the value axis', () => {
  it('labels every line of a series that stays at zero differently', async () => {
    await render({
      timestamps: [T0, T0 + 60_000, T0 + 120_000],
      series: [{ id: 'oper', label: 'if.oper', values: [0, 0, 0] }],
    });
    // The axis of an all-zero series runs from 0 to 0.1. The time axis is
    // left out: its labels are times and dates.
    const labels = drawn.filter((text) => /^\d/.test(text) && !text.includes(':') && !text.includes('-'));
    expect(labels.length).toBeGreaterThan(2);
    // The plot may be drawn more than once; within one drawing the labels
    // follow each other up the axis, and no two neighbours read the same.
    expect(labels.slice(1).filter((label, index) => label === labels[index])).toEqual([]);
    expect(labels).toContain('0.1');
  });
});
