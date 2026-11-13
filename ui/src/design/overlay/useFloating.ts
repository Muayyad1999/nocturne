import { useLayoutEffect, useState, type CSSProperties, type RefObject } from 'react';
import { computePosition, type Placement, type Rect, type Side } from './position';

/** A fixed point in viewport coordinates, for context menus. */
export interface AnchorPoint {
  x: number;
  y: number;
}

export interface FloatingOptions {
  placement?: Placement;
  offset?: number;
  /** Make the floating box at least as wide as the anchor (list boxes). */
  matchWidth?: boolean;
}

/**
 * Inline style for the floating element. The room left on the chosen side is
 * exposed as `--available-width` / `--available-height` so a component can cap
 * its own size (`max-height: var(--available-height)`) and scroll inside.
 */
export type FloatingStyle = CSSProperties & {
  '--available-width'?: string;
  '--available-height'?: string;
};

export interface Floating {
  /** Spread on the floating element. Hidden until the first measurement. */
  style: FloatingStyle;
  /** The side in use after flipping, for the entrance animation's origin. */
  side: Side;
}

const UNMEASURED: Floating = {
  style: { position: 'fixed', left: 0, top: 0, visibility: 'hidden' },
  side: 'bottom',
};

function isRef(anchor: RefObject<Element | null> | AnchorPoint): anchor is RefObject<Element | null> {
  return 'current' in anchor;
}

/**
 * Keeps a floating element positioned next to its anchor while `open`,
 * following scrolling, resizing and changes in its own size.
 */
export function useFloating(
  open: boolean,
  anchor: RefObject<Element | null> | AnchorPoint,
  floatingRef: RefObject<HTMLElement | null>,
  { placement = 'bottom-start', offset, matchWidth = false }: FloatingOptions = {},
): Floating {
  const [floating, setFloating] = useState<Floating>(UNMEASURED);

  const anchorRef = isRef(anchor) ? anchor : null;
  const pointX = isRef(anchor) ? 0 : anchor.x;
  const pointY = isRef(anchor) ? 0 : anchor.y;

  useLayoutEffect(() => {
    const element = floatingRef.current;
    if (!open || !element) {
      setFloating(UNMEASURED);
      return;
    }

    const update = () => {
      let rect: Rect = { x: pointX, y: pointY, width: 0, height: 0 };
      if (anchorRef) {
        const box = anchorRef.current?.getBoundingClientRect();
        if (!box) return;
        rect = { x: box.left, y: box.top, width: box.width, height: box.height };
      }
      const minWidth = matchWidth ? rect.width : undefined;
      // Apply before measuring: the width decides where the box fits.
      if (minWidth !== undefined) element.style.minWidth = `${minWidth}px`;

      const position = computePosition({
        anchor: rect,
        floating: { width: element.offsetWidth, height: element.offsetHeight },
        viewport: { width: window.innerWidth, height: window.innerHeight },
        placement,
        offset,
      });
      const style: FloatingStyle = {
        position: 'fixed',
        left: position.x,
        top: position.y,
        minWidth,
        '--available-width': `${position.maxWidth}px`,
        '--available-height': `${position.maxHeight}px`,
      };
      setFloating((previous) => {
        const unchanged =
          previous.side === position.side &&
          previous.style.left === style.left &&
          previous.style.top === style.top &&
          previous.style.minWidth === style.minWidth &&
          previous.style['--available-width'] === style['--available-width'] &&
          previous.style['--available-height'] === style['--available-height'];
        return unchanged ? previous : { style, side: position.side };
      });
    };

    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    if (anchorRef?.current) observer.observe(anchorRef.current);
    window.addEventListener('resize', update);
    // Capture: scrolling any ancestor moves the anchor.
    window.addEventListener('scroll', update, { capture: true, passive: true });
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', update);
      window.removeEventListener('scroll', update, { capture: true });
    };
  }, [open, anchorRef, pointX, pointY, floatingRef, placement, offset, matchWidth]);

  return floating;
}
