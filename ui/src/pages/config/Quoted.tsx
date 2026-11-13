/**
 * The engine's prose, with the bits it quoted shown as what they are.
 *
 * Compliance rationales, check descriptions and findings are written by the
 * backend with configuration lines in backticks — "Checked: the global line
 * `service password-encryption`". The words are the engine's and are never
 * paraphrased here, but a backtick is punctuation a reader has to decode;
 * rendering those spans in the monospace face the rest of the screen uses
 * for device text makes the sentence scannable without changing it.
 */

import styles from './Quoted.module.css';

/** Splits on backtick-delimited spans, keeping everything in order. */
export function splitQuoted(text: string): { text: string; code: boolean }[] {
  const parts: { text: string; code: boolean }[] = [];
  let rest = text;
  while (rest.length > 0) {
    const open = rest.indexOf('`');
    if (open === -1) {
      parts.push({ text: rest, code: false });
      break;
    }
    const close = rest.indexOf('`', open + 1);
    if (close === -1) {
      parts.push({ text: rest, code: false });
      break;
    }
    if (open > 0) parts.push({ text: rest.slice(0, open), code: false });
    parts.push({ text: rest.slice(open + 1, close), code: true });
    rest = rest.slice(close + 1);
  }
  return parts.filter((part) => part.text !== '');
}

export function Quoted({ text }: { text: string }) {
  return (
    <>
      {splitQuoted(text).map((part, index) =>
        part.code ? (
          <code key={index} className={styles.code}>
            {part.text}
          </code>
        ) : (
          part.text
        ),
      )}
    </>
  );
}
