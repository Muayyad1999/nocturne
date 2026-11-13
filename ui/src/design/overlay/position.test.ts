import { describe, expect, it } from 'vitest';
import { computePosition, type Rect } from './position';

const viewport = { width: 1000, height: 800 };
const floating = { width: 200, height: 100 };
const anchor: Rect = { x: 400, y: 300, width: 100, height: 30 };

describe('computePosition', () => {
  it('places below and aligned to the start', () => {
    expect(computePosition({ anchor, floating, viewport, placement: 'bottom-start', offset: 6 })).toMatchObject({
      x: 400,
      y: 336,
      side: 'bottom',
      align: 'start',
    });
  });

  it('aligns to the end and to the centre', () => {
    expect(computePosition({ anchor, floating, viewport, placement: 'bottom-end' }).x).toBe(300);
    expect(computePosition({ anchor, floating, viewport, placement: 'bottom' }).x).toBe(350);
  });

  it('places above, left and right', () => {
    expect(computePosition({ anchor, floating, viewport, placement: 'top-start', offset: 6 })).toMatchObject({ x: 400, y: 194, side: 'top' });
    expect(computePosition({ anchor, floating, viewport, placement: 'right', offset: 6 })).toMatchObject({ x: 506, y: 265, side: 'right' });
    expect(computePosition({ anchor, floating, viewport, placement: 'left', offset: 6 })).toMatchObject({ x: 194, y: 265, side: 'left' });
  });

  it('flips to the other side when there is no room and more room there', () => {
    const low: Rect = { ...anchor, y: 740 };
    expect(computePosition({ anchor: low, floating, viewport, placement: 'bottom-start', offset: 6 })).toMatchObject({
      side: 'top',
      y: 634,
    });
    const high: Rect = { ...anchor, y: 20 };
    expect(computePosition({ anchor: high, floating, viewport, placement: 'top' }).side).toBe('bottom');
  });

  it('stays on the preferred side when the other side is no better', () => {
    const tall = { width: 200, height: 700 };
    const middle: Rect = { ...anchor, y: 420 };
    // 336 px below, 406 px above: the menu fits neither, and above is roomier.
    expect(computePosition({ anchor: middle, floating: tall, viewport, placement: 'bottom' }).side).toBe('top');
    const upper: Rect = { ...anchor, y: 200 };
    expect(computePosition({ anchor: upper, floating: tall, viewport, placement: 'bottom' }).side).toBe('bottom');
  });

  it('shifts along the edge to stay inside the viewport', () => {
    const right: Rect = { ...anchor, x: 950, width: 40 };
    expect(computePosition({ anchor: right, floating, viewport, placement: 'bottom-start', padding: 8 }).x).toBe(792);
    const left: Rect = { ...anchor, x: -50 };
    expect(computePosition({ anchor: left, floating, viewport, placement: 'bottom-end', padding: 8 }).x).toBe(8);
  });

  it('pins an oversized box to the leading edge', () => {
    const wide = { width: 1200, height: 100 };
    expect(computePosition({ anchor, floating: wide, viewport, placement: 'bottom', padding: 8 }).x).toBe(8);
  });

  it('opens at a point for context menus', () => {
    const point: Rect = { x: 500, y: 400, width: 0, height: 0 };
    expect(computePosition({ anchor: point, floating, viewport, placement: 'bottom-start', offset: 0 })).toMatchObject({
      x: 500,
      y: 400,
    });
    const corner: Rect = { x: 990, y: 790, width: 0, height: 0 };
    const position = computePosition({ anchor: corner, floating, viewport, placement: 'bottom-start', offset: 0, padding: 8 });
    expect(position.x).toBe(792);
    expect(position.side).toBe('top');
    expect(position.y).toBe(690);
  });

  it('reports the room left on the chosen side for scrollable content', () => {
    const position = computePosition({ anchor, floating, viewport, placement: 'bottom', offset: 6, padding: 8 });
    expect(position.maxHeight).toBe(800 - 330 - 6 - 8);
    expect(position.maxWidth).toBe(1000 - 16);
    const side = computePosition({ anchor, floating, viewport, placement: 'right', offset: 6, padding: 8 });
    expect(side.maxWidth).toBe(1000 - 500 - 6 - 8);
    expect(side.maxHeight).toBe(800 - 16);
  });
});
