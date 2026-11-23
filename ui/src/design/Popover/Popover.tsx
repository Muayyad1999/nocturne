import {
  cloneElement,
  useId,
  useLayoutEffect,
  useRef,
  type MouseEvent,
  type ReactElement,
  type ReactNode,
  type Ref,
} from 'react';
import { cx } from '@/lib/cx';
import { useMergedRefs } from '@/lib/refs';
import { focusableWithin, useFocusScope } from '../overlay/focus';
import { useLayer } from '../overlay/layers';
import { Portal } from '../overlay/Portal';
import type { Placement } from '../overlay/position';
import { useControllableState } from '../overlay/useControllableState';
import { useFloating } from '../overlay/useFloating';
import styles from './Popover.module.css';

interface TriggerProps {
  ref?: Ref<HTMLElement>;
  onClick?: (event: MouseEvent<HTMLElement>) => void;
  'aria-haspopup'?: 'dialog';
  'aria-expanded'?: boolean;
  'aria-controls'?: string;
}

export interface PopoverProps {
  /** The element that opens the popover, usually a Button or IconButton. */
  trigger: ReactElement<TriggerProps>;
  /** The content, or a function that receives `close` for content that dismisses itself. */
  children: ReactNode | ((close: () => void) => ReactNode);
  /** Names the panel for assistive technology. */
  'aria-label': string;
  /** Controlled open state. Leave it out to let the popover manage itself. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  placement?: Placement;
  className?: string;
}

/**
 * A floating panel anchored to its trigger, for content that is too much for
 * a menu and too little for a dialog: filters, a short form, a list.
 * Escape or a press outside closes it and returns focus to the trigger.
 */
export function Popover({
  trigger,
  children,
  open: controlledOpen,
  onOpenChange,
  placement = 'bottom-start',
  className,
  ...rest
}: PopoverProps) {
  const [open, setOpen] = useControllableState(controlledOpen, false, onOpenChange);
  const triggerRef = useRef<HTMLElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const id = useId();
  const close = () => setOpen(false);

  const { style, side } = useFloating(open, triggerRef, panelRef, { placement });
  useLayer(open, panelRef, { onEscape: close, onPressOutside: close, trigger: triggerRef });
  useFocusScope(open, panelRef);

  // The panel mounts hidden until it has been measured, and a hidden element
  // refuses the focus: the scope's attempt in that first moment did nothing,
  // the focus stayed on the trigger, and Tab went on along the page instead
  // of into the panel. So it is given again once the panel is placed, as the
  // menu does (MenuPopup); the scope still remembers the trigger to give it
  // back to.
  const placed = open && style.visibility !== 'hidden';
  useLayoutEffect(() => {
    const panel = panelRef.current;
    if (!placed || !panel || panel.contains(document.activeElement)) return;
    const target = panel.querySelector<HTMLElement>('[data-autofocus]') ?? focusableWithin(panel)[0] ?? panel;
    target.focus({ preventScroll: true });
  }, [placed]);

  const ref = useMergedRefs(triggerRef, trigger.props.ref);

  return (
    <>
      {cloneElement(trigger, {
        ref,
        'aria-haspopup': 'dialog',
        'aria-expanded': open,
        'aria-controls': open ? id : undefined,
        onClick: (event) => {
          trigger.props.onClick?.(event);
          setOpen(!open);
        },
      })}
      {open && (
        <Portal>
          <div
            ref={panelRef}
            id={id}
            role="dialog"
            aria-label={rest['aria-label']}
            tabIndex={-1}
            className={cx(styles.popover, className)}
            style={style}
            data-side={side}
          >
            {typeof children === 'function' ? children(close) : children}
          </div>
        </Portal>
      )}
    </>
  );
}
