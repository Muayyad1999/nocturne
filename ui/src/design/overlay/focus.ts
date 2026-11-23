/**
 * Focus management for floating layers: move focus in when a layer opens,
 * keep Tab inside it, and give focus back to where it came from on close.
 */

import { useEffect, useLayoutEffect, useRef, type RefObject } from 'react';

const FOCUSABLE = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
  '[contenteditable="true"]',
].join(',');

/** The elements Tab can reach inside `container`, in order. */
export function focusableWithin(container: HTMLElement): HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
    (element) => !element.closest('[inert]') && element.getClientRects().length > 0,
  );
}

export interface FocusScopeOptions {
  /** Keep Tab cycling inside the container. */
  trap?: boolean;
  /** Return focus to the previously focused element when the scope ends. */
  restore?: boolean;
  /**
   * Where focus goes first: the element marked `data-autofocus`, else the
   * first focusable element, else the container itself.
   */
  autoFocus?: boolean;
}

export function useFocusScope(
  active: boolean,
  ref: RefObject<HTMLElement | null>,
  { trap = true, restore = true, autoFocus = true }: FocusScopeOptions = {},
): void {
  const cameFrom = useRef<HTMLElement | null>(null);

  useLayoutEffect(() => {
    const container = ref.current;
    if (!active || !container) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    cameFrom.current = previous;

    if (autoFocus && !container.contains(document.activeElement)) {
      const target =
        container.querySelector<HTMLElement>('[data-autofocus]') ??
        focusableWithin(container)[0] ??
        container;
      target.focus({ preventScroll: true });
    }

    return () => {
      // Only pull focus back if it is still inside the layer that is closing
      // (or nowhere): the user may already have moved on.
      const current = document.activeElement;
      const lost = !current || current === document.body || container.contains(current);
      if (restore && lost && previous?.isConnected) previous.focus({ preventScroll: true });
    };
  }, [active, ref, restore, autoFocus]);

  // Under a modal layer the page is inert, and an inert element refuses the
  // focus. The layer stack lifts that in a passive cleanup, which runs after
  // the layout cleanup above — so for a dialog the hand-back there did
  // nothing, and the focus fell to the body: the next Tab started at the top
  // of the page. This cleanup runs after the stack's (a layer registers
  // before it scopes the focus), and makes the hand-back if it is still owed.
  useEffect(() => {
    if (!active || !restore) return;
    return () => {
      const previous = cameFrom.current;
      const current = document.activeElement;
      if (previous?.isConnected && (!current || current === document.body)) {
        previous.focus({ preventScroll: true });
      }
    };
  }, [active, restore]);

  useEffect(() => {
    const container = ref.current;
    if (!active || !trap || !container) return;

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Tab') return;
      const focusable = focusableWithin(container);
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (!first || !last) {
        event.preventDefault();
        container.focus();
        return;
      }
      const current = document.activeElement;
      if (event.shiftKey && (current === first || current === container)) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && current === last) {
        event.preventDefault();
        first.focus();
      }
    };
    container.addEventListener('keydown', onKeyDown);
    return () => container.removeEventListener('keydown', onKeyDown);
  }, [active, ref, trap]);
}
