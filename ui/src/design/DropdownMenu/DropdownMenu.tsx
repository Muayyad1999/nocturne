import {
  cloneElement,
  useId,
  useRef,
  type KeyboardEvent,
  type MouseEvent,
  type ReactElement,
  type Ref,
} from 'react';
import { useMergedRefs } from '@/lib/refs';
import { MenuPopup, type MenuEntry } from '../Menu';
import type { Placement } from '../overlay/position';
import { useControllableState } from '../overlay/useControllableState';

interface TriggerProps {
  ref?: Ref<HTMLElement>;
  onClick?: (event: MouseEvent<HTMLElement>) => void;
  onKeyDown?: (event: KeyboardEvent<HTMLElement>) => void;
  'aria-haspopup'?: 'menu';
  'aria-expanded'?: boolean;
  'aria-controls'?: string;
}

export interface DropdownMenuProps {
  /** The element that opens the menu, usually a Button or IconButton. */
  trigger: ReactElement<TriggerProps>;
  items: readonly MenuEntry[];
  /** Names the menu for assistive technology: "Row actions", "Columns". */
  'aria-label': string;
  placement?: Placement;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}

/** A menu of actions that drops from a button. */
export function DropdownMenu({
  trigger,
  items,
  placement,
  open: controlledOpen,
  onOpenChange,
  ...rest
}: DropdownMenuProps) {
  const [open, setOpen] = useControllableState(controlledOpen, false, onOpenChange);
  const triggerRef = useRef<HTMLElement>(null);
  const ref = useMergedRefs(triggerRef, trigger.props.ref);
  const id = useId();

  return (
    <>
      {cloneElement(trigger, {
        ref,
        'aria-haspopup': 'menu',
        'aria-expanded': open,
        'aria-controls': open ? id : undefined,
        onClick: (event) => {
          trigger.props.onClick?.(event);
          setOpen(!open);
        },
        onKeyDown: (event) => {
          trigger.props.onKeyDown?.(event);
          if (event.key === 'ArrowDown' && !open) {
            event.preventDefault();
            setOpen(true);
          }
        },
      })}
      {open && (
        <MenuPopup
          id={id}
          anchor={triggerRef}
          items={items}
          placement={placement}
          onClose={() => setOpen(false)}
          aria-label={rest['aria-label']}
        />
      )}
    </>
  );
}
