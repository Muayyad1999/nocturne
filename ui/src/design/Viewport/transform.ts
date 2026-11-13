/**
 * The geometry of a pan/zoom viewport. "World" is the coordinate system the
 * scene is drawn in; "screen" is pixels from the viewport's top-left corner.
 *
 *   screen = world × k + (x, y)
 */

export interface Point {
  x: number;
  y: number;
}

export interface Size {
  width: number;
  height: number;
}

export interface Bounds extends Point, Size {}

export interface Transform {
  /** Screen position of the world origin. */
  x: number;
  y: number;
  /** Zoom: screen pixels per world unit. */
  k: number;
}

export const IDENTITY: Transform = { x: 0, y: 0, k: 1 };

export function worldToScreen(transform: Transform, point: Point): Point {
  return { x: point.x * transform.k + transform.x, y: point.y * transform.k + transform.y };
}

export function screenToWorld(transform: Transform, point: Point): Point {
  return { x: (point.x - transform.x) / transform.k, y: (point.y - transform.y) / transform.k };
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

export function pan(transform: Transform, dx: number, dy: number): Transform {
  return { ...transform, x: transform.x + dx, y: transform.y + dy };
}

/**
 * Zooms by `factor` while keeping the world point under the screen point
 * `at` exactly where it is: zooming towards the cursor.
 */
export function zoomAt(
  transform: Transform,
  at: Point,
  factor: number,
  minZoom: number,
  maxZoom: number,
): Transform {
  const k = clamp(transform.k * factor, minZoom, maxZoom);
  const ratio = k / transform.k;
  return {
    k,
    x: at.x - (at.x - transform.x) * ratio,
    y: at.y - (at.y - transform.y) * ratio,
  };
}

/**
 * The transform that shows all of `bounds` centred in a viewport of `size`,
 * with `padding` pixels around it. Content smaller than the viewport is not
 * magnified beyond `maxZoom`.
 */
export function fitBounds(
  bounds: Bounds,
  size: Size,
  padding: number,
  minZoom: number,
  maxZoom: number,
): Transform {
  const availableWidth = Math.max(1, size.width - 2 * padding);
  const availableHeight = Math.max(1, size.height - 2 * padding);
  const scale = Math.min(
    bounds.width > 0 ? availableWidth / bounds.width : maxZoom,
    bounds.height > 0 ? availableHeight / bounds.height : maxZoom,
  );
  const k = clamp(scale, minZoom, maxZoom);
  return {
    k,
    x: size.width / 2 - (bounds.x + bounds.width / 2) * k,
    y: size.height / 2 - (bounds.y + bounds.height / 2) * k,
  };
}
