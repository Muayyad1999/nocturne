/**
 * One configuration, as the device said it.
 *
 * Read-only CodeMirror: line numbers (a configuration is discussed by line
 * number), the cheap highlighter from `editor.ts`, and the search hits the
 * surrounding panel found. Nothing is editable and nothing is reformatted —
 * what is on screen is the stored text, character for character, with the
 * secrets the backend took out marked as the placeholders they are.
 *
 * Loaded through `React.lazy`, so CodeMirror reaches the browser only when
 * somebody opens a configuration.
 */

import { EditorState } from '@codemirror/state';
import { EditorView, highlightActiveLine, lineNumbers } from '@codemirror/view';
import { useEffect, useRef } from 'react';
import { cx } from '@/lib/cx';
import styles from './ConfigView.module.css';
import { configHighlight, configTheme, marksField, setMarks } from './editor';
import type { Range } from './find';

export interface ConfigViewProps {
  text: string;
  /** What this configuration is: the accessible name of the text box. */
  label: string;
  /** Places the in-panel search found, in document order. */
  matches?: readonly Range[];
  /** Index into `matches` of the one to scroll to. */
  current?: number;
  className?: string;
}

export default function ConfigView({
  text,
  label,
  matches = [],
  current = 0,
  className,
}: ConfigViewProps) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);

  useEffect(() => {
    if (!host.current) return;
    const editor = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: text,
        extensions: [
          lineNumbers(),
          highlightActiveLine(),
          marksField,
          configHighlight,
          configTheme,
          EditorView.editable.of(false),
          EditorState.readOnly.of(true),
          EditorView.contentAttributes.of({
            'aria-label': label,
            'aria-readonly': 'true',
            spellcheck: 'false',
            tabindex: '0',
          }),
        ],
      }),
    });
    view.current = editor;
    return () => {
      editor.destroy();
      view.current = null;
    };
    // Created once; the document and the marks are pushed in below.
  }, [label]);

  useEffect(() => {
    const editor = view.current;
    if (!editor) return;
    const doc = editor.state.doc.toString();
    if (doc === text) return;
    editor.dispatch({ changes: { from: 0, to: doc.length, insert: text } });
  }, [text]);

  useEffect(() => {
    const editor = view.current;
    if (!editor) return;
    const limit = editor.state.doc.length;
    const within = matches.filter((range) => range.to <= limit);
    editor.dispatch({ effects: setMarks.of({ ranges: within, current }) });
    const hit = within[current];
    if (hit) {
      editor.dispatch({
        effects: EditorView.scrollIntoView(hit.from, { y: 'center' }),
      });
    }
  }, [matches, current]);

  return <div ref={host} className={cx(styles.view, className)} />;
}
