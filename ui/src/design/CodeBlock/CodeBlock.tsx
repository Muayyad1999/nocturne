import { Check, Copy } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { copyText } from '@/lib/clipboard';
import { cx } from '@/lib/cx';
import { splitLines } from '@/lib/diff';
import { IconButton } from '../Button';
import { toast } from '../Toast';
import styles from './CodeBlock.module.css';

export interface CodeBlockProps {
  /** The text: a command, device output, a piece of configuration. */
  code: string;
  /** Names the block in its header: a file, a device, a command. */
  title?: string;
  lineNumbers?: boolean;
  /**
   * The number of the first line, for text that is a piece of something
   * longer: a page of a long capture is numbered as the whole is. Default 1.
   */
  firstLine?: number;
  /** Offers a button that copies the text exactly as given. */
  copyable?: boolean;
  /** Wrap long lines instead of scrolling sideways. */
  wrap?: boolean;
  /** Caps the height (a number is pixels); the block then scrolls inside. */
  maxHeight?: number | string;
  className?: string;
}

const COPIED_FEEDBACK_MS = 1500;

/** Monospace text as a device said it or will be told: read-only, selectable, copyable. */
export function CodeBlock({
  code,
  title,
  lineNumbers = false,
  firstLine = 1,
  copyable = true,
  wrap = false,
  maxHeight,
  className,
}: CodeBlockProps) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);

  const copy = async () => {
    if (!(await copyText(code))) {
      toast.error('Could not copy to the clipboard', {
        description: 'Select the text and copy it with the keyboard instead.',
      });
      return;
    }
    setCopied(true);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), COPIED_FEEDBACK_MS);
  };

  const lines = splitLines(code);
  // The gutter is four digits wide; a number with more would run into the
  // text, so it is widened for a capture that long.
  const digits = String(firstLine + Math.max(lines.length, 1) - 1).length;
  const gutter = digits > 4 ? { width: `${digits}ch` } : undefined;

  return (
    <div className={cx(styles.block, className)} data-wrap={wrap || undefined}>
      {title && <div className={styles.title}>{title}</div>}
      {copyable && (
        <IconButton
          className={styles.copy}
          icon={copied ? Check : Copy}
          size="sm"
          variant="secondary"
          aria-label={copied ? 'Copied' : 'Copy'}
          tooltipPlacement="left"
          onClick={() => void copy()}
        />
      )}
      {/* Focusable so a keyboard user can scroll it. */}
      <pre className={styles.scroll} style={{ maxHeight }} tabIndex={0}>
        {lineNumbers ? (
          <code className={styles.numbered}>
            {lines.map((line, index) => (
              <span key={index} className={styles.line}>
                <span className={styles.number} style={gutter} aria-hidden="true">
                  {index + firstLine}
                </span>
                <span className={styles.text}>{line}</span>
                {'\n'}
              </span>
            ))}
          </code>
        ) : (
          <code>{code}</code>
        )}
      </pre>
      <span className="visually-hidden" role="status">
        {copied ? 'Copied to the clipboard' : ''}
      </span>
    </div>
  );
}
