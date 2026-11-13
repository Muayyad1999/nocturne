import { useState, type ReactNode } from 'react';
import { cx } from '@/lib/cx';
import { readStored, storageKey, writeStored } from '@/lib/storage';
import { ResizeHandle } from './ResizeHandle';
import styles from './Splitter.module.css';

export interface SplitterProps {
  /** `horizontal`: panes side by side. `vertical`: panes stacked. */
  orientation?: 'horizontal' | 'vertical';
  /** Exactly two panes. */
  children: [ReactNode, ReactNode];
  /** The pane with a set size; the other one takes the remaining space. */
  sized?: 'first' | 'second';
  /** Initial size of the sized pane, in pixels. */
  defaultSize?: number;
  minSize?: number;
  maxSize?: number;
  /** Remembers the size under this id (`localStorage`), e.g. `devices.detail`. */
  storageId?: string;
  /** Names the handle for assistive technology. */
  handleLabel?: string;
  className?: string;
}

function isSize(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

/** Two panes with a draggable, keyboard-operable separator between them. */
export function Splitter({
  orientation = 'horizontal',
  children,
  sized = 'first',
  defaultSize = 280,
  minSize = 160,
  maxSize = 640,
  storageId,
  handleLabel = 'Resize panes',
  className,
}: SplitterProps) {
  const key = storageId ? storageKey(`splitter.${storageId}`) : null;
  const [size, setSize] = useState(() => {
    const stored = key ? readStored(key, isSize) : undefined;
    return Math.min(maxSize, Math.max(minSize, stored ?? defaultSize));
  });

  const [first, second] = children;
  const dimension = orientation === 'horizontal' ? 'width' : 'height';

  return (
    <div className={cx(styles.splitter, className)} data-orientation={orientation}>
      <div
        className={styles.pane}
        data-sized={sized === 'first' || undefined}
        style={sized === 'first' ? { [dimension]: size } : undefined}
      >
        {first}
      </div>
      <ResizeHandle
        orientation={orientation === 'horizontal' ? 'vertical' : 'horizontal'}
        value={size}
        min={minSize}
        max={maxSize}
        defaultValue={defaultSize}
        reverse={sized === 'second'}
        onChange={setSize}
        onChangeEnd={(next) => {
          if (key) writeStored(key, next);
        }}
        aria-label={handleLabel}
      />
      <div
        className={styles.pane}
        data-sized={sized === 'second' || undefined}
        style={sized === 'second' ? { [dimension]: size } : undefined}
      >
        {second}
      </div>
    </div>
  );
}
