/**
 * The box you type a template body, a playbook or a list of commands into.
 *
 * CodeMirror rather than a textarea for one reason: line numbers. A template
 * error and a playbook problem are both reported *by line*, and a marker in
 * the gutter is the only way to put the backend's complaint where the mistake
 * is. There is no autocompletion and no syntax highlighting: a guess about
 * what the engineer meant has no place where what is typed becomes
 * configuration.
 */

import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { EditorState, StateEffect, StateField, type Extension } from '@codemirror/state';
import {
  Decoration,
  EditorView,
  gutter,
  GutterMarker,
  highlightActiveLine,
  keymap,
  lineNumbers,
  type DecorationSet,
} from '@codemirror/view';
import { useEffect, useRef, type CSSProperties } from 'react';
import { cx } from '@/lib/cx';
import styles from './CodeEditor.module.css';

export interface CodeEditorProps {
  value: string;
  onChange: (value: string) => void;
  /** The accessible name: what is being written here. */
  label: string;
  /** Lines the backend complained about, 1-based, with the message. */
  problems?: readonly { line?: number; message: string }[];
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
  '&.cm-focused': { outline: 'var(--focus-ring)', outlineOffset: 'var(--focus-offset)' },
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
  '.cm-problemLine': { backgroundColor: 'var(--crit-soft)' },
  '.cm-problemMarker': { color: 'var(--crit)', paddingLeft: '4px', fontWeight: '600' },
});

/** The lines to mark, pushed in from outside after a validation. */
const setProblems = StateEffect.define<readonly number[]>();

const problemLines = StateField.define<readonly number[]>({
  create: () => [],
  update(value, transaction) {
    for (const effect of transaction.effects) if (effect.is(setProblems)) return effect.value;
    return value;
  },
});

const problemHighlight = EditorView.decorations.compute([problemLines], (state) => {
  const lines = state.field(problemLines);
  const marks = [];
  for (const line of lines) {
    if (line < 1 || line > state.doc.lines) continue;
    const at = state.doc.line(line);
    marks.push(Decoration.line({ class: 'cm-problemLine' }).range(at.from));
  }
  return Decoration.set(marks, true) as DecorationSet;
});

class ProblemMarker extends GutterMarker {
  override toDOM() {
    const span = document.createElement('span');
    span.textContent = '!';
    span.title = 'This line was reported as a problem';
    return span;
  }
}

const problemGutter = gutter({
  class: 'cm-problemMarker',
  lineMarker: (view, block) => {
    const line = view.state.doc.lineAt(block.from).number;
    return view.state.field(problemLines).includes(line) ? new ProblemMarker() : null;
  },
  lineMarkerChange: (update) => update.transactions.some((tr) => tr.effects.some((e) => e.is(setProblems))),
  initialSpacer: () => new ProblemMarker(),
});

const extensions: Extension[] = [
  lineNumbers(),
  problemLines,
  problemHighlight,
  problemGutter,
  history(),
  highlightActiveLine(),
  keymap.of([...defaultKeymap, ...historyKeymap]),
  EditorView.lineWrapping,
  theme,
];

export function CodeEditor({
  value,
  onChange,
  label,
  problems,
  placeholder,
  minRows = 12,
  className,
}: CodeEditorProps) {
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
          ...extensions,
          EditorView.contentAttributes.of({ 'aria-label': label, spellcheck: 'false' }),
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
    // The editor is created once; `value` and the problems are pushed in below.
  }, [label]);

  // Keep the document in step when the value is changed from outside (another
  // template was opened) without fighting the caret.
  useEffect(() => {
    const editor = view.current;
    if (!editor) return;
    const current = editor.state.doc.toString();
    if (current === value) return;
    editor.dispatch({ changes: { from: 0, to: current.length, insert: value } });
  }, [value]);

  const marked = (problems ?? [])
    .map((problem) => problem.line)
    .filter((line): line is number => typeof line === 'number');
  const markedKey = marked.join(',');
  useEffect(() => {
    view.current?.dispatch({ effects: setProblems.of(markedKey === '' ? [] : markedKey.split(',').map(Number)) });
  }, [markedKey]);

  return (
    <div
      ref={host}
      className={cx(styles.editor, className)}
      style={{ '--editor-min-height': `${minRows * 20 + 16}px` } as CSSProperties}
      data-placeholder={value === '' ? placeholder : undefined}
    />
  );
}
