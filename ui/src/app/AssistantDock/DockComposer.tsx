import { ArrowUp, Square } from 'lucide-react';
import { useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import { IconButton } from '@/design/Button';
import styles from './AssistantDock.module.css';

export interface DockComposerProps {
  /**
   * Receives the trimmed message. Returning `false` keeps the text in the
   * field: nothing a person typed is thrown away because the application was
   * not ready for it.
   */
  onSubmit?: (message: string) => boolean | void;
  /** Nothing can be sent right now. Say why in `placeholder`. */
  disabled?: boolean;
  /** A reply is in progress: sending is held back, typing is not. */
  pending?: boolean;
  placeholder?: string;
  /** Context chips and quick actions, above the field. */
  above?: ReactNode;
  /** One quiet line under the field: what Enter does, what is missing. */
  hint?: ReactNode;
  /**
   * Stops the running turn. While a turn runs and this is given, the send
   * button becomes a stop button: a turn that is reading devices has to be
   * interruptible from where it was started.
   */
  onStop?: () => void;
}

/** The dock's message field: Enter sends, Shift+Enter starts a new line. */
export function DockComposer({
  onSubmit,
  disabled = false,
  pending = false,
  placeholder = 'Ask about your network',
  above,
  hint,
  onStop,
}: DockComposerProps) {
  const [text, setText] = useState('');
  const fieldRef = useRef<HTMLTextAreaElement>(null);
  const message = text.trim();
  const canSend = !disabled && !pending && message.length > 0;
  const canStop = pending && Boolean(onStop);

  // Grow with the text, up to the maximum height set in CSS.
  useLayoutEffect(() => {
    const field = fieldRef.current;
    if (!field) return;
    field.style.height = 'auto';
    field.style.height = `${field.scrollHeight}px`;
  }, [text]);

  const send = () => {
    if (!canSend) return;
    if (onSubmit?.(message) === false) return;
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
      {above}
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
        {canStop ? (
          <IconButton
            type="button"
            icon={Square}
            size="sm"
            aria-label="Stop"
            tooltip="Stop this answer"
            tooltipPlacement="top-end"
            onClick={onStop}
          />
        ) : (
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
        )}
      </div>
      {hint && <p className={styles.hint}>{hint}</p>}
    </form>
  );
}
