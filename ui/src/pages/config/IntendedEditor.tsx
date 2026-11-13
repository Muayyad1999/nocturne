/**
 * The box the intended configuration is written in.
 *
 * An editable twin of `ConfigView`: the same highlighting and the same
 * placeholder markers, because a configuration pasted out of Nocturne may
 * well still carry `<secret:1>` in it and the writer needs to see that it
 * does. Nothing is reformatted or completed — what is written here is what
 * the engine compares, line for line.
 *
 * Loaded through `React.lazy`, so CodeMirror arrives only with the editor.
 */

import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { EditorState } from '@codemirror/state';
import { EditorView, keymap, lineNumbers, placeholder as placeholderExt } from '@codemirror/view';
import { useEffect, useRef } from 'react';
import styles from './IntendedEditor.module.css';
import { configHighlight, configTheme, marksField } from './editor';

export interface IntendedEditorProps {
  value: string;
  onChange: (value: string) => void;
  /** The accessible name of the text box. */
  label: string;
  placeholder?: string;
}

export default function IntendedEditor({ value, onChange, label, placeholder }: IntendedEditorProps) {
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
          keymap.of([...defaultKeymap, ...historyKeymap]),
          marksField,
          configHighlight,
          configTheme,
          ...(placeholder ? [placeholderExt(placeholder)] : []),
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
    // Created once; the value is pushed in below.
  }, [label]);

  useEffect(() => {
    const editor = view.current;
    if (!editor) return;
    const doc = editor.state.doc.toString();
    if (doc === value) return;
    editor.dispatch({ changes: { from: 0, to: doc.length, insert: value } });
  }, [value]);

  return <div ref={host} className={styles.editor} />;
}
