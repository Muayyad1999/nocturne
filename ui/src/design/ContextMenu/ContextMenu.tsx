import { cloneElement, useState, type MouseEvent, type ReactElement } from 'react';
import { MenuPopup, type MenuEntry } from '../Menu';
import type { AnchorPoint } from '../overlay/useFloating';

interface TargetProps {
  onContextMenu?: (event: MouseEvent<HTMLElement>) => void;
}

export interface ContextMenuProps {
  /** The element whose right-click (or Shift+F10 / Menu key) opens the menu. */
  children: ReactElement<TargetProps>;
  /** The actions. An empty list shows no menu and leaves the browser's own alone. */
  items: readonly MenuEntry[];
  /** Names the menu for assistive technology. */
  'aria-label': string;
}

/** Where to open for a context-menu event, including one raised from the keyboard. */
export function contextMenuPoint(event: MouseEvent<HTMLElement>): AnchorPoint {
  if (event.clientX !== 0 || event.clientY !== 0) return { x: event.clientX, y: event.clientY };
  // Keyboard-invoked events may carry no position: open at the element itself.
  const rect = event.currentTarget.getBoundingClientRect();
  return { x: rect.left + Math.min(24, rect.width / 2), y: rect.top + rect.height / 2 };
}

/**
 * Adds a context menu to an element. The same actions must also be reachable
 * another way (a row's actions button, the toolbar): a context menu is a
 * shortcut, never the only path.
 */
export function ContextMenu({ children, items, ...rest }: ContextMenuProps) {
  const [point, setPoint] = useState<AnchorPoint | null>(null);

  return (
    <>
      {cloneElement(children, {
        onContextMenu: (event) => {
          children.props.onContextMenu?.(event);
          if (event.defaultPrevented || items.length === 0) return;
          event.preventDefault();
          setPoint(contextMenuPoint(event));
        },
      })}
      {point && (
        <MenuPopup anchor={point} items={items} onClose={() => setPoint(null)} aria-label={rest['aria-label']} />
      )}
    </>
  );
}
