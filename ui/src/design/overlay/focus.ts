/**
 * Focus management for floating layers: move focus in when a layer opens,
 * keep Tab inside it, and give focus back to where it came from on close.
 */

import { useEffect, useInsertionEffect, useLayoutEffect, useRef, type RefObject } from 'react';
import { isEditableTarget } from '@/lib/keyboard';

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

/** The element that has the focus, or null when only the page has it. */
function focusedElement(): HTMLElement | null {
  const active = document.activeElement;
  return active instanceof HTMLElement && active !== document.body ? active : null;
}

/** The focus is on the page itself: where a browser drops it when the element that had it is removed. */
function focusDropped(): boolean {
  const active = document.activeElement;
  return active === null || active === document.body;
}

/** `element`'s ancestors, nearest first, up to `last` (included) or the top. */
function ancestorsOf(element: Element, last?: Element): Element[] {
  const path: Element[] = [];
  for (let node = element.parentElement; node; node = node.parentElement) {
    path.push(node);
    if (node === last) break;
  }
  return path;
}

/**
 * The page's main area and a layer. A focus that has lost its element is not
 * sent past one of these to whatever control comes first further up: the
 * region takes it itself (both accept a programmatic focus and draw no ring
 * for it), and Tab goes on from there.
 */
const BOUNDARY = 'body, main, [role="dialog"], [role="alertdialog"]';

/**
 * Gives the focus to what is left where an element that has gone used to be,
 * given that element's ancestors as they were: the first control in the
 * nearest one still on the page, else that region itself. A text field is
 * passed over (the next key would be typed into it) and so is anything inert.
 * Says whether the focus went anywhere.
 */
function focusNear(path: readonly Element[]): boolean {
  for (const ancestor of path) {
    if (!ancestor.isConnected) continue;
    if (!(ancestor instanceof HTMLElement) || ancestor.closest('[inert]')) return false;
    if (ancestor.matches(BOUNDARY)) {
      if (!ancestor.hasAttribute('tabindex')) return false;
      ancestor.focus({ preventScroll: true });
      return document.activeElement === ancestor;
    }
    const control = focusableWithin(ancestor).find((element) => !isEditableTarget(element));
    if (control) {
      control.focus({ preventScroll: true });
      return document.activeElement === control;
    }
  }
  return false;
}

/** Ends the one hand-back still looked after, if there is one. */
let stopWatching: (() => void) | null = null;

/**
 * Looks after a focus given back to `origin` until the user's next move. The
 * action a layer confirmed often makes the control that opened it obsolete —
 * an issued draft has no "Issue…", a suspended customer no "Mark suspended" —
 * and the screen removes it when it redraws, after the hand-back. The
 * browser then drops the focus on the page, and the next Tab starts at the
 * top of it; here it goes to what is left near `origin` instead.
 */
function watchHandBack(origin: HTMLElement): void {
  stopWatching?.();
  const path = ancestorsOf(origin);
  const observer = new MutationObserver(() => {
    if (origin.isConnected) return;
    stop();
    if (focusDropped()) focusNear(path);
  });
  const onFocusIn = (event: FocusEvent) => {
    if (event.target !== origin) stop();
  };
  function stop(): void {
    observer.disconnect();
    document.removeEventListener('keydown', stop, true);
    document.removeEventListener('pointerdown', stop, true);
    document.removeEventListener('focusin', onFocusIn, true);
    if (stopWatching === stop) stopWatching = null;
  }
  observer.observe(document.body, { childList: true, subtree: true });
  // In the capture phase: the commit that gave the focus back can run while
  // the key that closed the layer is still on its way up, and that key is
  // not the user's next move.
  document.addEventListener('keydown', stop, true);
  document.addEventListener('pointerdown', stop, true);
  document.addEventListener('focusin', onFocusIn, true);
  stopWatching = stop;
}

/**
 * Gives the focus back to `origin`, or, when it has gone in the meantime, to
 * what is left where it was (`path`, its ancestors when the scope began).
 */
function giveBack(origin: HTMLElement | null, path: readonly Element[]): void {
  if (!origin) return;
  if (!origin.isConnected) {
    focusNear(path);
    return;
  }
  origin.focus({ preventScroll: true });
  if (document.activeElement === origin) watchHandBack(origin);
}

/** A focus move seen while a scope was starting. */
interface Move {
  to: HTMLElement | null;
  from: HTMLElement | null;
}

/** What a scope saw of the focus between the start of its commit and its first look. */
interface Start {
  /** What had the focus when the commit began. */
  first: HTMLElement | null;
  moves: Move[];
  stop: () => void;
}

/**
 * Where the focus came from, decided when the scope first looks. Normally
 * that is whatever has it at that moment: every other layer that closed in
 * the same commit (a menu that opened this dialog) has given its focus back
 * by then. But a field with React's `autoFocus` has already taken it: React
 * focuses it while committing the layer, before the layer's own effects run.
 * Then it is wherever the focus was before it moved into the layer.
 */
function cameFrom(container: HTMLElement, start: Start): HTMLElement | null {
  const current = focusedElement();
  if (!current || !container.contains(current)) return current;
  const entered = start.moves.find((move) => move.to !== null && container.contains(move.to));
  const from = entered ? entered.from : start.first;
  return from && from.isConnected && !container.contains(from) ? from : null;
}

