import type { ComponentProps } from 'react';

export interface LogoMarkProps extends Omit<ComponentProps<'svg'>, 'children'> {
  /** Width and height in pixels. */
  size?: number;
  /** Accessible name. Leave it out when the wordmark is next to the mark. */
  title?: string;
}

/**
 * The Nocturne mark: a crescent moon drawn as a small constellation of nodes
 * and links, the way a network's topology reads as a star map. It takes its
 * colour from `currentColor`.
 */
export function LogoMark({ size = 24, title, ...rest }: LogoMarkProps) {
  return (
    <svg
      {...rest}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      role={title ? 'img' : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : true}
    >
      {/* The body of the moon, barely there, so the shape holds at small sizes. */}
      <path
        d="M13.1 3 6.5 5.8 4 12.5 7.2 18.9 14.1 20.9 20.3 17.3 14.3 17.5 9.8 13.6 9.4 7.6Z"
        fill="currentColor"
        opacity="0.16"
      />
      {/* Links: the outline of the crescent and two paths across it. */}
      <path
        d="M13.1 3 6.5 5.8 4 12.5 7.2 18.9 14.1 20.9 20.3 17.3 14.3 17.5 9.8 13.6 9.4 7.6ZM9.8 13.6 4 12.5M14.3 17.5 7.2 18.9"
        stroke="currentColor"
        strokeWidth="1.25"
        strokeLinecap="round"
        strokeLinejoin="round"
        opacity="0.7"
      />
      {/* Nodes: brighter along the outer limb. */}
      <g fill="currentColor">
        <circle cx="6.5" cy="5.8" r="1.6" />
        <circle cx="4" cy="12.5" r="1.6" />
        <circle cx="7.2" cy="18.9" r="1.6" />
        <circle cx="14.1" cy="20.9" r="1.6" />
        <circle cx="13.1" cy="3" r="1.25" />
        <circle cx="20.3" cy="17.3" r="1.25" />
        <circle cx="9.4" cy="7.6" r="1.25" />
        <circle cx="9.8" cy="13.6" r="1.25" />
        <circle cx="14.3" cy="17.5" r="1.25" />
      </g>
    </svg>
  );
}
