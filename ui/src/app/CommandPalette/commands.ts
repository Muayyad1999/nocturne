/**
 * The command registry. Everything a user can do from the keyboard is a
 * command: it appears in the palette (Ctrl/⌘ K) and, if it declares a
 * shortcut, runs app-wide.
 *
 *   useEffect(() => registerCommand({
 *     id: 'devices.add', title: 'Add device', group: 'Devices',
 *     keywords: ['new', 'create'], shortcut: 'mod+shift+d', run: openAddDialog,
 *   }), []);
 */

import { create } from 'zustand';
import { toast } from '@/design/Toast';
import type { LucideIcon } from '@/design/Icon';
import { parseShortcut } from '@/lib/keyboard';
import { readStored, storageKey, writeStored } from '@/lib/storage';

export interface Command {
  /** Unique and stable, namespaced by slice: `devices.add`. */
  id: string;
  /** What the command does, as an imperative: "Add device". */
  title: string;
  /** The heading it is listed under: "Navigate", "View", a slice name. */
  group: string;
  /** Extra words that find it. */
  keywords?: string[];
  /** A shortcut in `lib/keyboard` notation (`mod+shift+d`). */
  shortcut?: string;
  icon?: LucideIcon;
  run: () => void | Promise<void>;
}

interface CommandsState {
  /** Registered commands by id, in registration order. */
  commands: Record<string, Command>;
  /** Ids of recently run commands, most recent first. */
  recent: string[];
}

const RECENT_KEY = storageKey('palette.recent');
const RECENT_LIMIT = 6;

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((entry) => typeof entry === 'string');
}

export const useCommands = create<CommandsState>(() => ({
  commands: {},
  recent: readStored(RECENT_KEY, isStringArray) ?? [],
}));

/** Adds a command. Returns a function that removes it: return that from a `useEffect`. */
export function registerCommand(command: Command): () => void {
  // A malformed shortcut fails here, where it was written, not on a key press.
  if (command.shortcut) parseShortcut(command.shortcut);
  useCommands.setState((state) => ({ commands: { ...state.commands, [command.id]: command } }));
  return () => {
    useCommands.setState((state) => {
      if (state.commands[command.id] !== command) return state;
      const { [command.id]: _removed, ...commands } = state.commands;
      return { commands };
    });
  };
}

/** Runs a command by id and remembers it as recent. A failure is reported, never swallowed. */
export function runCommand(id: string): void {
  const command = useCommands.getState().commands[id];
  if (!command) return;

  useCommands.setState((state) => {
    const recent = [id, ...state.recent.filter((entry) => entry !== id)].slice(0, RECENT_LIMIT);
    writeStored(RECENT_KEY, recent);
    return { recent };
  });

  const fail = (error: unknown) => {
    toast.error(`"${command.title}" did not complete`, {
      description: error instanceof Error ? error.message : undefined,
    });
  };
  try {
    void Promise.resolve(command.run()).catch(fail);
  } catch (error) {
    fail(error);
  }
}
