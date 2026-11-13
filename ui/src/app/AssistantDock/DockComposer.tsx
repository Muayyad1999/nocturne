import { ArrowUp } from 'lucide-react';
import { useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react';
import { IconButton } from '@/design/Button';
import styles from './AssistantDock.module.css';

export interface DockComposerProps {
  /** Receives the trimmed message; the field is cleared. */
  onSubmit?: (message: string) => void;
  /** Nothing can be sent right now. Say why in `placeholder`. */
  disabled?: boolean;
  /** A reply is in progress: sending is held back, typing is not. */
  pending?: boolean;
  placeholder?: string;
}

/** The dock's message field: Enter sends, Shift+Enter starts a new line. */
export function DockComposer({
  onSubmit,
  disabled = false,
  pending = false,
  placeholder = 'Ask about your network',
}: DockComposerProps) {
  const [text, setText] = useState('');
  const fieldRef = useRef<HTMLTextAreaElement>(null);
  const message = text.trim();
  const canSend = !disabled && !pending && message.length > 0;

  // Grow with the text, up to the maximum height set in CSS.
  useLayoutEffect(() => {
    const field = fieldRef.current;
    if (!field) return;
    field.style.height = 'auto';
    field.style.height = `${field.scrollHeight}px`;
  }, [text]);

  const send = () => {
    if (!canSend) return;
    onSubmit?.(message);
    setText('');
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== 'Enter' || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    send();
  };

  return (
    <form
      className={styles.composer}
      onSubmit={(event) => {
        event.preventDefault();
        send();
      }}
    >
      <div className={styles.field} data-disabled={disabled || undefined}>
        <textarea
          ref={fieldRef}
          className={styles.input}
          rows={1}
          value={text}
          disabled={disabled}
          placeholder={placeholder}
          aria-label="Message to the assistant"
          onChange={(event) => setText(event.target.value)}
          onKeyDown={onKeyDown}
        />
        <IconButton
          type="submit"
          icon={ArrowUp}
          variant="ai"
          size="sm"
          aria-label="Send"
          tooltip={false}
          loading={pending}
          disabled={!canSend && !pending}
        />
      </div>
    </form>
  );
}
