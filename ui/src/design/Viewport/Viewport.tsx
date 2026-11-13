import { Minus, Plus, Scan } from 'lucide-react';
import {
  createContext,
  use,
  useCallback,
  useEffect,
  useId,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
  type Ref,
} from 'react';
import { cx } from '@/lib/cx';
import { formatPercent } from '@/lib/format';
import { IconButton } from '../Button';
import {
  IDENTITY,
  fitBounds,
  pan,
  screenToWorld,
  worldToScreen,
  zoomAt,
  type Bounds,
  type Point,
  type Size,
  type Transform,
} from './transform';
import styles from './Viewport.module.css';

export interface ViewportApi {
  /** The current pan and zoom. */
  transform: Transform;
  /** Shows the whole scene, or the given world bounds. */
  fit: (bounds?: Bounds) => void;
  /** Zooms by a factor around the centre of the view. */
  zoomBy: (factor: number) => void;
  setTransform: (transform: Transform) => void;
  /** Pixels from the viewport's top-left corner → world coordinates. */
  screenToWorld: (point: Point) => Point;
  worldToScreen: (point: Point) => Point;
  /** A pointer event's `clientX` / `clientY` → world coordinates. */
  clientToWorld: (clientX: number, clientY: number) => Point;
}

const ViewportContext = createContext<ViewportApi | null>(null);

/**
 * For content drawn inside a Viewport: the current transform and the
 * conversions between screen and world, e.g. to keep a label the same size at
 * every zoom or to drop a node where the pointer is.
 */
export function useViewport(): ViewportApi {
  const api = use(ViewportContext);
  if (!api) throw new Error('useViewport must be used inside a <Viewport>');
  return api;
}

export interface ViewportProps {
  /** SVG content, drawn in world coordinates. */
  children: ReactNode;
  /** Names the scene for assistive technology. */
  'aria-label': string;
  /** The scene's extent in world units, used by fit. Measured from the drawing when omitted. */
  contentBounds?: Bounds;
  minZoom?: number;
  maxZoom?: number;
  /** Fits the scene once, when the viewport first has a size. */
  fitOnMount?: boolean;
  /** Draws a dot grid that moves with the scene, so panning and zooming are legible on an empty canvas. */
  grid?: boolean;
  /** Shows the zoom and fit buttons. */
  controls?: boolean;
  /** Gives the parent the same API that `useViewport` gives the content. */
  ref?: Ref<ViewportApi>;
  className?: string;
}

const FIT_PADDING = 32;
const KEY_PAN = 48;
const KEY_ZOOM = 1.25;
const GRID_SPACING = 24;
/** Below this zoom the grid would be denser than it is useful. */
const GRID_MIN_ZOOM = 0.4;
/** A press becomes a pan only after moving this far, so clicks reach the content. */
const DRAG_THRESHOLD = 4;

/** A position in client coordinates, relative to the viewport's top-left corner. */
function localPoint(root: Element | null, clientX: number, clientY: number): Point {
  const rect = root?.getBoundingClientRect();
  return { x: clientX - (rect?.left ?? 0), y: clientY - (rect?.top ?? 0) };
}

/**
 * A pan/zoom/fit container for SVG scenes; the topology and lab canvases are
 * built on it. Wheel zooms towards the cursor, drag pans, two fingers pinch.
 * From the keyboard: arrows pan, + and − zoom, 0 fits, 1 resets to 100%.
 *
 * A press only becomes a pan once the pointer moves, so clicks on the content
 * work as usual. Content that handles its own dragging (a movable node) stops
 * propagation of `pointerdown`, and the viewport leaves that gesture alone.
 */
