/**
 * Keyboard shortcuts: one grammar for declaring, matching and displaying them.
 *
 * A shortcut is written as modifiers and a key joined by `+`, for example
 * `mod+k`, `mod+shift+p`, `alt+1`, `escape`, `?`. `mod` is ⌘ on macOS and
 * Ctrl everywhere else. Use `plus` for the `+` key and `space` for the space bar.
 */

export interface Shortcut {
  /** Normalised key: a lower-case character or a named key (`enter`, `arrowup`, `f2`). */
  key: string;
  /** ⌘ on macOS, Ctrl elsewhere. */
  mod: boolean;
  ctrl: boolean;
  meta: boolean;
  alt: boolean;
  shift: boolean;
}

/** The subset of `KeyboardEvent` that matching needs. */
export interface KeyEventLike {
  key: string;
  code?: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

const KEY_ALIASES: Record<string, string> = {
  esc: 'escape',
  return: 'enter',
  ' ': 'space',
  spacebar: 'space',
  up: 'arrowup',
  down: 'arrowdown',
  left: 'arrowleft',
  right: 'arrowright',
  del: 'delete',
  plus: '+',
};

function normalizeKey(key: string): string {
  const lower = key.toLowerCase();
  return KEY_ALIASES[lower] ?? lower;
}

/** True on macOS and iOS, where ⌘ is the primary modifier. */
export function isMacPlatform(): boolean {
  if (typeof navigator === 'undefined') return false;
  const platform =
    (navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData?.platform ??
    navigator.platform ??
    '';
  return /mac|iphone|ipad/i.test(platform);
}

/** Parses `mod+shift+p` into a {@link Shortcut}. Throws on an empty or malformed string. */
export function parseShortcut(text: string): Shortcut {
  const tokens = text
    .trim()
    .split('+')
    .map((token) => token.trim().toLowerCase());
  const key = tokens.pop();
  if (!key) throw new Error(`Invalid shortcut "${text}"`);

  const shortcut: Shortcut = {
    key: normalizeKey(key),
    mod: false,
    ctrl: false,
    meta: false,
    alt: false,
    shift: false,
  };
  for (const token of tokens) {
    switch (token) {
      case 'mod':
        shortcut.mod = true;
        break;
      case 'ctrl':
      case 'control':
        shortcut.ctrl = true;
        break;
      case 'meta':
      case 'cmd':
      case 'command':
        shortcut.meta = true;
        break;
      case 'alt':
      case 'option':
        shortcut.alt = true;
        break;
      case 'shift':
        shortcut.shift = true;
        break;
      default:
        throw new Error(`Invalid shortcut "${text}": unknown modifier "${token}"`);
    }
  }
  return shortcut;
}

/** Keys whose character already depends on Shift, so Shift is not compared. */
function isShiftedSymbol(key: string): boolean {
  return key.length === 1 && !/[a-z0-9]/.test(key);
}

/**
 * The key an event stands for. With Option on macOS or a non-Latin layout the
 * reported character is not the Latin letter on the key cap, so the physical
 * key is used instead; Latin layouts such as Dvorak keep their own letters.
 */
function eventKey(event: KeyEventLike): string {
  const key = normalizeKey(event.key);
  const named = key.length > 1 && key !== 'dead';
  const alphanumeric = /^[a-z0-9]$/.test(key);
  const asciiSymbol = /^[\x21-\x7e]$/.test(key) && !alphanumeric;
  if (named || alphanumeric || (asciiSymbol && !event.altKey)) return key;
  const physical = /^(?:Key([A-Z])|Digit([0-9]))$/.exec(event.code ?? '');
  return (physical?.[1] ?? physical?.[2] ?? key).toLowerCase();
}

/** Whether a keyboard event is exactly the given shortcut. */
export function matchesShortcut(
  event: KeyEventLike,
  shortcut: string | Shortcut,
  mac: boolean = isMacPlatform(),
): boolean {
  const wanted = typeof shortcut === 'string' ? parseShortcut(shortcut) : shortcut;
  if (eventKey(event) !== wanted.key) return false;
  if (event.ctrlKey !== (wanted.ctrl || (wanted.mod && !mac))) return false;
  if (event.metaKey !== (wanted.meta || (wanted.mod && mac))) return false;
  if (event.altKey !== wanted.alt) return false;
  return isShiftedSymbol(wanted.key) || event.shiftKey === wanted.shift;
}

/** Whether a shortcut includes a modifier other than Shift. */
export function hasModifier(shortcut: Shortcut): boolean {
  return shortcut.mod || shortcut.ctrl || shortcut.meta || shortcut.alt;
}

const KEY_LABELS: Record<string, string> = {
  escape: 'Esc',
  enter: 'Enter',
  space: 'Space',
  tab: 'Tab',
  backspace: '⌫',
  delete: 'Del',
  arrowup: '↑',
  arrowdown: '↓',
  arrowleft: '←',
  arrowright: '→',
  home: 'Home',
  end: 'End',
  pageup: 'PgUp',
  pagedown: 'PgDn',
};

function keyLabel(key: string): string {
  return KEY_LABELS[key] ?? (key.length === 1 ? key.toUpperCase() : key.replace(/^f(\d+)$/, 'F$1'));
}

/**
 * Key caps for display, in platform order: `['⌘', 'K']` on macOS,
 * `['Ctrl', 'K']` elsewhere.
 */
export function formatShortcut(shortcut: string | Shortcut, mac: boolean = isMacPlatform()): string[] {
  const parsed = typeof shortcut === 'string' ? parseShortcut(shortcut) : shortcut;
  const ctrl = parsed.ctrl || (parsed.mod && !mac);
  const meta = parsed.meta || (parsed.mod && mac);
  const caps = mac
    ? [ctrl && '⌃', parsed.alt && '⌥', parsed.shift && '⇧', meta && '⌘']
    : [ctrl && 'Ctrl', meta && 'Win', parsed.alt && 'Alt', parsed.shift && 'Shift'];
  return [...caps.filter((cap): cap is string => Boolean(cap)), keyLabel(parsed.key)];
}

/** Whether the event target is a place where the user types text. */
export function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  if (target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return true;
  if (target instanceof HTMLInputElement) {
    return !['button', 'checkbox', 'radio', 'range', 'submit', 'reset'].includes(target.type);
  }
  return false;
}

/**
 * Regions that need every key themselves (a terminal, where Ctrl+K belongs to
 * the device) opt out of app-wide shortcuts with `data-global-shortcuts="off"`.
 */
export function allowsGlobalShortcuts(target: EventTarget | null): boolean {
  return !(target instanceof Element && target.closest('[data-global-shortcuts="off"]'));
}
