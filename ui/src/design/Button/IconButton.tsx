import type { ComponentProps } from 'react';
import type { LucideIcon } from '../Icon';
import type { Placement } from '../overlay/position';
import { Tooltip } from '../Tooltip';
import { Button, type ButtonSize, type ButtonVariant } from './Button';

export interface IconButtonProps extends Omit<ComponentProps<'button'>, 'children' | 'aria-label'> {
  icon: LucideIcon;
  /** Required: an icon alone says nothing to a screen reader. */
  'aria-label': string;
  variant?: ButtonVariant;
  size?: ButtonSize;
  loading?: boolean;
  /**
   * The label is shown as a tooltip on hover and keyboard focus. Pass other
   * text to say more, or `false` where a tooltip would be in the way.
   */
  tooltip?: string | false;
  tooltipPlacement?: Placement;
}

/** A button that is only an icon. */
export function IconButton({
  variant = 'ghost',
  tooltip,
  tooltipPlacement,
  ...rest
}: IconButtonProps) {
  const button = <Button {...rest} variant={variant} data-icon-only />;
  if (tooltip === false) return button;
  return (
    // By default the tooltip repeats the accessible name for sighted users, so
    // it is not announced a second time as a description.
    <Tooltip
      content={tooltip ?? rest['aria-label']}
      placement={tooltipPlacement}
      describe={tooltip !== undefined}
    >
      {button}
    </Tooltip>
  );
}