export function Viewport({
  children,
  contentBounds,
  minZoom = 0.1,
  maxZoom = 8,
  fitOnMount = true,
  grid = false,
  controls = true,
  ref,
  className,
  ...rest
}: ViewportProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<SVGGElement>(null);
  const [transform, setTransform] = useState<Transform>(IDENTITY);
  const [size, setSize] = useState<Size | null>(null);
  const [panning, setPanning] = useState(false);
  /** Pressed pointers by id, for drag (one) and pinch (two). */
  const pointers = useRef(new Map<number, Point>());
  /** Where the first press landed; null once it has turned into a pan. */
  const pressOrigin = useRef<Point | null>(null);
  const gridId = useId();

  const fit = useCallback(
    (bounds?: Bounds) => {
      const root = rootRef.current;
      if (!root) return;
      const target = bounds ?? contentBounds ?? contentRef.current?.getBBox();
      if (!target || (target.width === 0 && target.height === 0)) return;
      setTransform(
        fitBounds(target, { width: root.clientWidth, height: root.clientHeight }, FIT_PADDING, minZoom, maxZoom),
      );
    },
    [contentBounds, minZoom, maxZoom],
  );

  const zoomBy = useCallback(
    (factor: number) => {
      const root = rootRef.current;
      if (!root) return;
      const centre = { x: root.clientWidth / 2, y: root.clientHeight / 2 };
      setTransform((current) => zoomAt(current, centre, factor, minZoom, maxZoom));
    },
    [minZoom, maxZoom],
  );

  const api = useMemo<ViewportApi>(
    () => ({
      transform,
      fit,
      zoomBy,
      setTransform,
      screenToWorld: (point) => screenToWorld(transform, point),
      worldToScreen: (point) => worldToScreen(transform, point),
      clientToWorld: (clientX, clientY) =>
        screenToWorld(transform, localPoint(rootRef.current, clientX, clientY)),
    }),
    [transform, fit, zoomBy],
  );
  useImperativeHandle(ref, () => api, [api]);

  // Track the viewport's size.
  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const observer = new ResizeObserver(() => {
      setSize({ width: root.clientWidth, height: root.clientHeight });
    });
    observer.observe(root);
    return () => observer.disconnect();
  }, []);

  // Fit once, as soon as there is a size to fit into.
  const fitted = useRef(false);
  useLayoutEffect(() => {
    if (!fitOnMount || fitted.current || !size || size.width === 0 || size.height === 0) return;
    fitted.current = true;
    fit();
  }, [fitOnMount, size, fit]);

  // Wheel zooms towards the cursor. Registered by hand: the handler must be
  // able to prevent the page from scrolling, which React's listener cannot.
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const at = localPoint(root, event.clientX, event.clientY);
      // Lines → pixels; a pinch on a trackpad arrives as ctrl+wheel with small deltas.
      const delta = event.deltaY * (event.deltaMode === WheelEvent.DOM_DELTA_LINE ? 16 : 1);
      const factor = Math.exp(-delta * (event.ctrlKey ? 0.01 : 0.0015));
      setTransform((current) => zoomAt(current, at, factor, minZoom, maxZoom));
    };
    root.addEventListener('wheel', onWheel, { passive: false });
    return () => root.removeEventListener('wheel', onWheel);
  }, [minZoom, maxZoom]);

  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    // Primary button, middle button or touch; and not on the controls.
    if (event.button > 1 || (event.target as Element).closest('[data-viewport-controls]')) return;
    const point = localPoint(event.currentTarget, event.clientX, event.clientY);
    pointers.current.set(event.pointerId, point);
    if (pointers.current.size === 1) {
      pressOrigin.current = point;
    } else {
      // A second finger: this is a pinch, not a click on the content.
      pressOrigin.current = null;
      for (const id of pointers.current.keys()) event.currentTarget.setPointerCapture(id);
      setPanning(true);
    }
  };

  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const active = pointers.current;
    const previous = active.get(event.pointerId);
    if (!previous) return;
    const next = localPoint(event.currentTarget, event.clientX, event.clientY);

    const origin = pressOrigin.current;
    if (origin) {
      if (Math.hypot(next.x - origin.x, next.y - origin.y) < DRAG_THRESHOLD) return;
      // From here on it is a pan: capture, so it continues outside the viewport.
      pressOrigin.current = null;
      event.currentTarget.setPointerCapture(event.pointerId);
      setPanning(true);
    }

    if (active.size === 1) {
      setTransform((current) => pan(current, next.x - previous.x, next.y - previous.y));
    } else if (active.size === 2) {
      // Pinch: scale by the change in distance, around the moving midpoint.
      const other = [...active].find(([id]) => id !== event.pointerId)?.[1];
      if (other) {
        const before = Math.hypot(previous.x - other.x, previous.y - other.y);
        const after = Math.hypot(next.x - other.x, next.y - other.y);
        const middle = { x: (next.x + other.x) / 2, y: (next.y + other.y) / 2 };
        const dx = (next.x - previous.x) / 2;
        const dy = (next.y - previous.y) / 2;
        setTransform((current) =>
          zoomAt(pan(current, dx, dy), middle, before > 0 ? after / before : 1, minZoom, maxZoom),
        );
      }
    }
    active.set(event.pointerId, next);
  };

  const onPointerEnd = (event: PointerEvent<HTMLDivElement>) => {
    pointers.current.delete(event.pointerId);
    if (pointers.current.size === 0) {
      pressOrigin.current = null;
      setPanning(false);
    }
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.target !== event.currentTarget || event.ctrlKey || event.metaKey || event.altKey) return;
    const step = event.shiftKey ? KEY_PAN * 4 : KEY_PAN;
    switch (event.key) {
      case 'ArrowLeft':
        setTransform((current) => pan(current, step, 0));
        break;
      case 'ArrowRight':
        setTransform((current) => pan(current, -step, 0));
        break;
      case 'ArrowUp':
        setTransform((current) => pan(current, 0, step));
        break;
      case 'ArrowDown':
        setTransform((current) => pan(current, 0, -step));
        break;
      case '+':
      case '=':
        zoomBy(KEY_ZOOM);
        break;
      case '-':
      case '_':
        zoomBy(1 / KEY_ZOOM);
        break;
      case '0':
        fit();
        break;
      case '1':
        zoomBy(1 / transform.k);
        break;
      default:
        return;
    }
    event.preventDefault();
  };

  const zoomLabel = formatPercent(transform.k, { fromRatio: true, digits: 0 });

  return (
    <div
      ref={rootRef}
      className={cx(styles.viewport, className)}
      role="application"
      aria-label={rest['aria-label']}
      aria-roledescription="Pan and zoom canvas"
      tabIndex={0}
      data-panning={panning || undefined}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerEnd}
      onPointerCancel={onPointerEnd}
      onKeyDown={onKeyDown}
    >
      <svg className={styles.scene} width="100%" height="100%">
        {grid && transform.k >= GRID_MIN_ZOOM && (
          <>
            <defs>
              <pattern
                id={gridId}
                width={GRID_SPACING}
                height={GRID_SPACING}
                patternUnits="userSpaceOnUse"
                patternTransform={`translate(${transform.x} ${transform.y}) scale(${transform.k})`}
              >
                {/* The dots keep their size on screen at every zoom. */}
                <circle className={styles.gridDot} cx="1" cy="1" r={Math.min(1, 1 / transform.k)} />
              </pattern>
            </defs>
            <rect width="100%" height="100%" fill={`url(#${gridId})`} />
          </>
        )}
        <g ref={contentRef} transform={`translate(${transform.x} ${transform.y}) scale(${transform.k})`}>
          <ViewportContext value={api}>{children}</ViewportContext>
        </g>
      </svg>

      {controls && (
        <div className={styles.controls} data-viewport-controls>
          <IconButton icon={Minus} size="sm" aria-label="Zoom out" tooltipPlacement="top" onClick={() => zoomBy(1 / KEY_ZOOM)} />
          <button
            type="button"
            className={styles.zoom}
            aria-label={`Zoom ${zoomLabel}. Reset to 100%`}
            onClick={() => zoomBy(1 / transform.k)}
          >
            {zoomLabel}
          </button>
          <IconButton icon={Plus} size="sm" aria-label="Zoom in" tooltipPlacement="top" onClick={() => zoomBy(KEY_ZOOM)} />
          <span className={styles.divider} />
          <IconButton icon={Scan} size="sm" aria-label="Fit to view" tooltipPlacement="top-end" onClick={() => fit()} />
        </div>
      )}
    </div>
  );
}