export interface FocusScopeOptions {
  /** Keep Tab cycling inside the container. */
  trap?: boolean;
  /** Return focus to the previously focused element when the scope ends. */
  restore?: boolean;
  /**
   * Where focus goes first: the element marked `data-autofocus`, else the
   * first focusable element, else the container itself. A field that takes
   * the focus with React's `autoFocus` keeps it.
   */
  autoFocus?: boolean;
}

export function useFocusScope(
  active: boolean,
  ref: RefObject<HTMLElement | null>,
  { trap = true, restore = true, autoFocus = true }: FocusScopeOptions = {},
): void {
  /** The element to give the focus back to, and its ancestors when the scope began. */
  const origin = useRef<{ element: HTMLElement | null; path: Element[] }>({ element: null, path: [] });
  const start = useRef<Start | null>(null);
  /**
   * The element inside that had the focus when a cleanup took it away while
   * the layer stayed: StrictMode unmounts and mounts every new layer once in
   * development. It is given the focus again, rather than the first control,
   * because React does not repeat an `autoFocus` on that second mount.
   */
  const resume = useRef<HTMLElement | null>(null);

  // An insertion effect runs before the layout phase of its commit, which is
  // where React focuses an `autoFocus` field: the only point at which where
  // the focus was can still be seen. Nothing is decided here (another layer
  // may hand its focus back later in this commit); the moves are noted for
  // the layout effect below.
  useInsertionEffect(() => {
    if (!active) return;
    const first = focusedElement();
    const moves: Move[] = [];
    let last = first;
    const onFocusIn = (event: FocusEvent) => {
      const to = event.target instanceof HTMLElement ? event.target : null;
      const from = event.relatedTarget instanceof HTMLElement ? event.relatedTarget : last;
      moves.push({ to, from });
      last = to;
    };
    document.addEventListener('focusin', onFocusIn, true);
    const stop = () => document.removeEventListener('focusin', onFocusIn, true);
    start.current = { first, moves, stop };
    return stop;
  }, [active]);

  useLayoutEffect(() => {
    const container = ref.current;
    if (!active || !container) return;

    // The first look since the scope began. When there is nothing to look at,
    // this is StrictMode's second mount or an option that changed: the
    // origin is already known, and the focus is no longer where it came from.
    const started = start.current;
    if (started) {
      started.stop();
      start.current = null;
      resume.current = null;
      const element = cameFrom(container, started);
      origin.current = { element, path: element ? ancestorsOf(element) : [] };
    }

    if (autoFocus && !container.contains(document.activeElement)) {
      const back = resume.current;
      const target =
        (back?.isConnected && container.contains(back) ? back : null) ??
        container.querySelector<HTMLElement>('[data-autofocus]') ??
        focusableWithin(container)[0] ??
        container;
      target.focus({ preventScroll: true });
    }
    resume.current = null;

    return () => {
      const current = focusedElement();
      const inside = current && container.contains(current) ? current : null;
      resume.current = inside;
      // Only pull focus back if it is still inside the layer that is closing
      // (or nowhere): the user may already have moved on.
      if (restore && (inside || focusDropped())) giveBack(origin.current.element, origin.current.path);
    };
  }, [active, ref, restore, autoFocus]);

  // Under a modal layer the page is inert, and an inert element refuses the
  // focus. The layer stack lifts that in its own layout cleanup, which runs
  // before the one above when the layer is registered first, as every layer
  // here is. Where it is not, or where the focus fell to the body because the
  // layer's element went first, the hand-back above did nothing and the next
  // Tab would start at the top of the page: this passive cleanup runs after
  // every layout one, and makes the hand-back if it is still owed.
  useEffect(() => {
    if (!active || !restore) return;
    return () => {
      if (focusDropped()) giveBack(origin.current.element, origin.current.path);
    };
  }, [active, restore]);

  // Layout, like the layer itself: a Tab pressed the instant the layer
  // appeared would otherwise leave it before the trap was set.
  useLayoutEffect(() => {
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

    // A layer keeps the focus when the control that had it is removed while
    // it stays open (a step that replaces its button, a draft that is issued
    // and loses its "Issue…"). The browser drops it on the page, under the
    // layer, where Tab reaches nothing: it goes to what is left nearby, or to
    // the layer. A screen that moves it on purpose does so in its own commit,
    // before this runs, and is left alone.
    const current = focusedElement();
    let holder = current && container.contains(current) ? current : null;
    let holderPath = holder ? ancestorsOf(holder, container) : [];
    const onFocusIn = (event: FocusEvent) => {
      if (!(event.target instanceof HTMLElement)) return;
      holder = event.target;
      holderPath = ancestorsOf(holder, container);
    };
    const observer = new MutationObserver(() => {
      if (!holder || holder.isConnected || !focusDropped() || container.closest('[inert]')) return;
      holder = null;
      if (!focusNear(holderPath)) container.focus({ preventScroll: true });
    });

    container.addEventListener('keydown', onKeyDown);
    container.addEventListener('focusin', onFocusIn);
    observer.observe(container, { childList: true, subtree: true });
    return () => {
      container.removeEventListener('keydown', onKeyDown);
      container.removeEventListener('focusin', onFocusIn);
      observer.disconnect();
    };
  }, [active, ref, trap]);
}
