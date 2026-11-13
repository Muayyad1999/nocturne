/**
 * Pure geometry for floating layers (menus, popovers, tooltips): where to put
 * a box next to an anchor so that it stays inside the viewport.
 */

export type Side = 'top' | 'bottom' | 'left' | 'right';
export type Align = 'start' | 'center' | 'end';
export type Placement = Side | `${Side}-${Align}`;

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Size {
  width: number;
  height: number;
}

export interface PositionInput {
  /** The anchor, in viewport coordinates. A point is a rect with no size. */
  anchor: Rect;
  floating: Size;
  viewport: Size;
  placement: Placement;
  /** Gap between the anchor and the floating box. */
  offset?: number;
  /** Minimum distance kept from the viewport edges. */
  padding?: number;
}

export interface Position {
  x: number;
  y: number;
  /** The side actually used, after flipping. */
  side: Side;
  align: Align;
  /** Room available on that side, for content that can scroll. */
  maxWidth: number;
  maxHeight: number;
}

const OPPOSITE: Record<Side, Side> = { top: 'bottom', bottom: 'top', left: 'right', right: 'left' };

function clamp(value: number, min: number, max: number): number {
  // When the box is larger than the viewport, pin it to the leading edge.
  return Math.max(min, Math.min(value, Math.max(min, max)));
}

export function computePosition({
  anchor,
  floating,
  viewport,
  placement,
  offset = 6,
  padding = 8,
}: PositionInput): Position {
  const [preferred = 'bottom', align = 'center'] = placement.split('-') as [Side, Align?];

  const room: Record<Side, number> = {
    top: anchor.y - offset - padding,
    bottom: viewport.height - (anchor.y + anchor.height) - offset - padding,
    left: anchor.x - offset - padding,
    right: viewport.width - (anchor.x + anchor.width) - offset - padding,
  };

  const vertical = preferred === 'top' || preferred === 'bottom';
  const needed = vertical ? floating.height : floating.width;
  const opposite = OPPOSITE[preferred];
  // Flip only when the preferred side is too small and the other side is roomier.
  const side = room[preferred] < needed && room[opposite] > room[preferred] ? opposite : preferred;

  const crossStart = vertical ? anchor.x : anchor.y;
  const crossAnchor = vertical ? anchor.width : anchor.height;
  const crossFloating = vertical ? floating.width : floating.height;
  const crossViewport = vertical ? viewport.width : viewport.height;
  const aligned =
    align === 'start'
      ? crossStart
      : align === 'end'
        ? crossStart + crossAnchor - crossFloating
        : crossStart + (crossAnchor - crossFloating) / 2;
  const cross = clamp(aligned, padding, crossViewport - crossFloating - padding);

  let x: number;
  let y: number;
  if (side === 'bottom') {
    x = cross;
    y = anchor.y + anchor.height + offset;
  } else if (side === 'top') {
    x = cross;
    y = Math.max(padding, anchor.y - offset - floating.height);
  } else if (side === 'right') {
    x = anchor.x + anchor.width + offset;
    y = cross;
  } else {
    x = Math.max(padding, anchor.x - offset - floating.width);
    y = cross;
  }

  return {
    x: Math.round(x),
    y: Math.round(y),
    side,
    align,
    maxWidth: Math.max(0, vertical ? viewport.width - 2 * padding : room[side]),
    maxHeight: Math.max(0, vertical ? room[side] : viewport.height - 2 * padding),
  };
}
