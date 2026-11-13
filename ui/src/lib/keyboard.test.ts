// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import {
  allowsGlobalShortcuts,
  formatShortcut,
  hasModifier,
  isEditableTarget,
  matchesShortcut,
  parseShortcut,
  type KeyEventLike,
} from './keyboard';

function press(key: string, init: Partial<KeyEventLike> = {}): KeyEventLike {
  return { key, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...init };
}

describe('parseShortcut', () => {
  it('parses modifiers and a key', () => {
    expect(parseShortcut('mod+shift+p')).toEqual({
      key: 'p',
      mod: true,
      ctrl: false,
      meta: false,
      alt: false,
      shift: true,
    });
  });

  it('normalises key names', () => {
    expect(parseShortcut('Esc').key).toBe('escape');
    expect(parseShortcut('alt+Up').key).toBe('arrowup');
    expect(parseShortcut('mod+plus').key).toBe('+');
    expect(parseShortcut('space').key).toBe('space');
  });

  it('rejects malformed shortcuts', () => {
    expect(() => parseShortcut('')).toThrow(/Invalid shortcut/);
    expect(() => parseShortcut('hyper+k')).toThrow(/unknown modifier/);
  });

  it('knows whether a shortcut has a modifier', () => {
    expect(hasModifier(parseShortcut('mod+k'))).toBe(true);
    expect(hasModifier(parseShortcut('shift+a'))).toBe(false);
    expect(hasModifier(parseShortcut('?'))).toBe(false);
  });
});

describe('matchesShortcut', () => {
  it('maps mod to Ctrl off macOS and ⌘ on macOS', () => {
    expect(matchesShortcut(press('k', { ctrlKey: true }), 'mod+k', false)).toBe(true);
    expect(matchesShortcut(press('k', { metaKey: true }), 'mod+k', false)).toBe(false);
    expect(matchesShortcut(press('k', { metaKey: true }), 'mod+k', true)).toBe(true);
    expect(matchesShortcut(press('k', { ctrlKey: true }), 'mod+k', true)).toBe(false);
  });

  it('requires exactly the declared modifiers', () => {
    expect(matchesShortcut(press('k', { ctrlKey: true, shiftKey: true }), 'mod+k', false)).toBe(false);
    expect(matchesShortcut(press('K', { ctrlKey: true, shiftKey: true }), 'mod+shift+k', false)).toBe(true);
    expect(matchesShortcut(press('k', { ctrlKey: true, altKey: true }), 'mod+k', false)).toBe(false);
    expect(matchesShortcut(press('k'), 'mod+k', false)).toBe(false);
  });

  it('ignores Shift for symbols that need it on some layouts', () => {
    expect(matchesShortcut(press('?', { shiftKey: true }), '?', false)).toBe(true);
    expect(matchesShortcut(press('?'), '?', false)).toBe(true);
  });

  it('matches named keys', () => {
    expect(matchesShortcut(press('Escape'), 'esc', false)).toBe(true);
    expect(matchesShortcut(press(' '), 'space', false)).toBe(true);
    expect(matchesShortcut(press('ArrowDown', { altKey: true }), 'alt+down', false)).toBe(true);
  });

  it('falls back to the physical key on non-Latin layouts and with Option', () => {
    expect(matchesShortcut(press('л', { ctrlKey: true, code: 'KeyK' }), 'mod+k', false)).toBe(true);
    expect(matchesShortcut(press('˚', { altKey: true, code: 'KeyK' }), 'alt+k', true)).toBe(true);
  });

  it('keeps the logical letter on Latin layouts such as Dvorak', () => {
    // Physical KeyK types "t" on Dvorak: it must not trigger mod+k.
    expect(matchesShortcut(press('t', { ctrlKey: true, code: 'KeyK' }), 'mod+k', false)).toBe(false);
    expect(matchesShortcut(press('k', { ctrlKey: true, code: 'KeyV' }), 'mod+k', false)).toBe(true);
  });
});

describe('formatShortcut', () => {
  it('uses symbols on macOS and words elsewhere', () => {
    expect(formatShortcut('mod+k', true)).toEqual(['⌘', 'K']);
    expect(formatShortcut('mod+k', false)).toEqual(['Ctrl', 'K']);
    expect(formatShortcut('mod+shift+p', true)).toEqual(['⇧', '⌘', 'P']);
    expect(formatShortcut('mod+shift+p', false)).toEqual(['Ctrl', 'Shift', 'P']);
  });

  it('labels named keys', () => {
    expect(formatShortcut('esc', false)).toEqual(['Esc']);
    expect(formatShortcut('alt+up', false)).toEqual(['Alt', '↑']);
    expect(formatShortcut('f2', false)).toEqual(['F2']);
    expect(formatShortcut('mod+plus', false)).toEqual(['Ctrl', '+']);
  });
});

describe('targets', () => {
  it('recognises editable elements', () => {
    const input = document.createElement('input');
    const checkbox = document.createElement('input');
    checkbox.type = 'checkbox';
    expect(isEditableTarget(input)).toBe(true);
    expect(isEditableTarget(document.createElement('textarea'))).toBe(true);
    expect(isEditableTarget(checkbox)).toBe(false);
    expect(isEditableTarget(document.createElement('button'))).toBe(false);
    expect(isEditableTarget(null)).toBe(false);
  });

  it('lets a region opt out of global shortcuts', () => {
    const region = document.createElement('div');
    region.dataset.globalShortcuts = 'off';
    const inner = document.createElement('textarea');
    region.append(inner);
    expect(allowsGlobalShortcuts(inner)).toBe(false);
    expect(allowsGlobalShortcuts(document.createElement('div'))).toBe(true);
    expect(allowsGlobalShortcuts(null)).toBe(true);
  });
});
