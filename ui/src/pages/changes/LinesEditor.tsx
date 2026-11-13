/**
 * The box you type configuration into.
 *
 * CodeMirror rather than a textarea for one reason: line numbers. What is
 * typed here is sent to the device verbatim, so seeing *which* line is line 7
 * matters — the engine refuses a sub-command with no section header above it,
 * and names the line when it does.
 *
 * Everything else is deliberately absent. There is no autocompletion, no
 * syntax highlighting and no reformatting: a guess about what the engineer
 * meant has no place on the one screen whose promise is that what you see is
 * what the device gets.
 */

import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { EditorState } from '@codemirror/state';
import { EditorView, highlightActiveLine, keymap, lineNumbers } from '@codemirror/view';
import { useEffect, useRef } from 'react';
import { cx } from '@/lib/cx';
import styles from './LinesEditor.module.css';

export interface LinesEditorProps {
  value: string;
  onChange: (value: string) => void;
  /** The accessible name: what this configuration is for. */
  label: string;
  placeholder?: string;
  /** Visible rows before it scrolls. */
  minRows?: number;
  className?: string;
}

/** Design tokens, so the editor follows the theme like everything else. */
const theme = EditorView.theme({
  '&': {
    color: 'var(--text-1)',
    backgroundColor: 'var(--bg-0)',
    fontSize: 'var(--fs-12)',
    border: '1px solid var(--line-strong)',
    borderRadius: 'var(--radius-control)',
  },
  '&.cm-focused': {
    outline: 'var(--focus-ring)',
    outlineOffset: 'var(--focus-offset)',
  },
  '.cm-content': {
    fontFamily: 'var(--font-mono)',
    caretColor: 'var(--accent)',
    padding: 'var(--sp-8) 0',
  },
  '.cm-gutters': {
    backgroundColor: 'var(--bg-1)',
    color: 'var(--text-3)',
    border: 'none',
    borderRight: '1px solid var(--line)',
  },
  '.cm-activeLine': { backgroundColor: 'var(--bg-2)' },
  '.cm-activeLineGutter': { backgroundColor: 'var(--bg-2)', color: 'var(--text-2)' },
  '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--accent)' },
  '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection': {
    backgroundColor: 'var(--selection)',
  },
  '.cm-placeholder': { color: 'var(--text-3)' },
  '.cm-scroller': { lineHeight: '20px' },
});

export function LinesEditor({
  value,
  onChange,
  label,
  placeholder,
  minRows = 10,
  className,
}: LinesEditorProps) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const latest = useRef(onChange);
  latest.current = onChange;

  useEffect(() => {
    if (!host.current) return;
    const editor = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: value,
        extensions: [
          lineNumbers(),
          history(),
          highlightActiveLine(),
          keymap.of([...defaultKeymap, ...historyKeymap]),
          EditorView.lineWrapping,
          EditorView.contentAttributes.of({ 'aria-label': label, spellcheck: 'false' }),
          theme,
          EditorView.updateListener.of((update) => {
            if (update.docChanged) latest.current(update.state.doc.toString());
          }),
        ],
      }),
    });
    view.current = editor;
    return () => {
      editor.destroy();
      view.current = null;
    };
    // The editor is created once; `value` is pushed in below.
  }, [label]);

  // Keep the document in step when the value is changed from outside (a
  // template was chosen, the step was reopened) without fighting the caret.
  useEffect(() => {
    const editor = view.current;
    if (!editor) return;
    const current = editor.state.doc.toString();
    if (current === value) return;
    editor.dispatch({ changes: { from: 0, to: current.length, insert: value } });
  }, [value]);

  return (
    <div
      ref={host}
      className={cx(styles.editor, className)}
      style={{ '--lines-min-height': `${minRows * 20 + 16}px` } as React.CSSProperties}
      data-placeholder={value === '' ? placeholder : undefined}
    />
  );
}
