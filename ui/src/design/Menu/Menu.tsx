import { Check } from 'lucide-react';
import {
  useEffect,
  useRef,
  type CSSProperties,
  type KeyboardEvent,
  type Ref,
} from 'react';
import { cx } from '@/lib/cx';
import { useMergedRefs } from '@/lib/refs';
import { Icon, type LucideIcon } from '../Icon';
import { Kbd } from '../Kbd';
import styles from './Menu.module.css';

export interface MenuAction {
  type?: 'item';
  label: string;
  icon?: LucideIcon;
  /** Shown at the right, in `lib/keyboard` notation. Display only: register a command to make it work. */
  shortcut?: string;
  /** `danger` for destructive actions. */
  tone?: 'danger';
  disabled?: boolean;
  /**
   * Makes the item a toggle: it shows a check mark when true and the menu
   * stays open when it is chosen, so several can be switched at once.
   */
  checked?: boolean;
  onSelect: () => void;
}

export type MenuEntry = MenuAction | { type: 'separator' } | { type: 'label'; label: string };

export interface MenuProps {
  items: readonly MenuEntry[];
  /** Called when the menu should close: an action was chosen, or Tab was pressed. */
  onClose: () => void;
  'aria-label': string;
  ref?: Ref<HTMLDivElement>;
  id?: string;
  style?: CSSProperties;
  className?: string;
}

const TYPEAHEAD_RESET_MS = 600;

/**
 * The menu surface: a list of actions with full keyboard control. Used by
 * DropdownMenu and ContextMenu, which add the trigger and the positioning.
 */
export function Menu({ items, onClose, ref, className, ...rest }: MenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  const mergedRef = useMergedRefs(menuRef, ref);
  const typeahead = useRef({ text: '', at: 0 });

  const enabledItems = () =>
    [...(menuRef.current?.querySelectorAll<HTMLElement>('[role^="menuitem"]:not([aria-disabled="true"])') ?? [])];

  // Focus the first action when the menu opens.
  useEffect(() => {
    enabledItems()[0]?.focus({ preventScroll: true });
  }, []);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const all = enabledItems();
    const index = all.indexOf(document.activeElement as HTMLElement);
    const focus = (next: number) => all[(next + all.length) % all.length]?.focus();

    switch (event.key) {
      case 'ArrowDown':
        focus(index + 1);
        break;
      case 'ArrowUp':
        focus(index < 0 ? all.length - 1 : index - 1);
        break;
      case 'Home':
        focus(0);
        break;
      case 'End':
        focus(all.length - 1);
        break;
      case 'Tab':
        onClose();
        break;
      default: {
        // Type a letter to jump to the next action that starts with it.
        if (event.key.length !== 1 || event.key === ' ') return;
        if (event.ctrlKey || event.metaKey || event.altKey) return;
        const now = performance.now();
        const state = typeahead.current;
        state.text = now - state.at > TYPEAHEAD_RESET_MS ? event.key : state.text + event.key;
        state.at = now;
        const query = state.text.toLowerCase();
        const rotated = [...all.slice(index + 1), ...all.slice(0, index + 1)];
        rotated.find((item) => item.textContent?.trim().toLowerCase().startsWith(query))?.focus();
      }
    }
    event.preventDefault();
  };

  return (
    <div
      {...rest}
      ref={mergedRef}
      role="menu"
      tabIndex={-1}
      className={cx(styles.menu, className)}
      onKeyDown={onKeyDown}
    >
      {items.map((entry, index) => {
        if (entry.type === 'separator') {
          return <div key={index} role="separator" className={styles.separator} />;
        }
        if (entry.type === 'label') {
          return (
            <div key={index} className={styles.label}>
              {entry.label}
            </div>
          );
        }
        const toggle = entry.checked !== undefined;
        const select = () => {
          if (entry.disabled) return;
          entry.onSelect();
          if (!toggle) onClose();
        };
        return (
          <div
            key={index}
            role={toggle ? 'menuitemcheckbox' : 'menuitem'}
            aria-checked={toggle ? entry.checked : undefined}
            aria-disabled={entry.disabled || undefined}
            tabIndex={-1}
            className={styles.item}
            data-tone={entry.tone === 'danger' ? 'crit' : undefined}
            onClick={select}
            onKeyDown={(event) => {
              if (event.key !== 'Enter' && event.key !== ' ') return;
              event.preventDefault();
              event.stopPropagation();
              select();
            }}
            // Hover and keyboard share one highlight: the focused item.
            onPointerMove={(event) => {
              if (!entry.disabled && document.activeElement !== event.currentTarget) {
                event.currentTarget.focus({ preventScroll: true });
              }
            }}
          >
            <span className={styles.leading}>
              {toggle
                ? entry.checked && <Icon icon={Check} size={14} />
                : entry.icon && <Icon icon={entry.icon} size={14} />}
            </span>
            <span className={styles.text}>{entry.label}</span>
            {entry.shortcut && <Kbd shortcut={entry.shortcut} className={styles.shortcut} />}
          </div>
        );
      })}
    </div>
  );
}
