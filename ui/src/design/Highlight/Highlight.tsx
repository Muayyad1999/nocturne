import type { ReactNode } from 'react';
import styles from './Highlight.module.css';

export interface HighlightProps {
  text: string;
  /** Indices of the characters to emphasise, ascending: what `fuzzyMatch` returns. */
  indices: readonly number[];
}

/** Text with the characters a search matched picked out. */
export function Highlight({ text, indices }: HighlightProps) {
  if (indices.length === 0) return text;

  const parts: ReactNode[] = [];
  let cursor = 0;
  let position = 0;
  while (position < indices.length) {
    // Merge adjacent indices into one run.
    const start = indices[position] ?? 0;
    let end = start + 1;
    position++;
    while (indices[position] === end) {
      end++;
      position++;
    }
    if (start > cursor) parts.push(text.slice(cursor, start));
    parts.push(
      <mark key={start} className={styles.match}>
        {text.slice(start, end)}
      </mark>,
    );
    cursor = end;
  }
  if (cursor < text.length) parts.push(text.slice(cursor));
  return <>{parts}</>;
}
