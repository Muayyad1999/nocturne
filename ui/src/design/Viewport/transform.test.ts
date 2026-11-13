import { describe, expect, it } from 'vitest';
import { IDENTITY, fitBounds, pan, screenToWorld, worldToScreen, zoomAt, type Transform } from './transform';

const close = (actual: { x: number; y: number }, expected: { x: number; y: number }) => {
  expect(actual.x).toBeCloseTo(expected.x, 9);
  expect(actual.y).toBeCloseTo(expected.y, 9);
};

describe('screen and world', () => {
  const transform: Transform = { x: 40, y: -20, k: 2 };

  it('converts in both directions', () => {
    expect(worldToScreen(transform, { x: 10, y: 5 })).toEqual({ x: 60, y: -10 });
    expect(screenToWorld(transform, { x: 60, y: -10 })).toEqual({ x: 10, y: 5 });
  });

  it('round-trips any point', () => {
    for (const point of [
      { x: 0, y: 0 },
      { x: -123.4, y: 987.6 },
      { x: 1e6, y: -1e-3 },
    ]) {
      close(screenToWorld(transform, worldToScreen(transform, point)), point);
    }
  });

  it('is the identity at rest', () => {
    expect(worldToScreen(IDENTITY, { x: 7, y: 9 })).toEqual({ x: 7, y: 9 });
  });
});

describe('pan', () => {
  it('moves the scene by screen pixels, whatever the zoom', () => {
    expect(pan({ x: 10, y: 10, k: 3 }, 5, -5)).toEqual({ x: 15, y: 5, k: 3 });
  });
});

describe('zoomAt', () => {
  it('keeps the world point under the cursor where it is', () => {
    const before: Transform = { x: 30, y: 12, k: 1.5 };
    const cursor = { x: 400, y: 250 };
    const anchor = screenToWorld(before, cursor);
    for (const factor of [0.5, 1.1, 3]) {
      const after = zoomAt(before, cursor, factor, 0.1, 8);
      expect(after.k).toBeCloseTo(before.k * factor, 9);
      close(worldToScreen(after, anchor), cursor);
    }
  });

  it('stops at the zoom limits without drifting', () => {
    const before: Transform = { x: 0, y: 0, k: 7 };
    const cursor = { x: 100, y: 100 };
    const anchor = screenToWorld(before, cursor);
    const after = zoomAt(before, cursor, 10, 0.1, 8);
    expect(after.k).toBe(8);
    close(worldToScreen(after, anchor), cursor);
    expect(zoomAt({ x: 0, y: 0, k: 0.2 }, cursor, 0.01, 0.1, 8).k).toBe(0.1);
  });

  it('does nothing at a limit', () => {
    const atMax: Transform = { x: 5, y: 6, k: 8 };
    expect(zoomAt(atMax, { x: 50, y: 50 }, 2, 0.1, 8)).toEqual(atMax);
  });
});

describe('fitBounds', () => {
  const size = { width: 800, height: 600 };

  it('scales content down to fit, centred, inside the padding', () => {
    const bounds = { x: 100, y: 50, width: 1600, height: 400 };
    const transform = fitBounds(bounds, size, 40, 0.1, 8);
    expect(transform.k).toBeCloseTo(720 / 1600, 9);
    const topLeft = worldToScreen(transform, { x: bounds.x, y: bounds.y });
    const bottomRight = worldToScreen(transform, { x: bounds.x + bounds.width, y: bounds.y + bounds.height });
    expect(topLeft.x).toBeCloseTo(40, 9);
    expect(bottomRight.x).toBeCloseTo(760, 9);
    expect((topLeft.y + bottomRight.y) / 2).toBeCloseTo(300, 9);
  });

  it('is limited by the tighter dimension', () => {
    const transform = fitBounds({ x: 0, y: 0, width: 100, height: 1000 }, size, 0, 0.1, 8);
    expect(transform.k).toBeCloseTo(0.6, 9);
  });

  it('does not magnify small content beyond the maximum zoom', () => {
    const transform = fitBounds({ x: 0, y: 0, width: 10, height: 10 }, size, 0, 0.1, 2);
    expect(transform.k).toBe(2);
    close(worldToScreen(transform, { x: 5, y: 5 }), { x: 400, y: 300 });
  });

  it('centres a single point', () => {
    const transform = fitBounds({ x: 42, y: 7, width: 0, height: 0 }, size, 0, 0.1, 4);
    close(worldToScreen(transform, { x: 42, y: 7 }), { x: 400, y: 300 });
  });
});
