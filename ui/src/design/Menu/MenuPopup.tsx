import { useEffect, useRef, type RefObject } from 'react';
import { useFocusScope } from '../overlay/focus';
import { useLayer } from '../overlay/layers';
import { Portal } from '../overlay/Portal';
import type { Placement } from '../overlay/position';
import { useFloating, type AnchorPoint } from '../overlay/useFloating';
import { Menu, type MenuEntry } from './Menu';

export interface MenuPopupProps {
  /** The element the menu hangs from, or a point in the viewport (context menus). */
  anchor: RefObject<Element | null> | AnchorPoint;
  items: readonly MenuEntry[];
  onClose: () => void;
  'aria-label': string;
  placement?: Placement;
  id?: string;
}

/**
 * An open menu: positioned, dismissible, and returning focus to where it came
 * from. Render it while the menu should be open. DropdownMenu and ContextMenu
 * wrap it; use it directly when one menu serves many triggers (table rows).
 */
export function MenuPopup({ anchor, items, onClose, placement = 'bottom-start', ...rest }: MenuPopupProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  const isElement = 'current' in anchor;

  const { style } = useFloating(true, anchor, menuRef, { placement, offset: isElement ? 4 : 0 });
  useLayer(true, menuRef, {
    onEscape: onClose,
    onPressOutside: onClose,
    trigger: isElement ? anchor : undefined,
  });
  // The menu focuses its own first item; the scope only gives focus back.
  useFocusScope(true, menuRef, { trap: false, autoFocus: false });

  // … except that the menu mounts hidden, until it has been measured, and a
  // hidden element refuses the focus: the menu's own attempt, made in that
  // first moment, did nothing, the focus stayed on the trigger, and the arrow
  // keys never reached an item. So it is given again once the menu is placed.
  const placed = style.visibility !== 'hidden';
  useEffect(() => {
    const menu = menuRef.current;
    if (!placed || !menu || menu.contains(document.activeElement)) return;
    menu
      .querySelector<HTMLElement>('[role^="menuitem"]:not([aria-disabled="true"])')
      ?.focus({ preventScroll: true });
  }, [placed]);

  return (
    <Portal>
      <Menu {...rest} ref={menuRef} items={items} onClose={onClose} style={style} />
    </Portal>
  );
}
