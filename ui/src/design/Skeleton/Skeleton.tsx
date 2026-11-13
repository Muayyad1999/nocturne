import type { ComponentProps, CSSProperties } from 'react';
import { cx } from '@/lib/cx';
import styles from './Skeleton.module.css';

export interface SkeletonProps extends Omit<ComponentProps<'span'>, 'children'> {
  /** CSS width: a number is pixels. Defaults to filling the line. */
  width?: number | string;
  /** CSS height: a number is pixels. Defaults to one line of text. */
  height?: number | string;
  /** `text` is a rounded line, `block` a card-shaped area, `circle` an avatar or icon. */
  shape?: 'text' | 'block' | 'circle';
}

/**
 * A placeholder shaped like the content that is loading. Compose several to
 * mirror the real layout, and mark the region `aria-busy` while they show.
 */
export function Skeleton({ width, height, shape = 'text', className, style, ...rest }: SkeletonProps) {
  const size: CSSProperties = { width, height };
  return (
    <span
      {...rest}
      className={cx(styles.skeleton, className)}
      data-shape={shape}
      style={{ ...size, ...style }}
      aria-hidden="true"
    />
  );
}
