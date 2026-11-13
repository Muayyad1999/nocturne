/**
 * The CodeMirror pieces the configuration views share: the theme, a cheap
 * highlighter for device configuration, and the marker that shows where a
 * secret was taken out.
 *
 * This module imports CodeMirror, so it must only ever be reached through a
 * dynamic import (see `ConfigView.tsx` and `IntendedEditor.tsx`). The
 * Changes screen learned that the hard way: CodeMirror is 276 kB, and no
 * screen may pay for it before it shows an editor.
 *
 * The highlighting is deliberately regex-based rather than a real grammar.
 * Nocturne supports IOS, RouterOS, VRP and set-style configurations, and a
 * grammar for each would be a dependency and a lie for the fifth platform;
 * comments, section indentation, addresses, numbers and quoted strings are
 * common to all of them and are what the eye actually uses.
 */

import { EditorView, type DecorationSet } from '@codemirror/view';
import { Decoration, ViewPlugin, type ViewUpdate } from '@codemirror/view';
import { RangeSetBuilder, StateEffect, StateField } from '@codemirror/state';
import type { Range } from './find';

/** Design tokens, so an editor follows the theme like everything else. */
export const configTheme = EditorView.theme({
  '&': {
    color: 'var(--text-1)',
    backgroundColor: 'var(--bg-0)',
    fontSize: 'var(--fs-12)',
    height: '100%',
  },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': { lineHeight: '19px', fontFamily: 'var(--font-mono)' },
  '.cm-content': { fontFamily: 'var(--font-mono)', caretColor: 'var(--accent)', padding: 'var(--sp-8) 0' },
  '.cm-gutters': {
    backgroundColor: 'var(--bg-1)',
    color: 'var(--text-3)',
    border: 'none',
    borderRight: '1px solid var(--line)',
    userSelect: 'none',
  },
  '.cm-activeLine': { backgroundColor: 'var(--bg-2)' },
  '.cm-activeLineGutter': { backgroundColor: 'var(--bg-2)', color: 'var(--text-2)' },
  '.cm-selectionBackground, &.cm-focused .cm-selectionBackground, ::selection': {
    backgroundColor: 'var(--selection)',
  },
  '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--accent)' },
  '.cm-placeholder': { color: 'var(--text-3)' },
  // Highlighting. Status colours are not used: nothing here is a status.
  '.ncfg-comment': { color: 'var(--text-3)', fontStyle: 'italic' },
  '.ncfg-keyword': { color: 'var(--accent-strong)' },
  '.ncfg-negation': { color: 'var(--warn)' },
  '.ncfg-addr': { color: 'var(--info)' },
  '.ncfg-number': { color: 'var(--info)' },
  '.ncfg-string': { color: 'var(--ok)' },
  '.ncfg-secret': {
    color: 'var(--maint)',
    backgroundColor: 'var(--maint-soft)',
    borderRadius: '3px',
    padding: '0 2px',
    outline: '1px solid color-mix(in oklab, var(--maint) 45%, transparent)',
  },
  '.ncfg-secret-weak': {
    color: 'var(--crit)',
    backgroundColor: 'var(--crit-soft)',
    outline: '1px solid color-mix(in oklab, var(--crit) 45%, transparent)',
  },
  '.ncfg-match': { backgroundColor: 'var(--warn-soft)', outline: '1px solid var(--warn)' },
  '.ncfg-match-current': { backgroundColor: 'var(--accent-soft)', outline: '1px solid var(--accent)' },
});

// ------------------------------------------------------------ search matches

export interface Marks {
  ranges: readonly Range[];
  /** Index into `ranges` of the one the engineer is looking at. */
  current: number;
}

export const setMarks = StateEffect.define<Marks>();

export const marksField = StateField.define<Marks>({
  create: () => ({ ranges: [], current: 0 }),
  update(value, transaction) {
    for (const effect of transaction.effects) if (effect.is(setMarks)) return effect.value;
    return value;
  },
});

