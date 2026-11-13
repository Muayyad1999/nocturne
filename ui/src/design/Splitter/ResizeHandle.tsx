import { useRef, type KeyboardEvent, type PointerEvent } from 'react';
import { cx } from '@/lib/cx';
import styles from './Splitter.module.css';

export interface ResizeHandleProps {
  /**
   * The direction of the bar itself: `vertical` separates side-by-side panes
   * and is dragged left and right; `horizontal` separates stacked panes.
   */
  orientation: 'vertical' | 'horizontal';
  /** Size of the pane being resized, in pixels. */
  value: number;
  min: number;
  max: number;
  /** Called continuously while dragging and on every key press. */
  onChange: (size: number) => void;
  /** Called once when a drag ends or after a key press: the moment to remember the size. */
  onChangeEnd?: (size: number) => void;
  /** The resized pane comes after the handle, so moving towards the start grows it. */
  reverse?: boolean;
  /** Double-click or Enter returns to this size. */
  defaultValue?: number;
  'aria-label': string;
  className?: string;
}

const KEY_STEP = 16;
const KEY_STEP_LARGE = 64;

/**
 * The draggable separator between two panes. Splitter uses it; so can any
 * layout that sizes a panel itself (the assistant dock does).
 */
export function ResizeHandle({
  orientation,
  value,
  min,
  max,
  onChange,
  onChangeEnd,
  reverse = false,
  defaultValue,
  className,
  ...rest
}: ResizeHandleProps) {
  const drag = useRef<{ start: number; value: number; latest: number } | null>(null);
  const clamp = (size: number) => Math.round(Math.min(max, Math.max(min, size)));
  const position = (event: PointerEvent) => (orientation === 'vertical' ? event.clientX : event.clientY);

  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    event.currentTarget.focus();
    drag.current = { start: position(event), value, latest: value };
    // Keep the resize cursor and stop text selection wherever the pointer goes.
    document.body.style.cursor = orientation === 'vertical' ? 'col-resize' : 'row-resize';
    document.body.style.userSelect = 'none';
  };

  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const current = drag.current;
    if (!current) return;
    const delta = (position(event) - current.start) * (reverse ? -1 : 1);
    const next = clamp(current.value + delta);
    if (next !== current.latest) {
      current.latest = next;
      onChange(next);
    }
  };

  const endDrag = () => {
    const current = drag.current;
    if (!current) return;
    drag.current = null;
    document.body.style.cursor = '';
    document.body.style.userSelect = '';
    onChangeEnd?.(current.latest);
  };

  const set = (size: number) => {
    const next = clamp(size);
    onChange(next);
    onChangeEnd?.(next);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const step = event.shiftKey ? KEY_STEP_LARGE : KEY_STEP;
    const [shrinkKey, growKey] = orientation === 'vertical' ? ['ArrowLeft', 'ArrowRight'] : ['ArrowUp', 'ArrowDown'];
    const direction = reverse ? -1 : 1;
    if (event.key === growKey) set(value + step * direction);
    else if (event.key === shrinkKey) set(value - step * direction);
    else if (event.key === 'Home') set(min);
    else if (event.key === 'End') set(max);
    else if (event.key === 'Enter' && defaultValue !== undefined) set(defaultValue);
    else return;
    event.preventDefault();
  };

  return (
    <div
      {...rest}
      className={cx(styles.handle, className)}
      role="separator"
      tabIndex={0}
      aria-orientation={orientation}
      aria-valuenow={value}
      aria-valuemin={min}
      aria-valuemax={max}
      data-orientation={orientation}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onLostPointerCapture={endDrag}
      onDoubleClick={defaultValue === undefined ? undefined : () => set(defaultValue)}
      onKeyDown={onKeyDown}
    />
  );
}
