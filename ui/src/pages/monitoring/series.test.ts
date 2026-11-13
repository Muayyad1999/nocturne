import { describe, expect, test } from 'vitest';
import type { MetricSeries } from '@/api/types/monitor';
import { alignSeries, range, RANGES } from './series';

function series(points: [number, number][], step_s = 10, instance = ''): MetricSeries {
  return { device_id: 'd1', metric: 'cpu.pct', instance, points, step_s, rollup: 'raw' };
}

const MINUTE = 60_000;

describe('aligning metric series for a chart', () => {
  test('a stretch with no samples becomes a hole, not a line across it', () => {
    // Polled every minute; the device said nothing for five minutes.
    const answer = series([
      [0, 10],
      [MINUTE, 11],
      [2 * MINUTE, 12],
      [7 * MINUTE, 40],
      [8 * MINUTE, 41],
    ]);

    const { timestamps, values, gaps } = alignSeries([answer]);

    expect(gaps).toEqual([{ from: 2 * MINUTE, to: 7 * MINUTE }]);
    // The extra timestamp sits one sample after the last real one, and no
    // series has a value there: that is what lifts the pen.
    expect(timestamps).toEqual([0, MINUTE, 2 * MINUTE, 3 * MINUTE, 7 * MINUTE, 8 * MINUTE]);
    expect(values[0]).toEqual([10, 11, 12, null, 40, 41]);
  });

  test('ordinary jitter between polls is not a hole', () => {
    // A minute apart, give or take the jitter the scheduler adds.
    const answer = series([
      [0, 1],
      [58_000, 2],
      [121_000, 3],
      [179_000, 4],
    ]);

    const { gaps, values } = alignSeries([answer]);

    expect(gaps).toEqual([]);
    expect(values[0]).toEqual([1, 2, 3, 4]);
  });

  test('a series missing one bucket another has is null there, not zero', () => {
    const first = series([
      [0, 1],
      [MINUTE, 2],
      [2 * MINUTE, 3],
    ]);
    const second = series(
      [
        [0, 9],
        [2 * MINUTE, 7],
      ],
      10,
      'Gi0/2',
    );

    const { timestamps, values, gaps } = alignSeries([first, second]);

    expect(gaps).toEqual([]);
    expect(timestamps).toEqual([0, MINUTE, 2 * MINUTE]);
    expect(values[1]).toEqual([9, null, 7]);
  });

  test('samples can never be closer than the bucket width the backend used', () => {
    // Five-minute roll-ups: consecutive buckets are normal, a missing one is a gap.
    const answer = series(
      [
        [0, 1],
        [300_000, 2],
        [900_000, 3],
      ],
      300,
    );

    const { gaps, spacing } = alignSeries([answer]);

    expect(spacing).toBe(300_000);
    expect(gaps).toEqual([{ from: 300_000, to: 900_000 }]);
  });

  test('an empty answer draws nothing at all rather than a line at zero', () => {
    const { timestamps, values, gaps } = alignSeries([series([])]);

    expect(timestamps).toEqual([]);
    expect(values).toEqual([[]]);
    expect(gaps).toEqual([]);
  });

  test('two samples with nothing to compare against are left as they are', () => {
    // One distance says nothing about what is normal, so neither is invented.
    const { gaps, timestamps } = alignSeries([series([[0, 1], [MINUTE, 2]])]);

    expect(gaps).toEqual([]);
    expect(timestamps).toEqual([0, MINUTE]);
  });
});

describe('the chart time ranges', () => {
  test('every range asks for points finer than it spans', () => {
    for (const entry of RANGES) {
      const points = entry.span / (entry.step_s * 1000);
      expect(points).toBeGreaterThanOrEqual(60);
      expect(points).toBeLessThanOrEqual(5_000);
    }
  });

  test('an unknown range id falls back to the hour rather than breaking', () => {
    expect(range('nonsense').id).toBe('1h');
    expect(range('24h').span).toBe(86_400_000);
  });
});
