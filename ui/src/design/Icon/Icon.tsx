import type { LucideIcon } from 'lucide-react';

export type { LucideIcon } from 'lucide-react';

export interface IconProps {
  icon: LucideIcon;
  /** Size in pixels. 16 suits text and controls; 20 and up suits headings and empty states. */
  size?: number;
  /**
   * Accessible name. Leave it out when the icon sits next to text that says
   * the same thing: the icon is then hidden from assistive technology.
   */
  label?: string;
  className?: string;
}

/** A Lucide icon at the system's sizes and stroke weight. */
export function Icon({ icon: Glyph, size = 16, label, className }: IconProps) {
  return (
    <Glyph
      size={size}
      strokeWidth={size >= 20 ? 1.5 : 1.75}
      className={className}
      role={label ? 'img' : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    />
  );
}
