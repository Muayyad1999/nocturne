import {
  cloneElement,
  useEffect,
  useId,
  useRef,
  useState,
  type FocusEvent,
  type PointerEvent,
  type ReactElement,
  type ReactNode,
  type Ref,
} from 'react';
import { useMergedRefs } from '@/lib/refs';
import { Portal } from '../overlay/Portal';
import type { Placement } from '../overlay/position';
import { useFloating } from '../overlay/useFloating';
import styles from './Tooltip.module.css';

/** What the tooltip needs to attach to its trigger. Any element or component that forwards these works. */
interface TriggerProps {
  ref?: Ref<HTMLElement>;
  'aria-describedby'?: string;
  onPointerEnter?: (event: PointerEvent<HTMLElement>) => void;
  onPointerLeave?: (event: PointerEvent<HTMLElement>) => void;
  onPointerDown?: (event: PointerEvent<HTMLElement>) => void;
  onFocus?: (event: FocusEvent<HTMLElement>) => void;
  onBlur?: (event: FocusEvent<HTMLElement>) => void;
}

export interface TooltipProps {
  /** Short supplementary text. Never the only place something can be learned. */
  content: ReactNode;
  /** The element the tooltip describes. */
  children: ReactElement<TriggerProps>;
  placement?: Placement;
  /**
   * Announce the content as the trigger's description. Turn off when the
   * content repeats the trigger's accessible name.
   */
  describe?: boolean;
  disabled?: boolean;
}

const SHOW_DELAY_MS = 450;
/** Moving from one trigger to the next within this window skips the delay. */
const WARM_WINDOW_MS = 350;
let lastHiddenAt = 0;

/** Shows a hint on hover and on keyboard focus; Escape or leaving hides it. */
export function Tooltip({
  content,
  children,
  placement = 'top',
  describe = true,
  disabled = false,
}: TooltipProps) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLElement>(null);
  const tooltipRef = useRef<HTMLDivElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const id = useId();
  const visible = open && !disabled;
  const { style, side } = useFloating(visible, triggerRef, tooltipRef, { placement, offset: 6 });

  const show = (immediately: boolean) => {
    clearTimeout(timer.current);
    if (immediately || performance.now() - lastHiddenAt < WARM_WINDOW_MS) setOpen(true);
    else timer.current = setTimeout(() => setOpen(true), SHOW_DELAY_MS);
  };

  const hide = () => {
    clearTimeout(timer.current);
    setOpen((wasOpen) => {
      if (wasOpen) lastHiddenAt = performance.now();
      return false;
    });
  };

  useEffect(() => () => clearTimeout(timer.current), []);

  useEffect(() => {
    if (!visible) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') hide();
    };
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [visible]);

  const trigger = children.props;
  const ref = useMergedRefs(triggerRef, trigger.ref);

  return (
    <>
      {cloneElement(children, {
        ref,
        'aria-describedby': visible && describe ? id : trigger['aria-describedby'],
        onPointerEnter: (event) => {
          trigger.onPointerEnter?.(event);
          if (event.pointerType !== 'touch') show(false);
        },
        onPointerLeave: (event) => {
          trigger.onPointerLeave?.(event);
          hide();
        },
        onPointerDown: (event) => {
          trigger.onPointerDown?.(event);
          hide();
        },
        onFocus: (event) => {
          trigger.onFocus?.(event);
          if (event.currentTarget.matches(':focus-visible')) show(true);
        },
        onBlur: (event) => {
          trigger.onBlur?.(event);
          hide();
        },
      })}
      {visible && (
        <Portal>
          <div ref={tooltipRef} id={id} role="tooltip" className={styles.tooltip} style={style} data-side={side}>
            {content}
          </div>
        </Portal>
      )}
    </>
  );
}