// ----------------------------------------------------------------- the marks

interface Mark {
  from: number;
  to: number;
  cls: string;
}

const COMMENT = /^\s*[!#]/;
const STRING = /"[^"\n]*"/g;
const ADDRESS = /\b\d{1,3}(?:\.\d{1,3}){3}(?:\/\d{1,2})?\b/g;
const NUMBER = /\b\d+(?:\.\d+)?\b/g;
const SECRET = /<secret:(\d+)(:weak)?>/g;
const HEAD = /^(\s*)(no\s+|)([A-Za-z][\w-]*)/;

function marksForLine(text: string, start: number): Mark[] {
  const marks: Mark[] = [];
  if (COMMENT.test(text)) {
    if (text.trim() !== '') marks.push({ from: start, to: start + text.length, cls: 'ncfg-comment' });
    return marks;
  }
  const head = HEAD.exec(text);
  if (head) {
    const indent = head[1] ?? '';
    const negation = head[2] ?? '';
    const word = head[3] ?? '';
    let at = start + indent.length;
    if (negation !== '') {
      marks.push({ from: at, to: at + negation.trimEnd().length, cls: 'ncfg-negation' });
      at += negation.length;
    }
    marks.push({ from: at, to: at + word.length, cls: 'ncfg-keyword' });
  }
  const add = (pattern: RegExp, cls: string) => {
    pattern.lastIndex = 0;
    for (const match of text.matchAll(pattern)) {
      const from = start + (match.index ?? 0);
      marks.push({ from, to: from + match[0].length, cls });
    }
  };
  add(SECRET, 'ncfg-secret');
  add(STRING, 'ncfg-string');
  add(ADDRESS, 'ncfg-addr');
  add(NUMBER, 'ncfg-number');
  for (const match of text.matchAll(SECRET)) {
    if (match[2] === undefined) continue;
    const from = start + (match.index ?? 0);
    marks.push({ from, to: from + match[0].length, cls: 'ncfg-secret ncfg-secret-weak' });
  }
  return marks;
}

/**
 * Keeps the first mark of any overlapping pair, so a number inside a secret
 * placeholder cannot break the placeholder's own marker apart. Earlier rules
 * win, which is why secrets are added before numbers.
 */
function flatten(marks: Mark[]): Mark[] {
  const sorted = [...marks].sort((a, b) => a.from - b.from || b.to - a.to);
  const kept: Mark[] = [];
  let end = -1;
  for (const mark of sorted) {
    if (mark.from >= end && mark.to > mark.from) {
      kept.push(mark);
      end = mark.to;
    }
  }
  return kept;
}

function decorations(view: EditorView): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  const { ranges, current } = view.state.field(marksField);
  const marks: Mark[] = [];
  for (const { from, to } of view.visibleRanges) {
    for (let pos = from; pos <= to; ) {
      const line = view.state.doc.lineAt(pos);
      marks.push(...marksForLine(line.text, line.from));
      pos = line.to + 1;
    }
  }
  const kept = flatten(marks);
  // Search hits sit on top of the syntax marks: both are drawn, and a mark
  // that would cut a hit in half is harmless because they are separate sets.
  const hits = ranges.map((range, index) => ({
    from: range.from,
    to: range.to,
    cls: index === current ? 'ncfg-match ncfg-match-current' : 'ncfg-match',
  }));
  for (const mark of [...kept, ...hits].sort((a, b) => a.from - b.from || a.to - b.to)) {
    builder.add(mark.from, mark.to, Decoration.mark({ class: mark.cls }));
  }
  return builder.finish();
}

/** Highlights configuration text, its redacted secrets and the search hits. */
export const configHighlight = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = decorations(view);
    }

    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.viewportChanged ||
        update.state.field(marksField) !== update.startState.field(marksField)
      ) {
        this.decorations = decorations(update.view);
      }
    }
  },
  { decorations: (plugin) => plugin.decorations },
);
