/**
 * Where the focus goes when a drawer changes what it shows without closing.
 *
 * The snippets and logs drawers show a list, then one item, then the list
 * again. The control that was pressed to get from one to the other is gone
 * by then, and a browser drops the focus of a removed element on the page
 * itself. A layer's own focus handling acts when the layer opens and closes,
 * not in between, so the next Tab would start at the top of the page — under
 * the drawer, where nothing can be reached.
 */

import { useLayoutEffect, type RefObject } from 'react';
import { focusableWithin } from '@/design/overlay/focus';

/**
 * Gives the focus to a view when it appears and the focus has been dropped:
 * to its `data-autofocus` element, or else to the first control in it.
 *
 * Only when it was dropped. When a drawer opens, the focus is still on the
 * button that opened it, and the drawer's own handling moves it in and
 * remembers where to give it back; taking it here first would leave the
 * drawer nothing to give it back to.
 *
 * `view` names what is shown, for a component that draws several views
 * itself: the focus is looked after again each time it changes.
 */
export function useFocusWhenShown(ref: RefObject<HTMLElement | null>, view?: string): void {
  useLayoutEffect(() => {
    const shown = ref.current;
    if (!shown) return;
    const active = document.activeElement;
    if (active !== null && active !== document.body) return;
    const marked = shown.querySelector<HTMLElement>('[data-autofocus]');
    // A marked control that cannot take the focus (a row with nowhere to send to) is passed over.
    const target = marked && !marked.matches(':disabled') ? marked : focusableWithin(shown)[0];
    target?.focus({ preventScroll: true });
  }, [ref, view]);
}
