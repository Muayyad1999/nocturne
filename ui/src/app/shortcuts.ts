/**
 * App-wide keyboard shortcuts: one window listener runs the registered
 * command whose shortcut matches.
 *
 * A shortcut with a modifier works everywhere, including text fields. A bare
 * key (`?`) is ignored while typing. A region that needs every key itself,
 * such as a terminal, opts out with `data-global-shortcuts="off"`.
 */

import { useEffect } from 'react';
import {
  allowsGlobalShortcuts,
  hasModifier,
  isEditableTarget,
  matchesShortcut,
  parseShortcut,
} from '@/lib/keyboard';
import { runCommand, useCommands } from './CommandPalette/commands';

export function useGlobalShortcuts(): void {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing) return;
      if (!allowsGlobalShortcuts(event.target)) return;
      const typing = isEditableTarget(event.target);

      for (const command of Object.values(useCommands.getState().commands)) {
        if (!command.shortcut) continue;
        const shortcut = parseShortcut(command.shortcut);
        if (typing && !hasModifier(shortcut)) continue;
        if (!matchesShortcut(event, shortcut)) continue;
        event.preventDefault();
        runCommand(command.id);
        return;
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);
}
