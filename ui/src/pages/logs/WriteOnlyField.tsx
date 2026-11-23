/**
 * A field for something that can be written and never read back.
 *
 * A forwarder's URL and token live in the vault. `logs.forward.save` takes
 * them; nothing returns them. So this field never has a stored value to
 * show, and it does not pretend to: it says that one is stored (or that it
 * cannot even say whether one is), and offers the honest moves — replace it,
 * remove it where that is allowed, or leave it exactly as it is.
 *
 * What the parent sends is the value this reports:
 *
 *   `undefined`    nothing was typed: what is stored stays (nothing is sent)
 *   `''`           remove it (only offered when `removable`)
 *   anything else  replace it with that
 *
 * What is typed is masked like a password, with a button to look at it,
 * because a collector's URL often carries a token and a screen is shared
 * more often than people think.
 */

import { Eye, EyeOff, Trash, Undo2 } from 'lucide-react';
import { useState } from 'react';
import { Button } from '@/design/Button';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import styles from './Logs.module.css';

export interface WriteOnlyFieldProps {
  label: string;
  /** What the field holds, as a sentence names it (`URL`, `token`): the reveal button says it. */
  noun?: string;
  /**
   * Whether the vault holds one. `unknown` is for a secret whose presence
   * cannot be read back either (a token stored with a URL).
   */
  stored: boolean | 'unknown';
  /** See the note above. */
  value: string | undefined;
  onChange: (value: string | undefined) => void;
  /** Whether "remove it" is something the engine accepts for this field. */
  removable?: boolean;
  hint?: string;
  /** The engine's refusal, when it is about this field. */
  error?: string;
  placeholder?: string;
  required?: boolean;
}

export function WriteOnlyField({
  label,
  noun = label.toLowerCase(),
  stored,
  value,
  onChange,
  removable = false,
  hint,
  error,
  placeholder,
  required,
}: WriteOnlyFieldProps) {
  // With nothing stored there is nothing to keep, so the box is simply there.
  const [editing, setEditing] = useState(stored === false);
  // Asked for with "Replace…", the box takes the focus: otherwise it stays on
  // the button that took that one's place, "Keep what is stored", and a
  // second key press from the keyboard would undo the first.
  const [replacing, setReplacing] = useState(false);
  const [visible, setVisible] = useState(false);
  const mayBeStored = stored !== false;

  const keep = () => {
    setEditing(false);
    setVisible(false);
    onChange(undefined);
  };

  if (value === '' && removable) {
    return (
      <Field label={label} hint={hint} error={error}>
        <div className={styles.writeOnly}>
          <span>Will be removed when you save.</span>
          <Button size="sm" icon={Undo2} onClick={keep}>
            Keep it
          </Button>
        </div>
      </Field>
    );
  }

  if (!editing && mayBeStored) {
    return (
      <Field label={label} hint={hint} error={error}>
        <div className={styles.writeOnly}>
          <span>
            {stored === true
              ? 'Stored in the vault. It cannot be shown again.'
              : 'Never shown — not even whether one is stored.'}
          </span>
          <Button
            size="sm"
            onClick={() => {
              setReplacing(true);
              setEditing(true);
            }}
          >
            Replace…
          </Button>
          {removable && (
            <Button size="sm" variant="ghost" icon={Trash} onClick={() => onChange('')}>
              Remove
            </Button>
          )}
        </div>
      </Field>
    );
  }

  return (
    <Field
      label={label}
      required={required}
      error={error}
      hint={`${hint ? `${hint} ` : ''}After saving it is never shown again.`}
    >
      <div className={styles.writeOnlyEdit}>
        <Input
          mono
          autoFocus={replacing}
          autoComplete="new-password"
          spellCheck={false}
          type={visible ? 'text' : 'password'}
          placeholder={placeholder}
          value={value ?? ''}
          onChange={(event) => {
            // An empty box is "I changed my mind", not "remove what is stored".
            onChange(event.target.value === '' ? undefined : event.target.value);
          }}
          trailing={
            <button
              type="button"
              className={styles.reveal}
              aria-label={visible ? `Hide the ${noun} you typed` : `Show the ${noun} you typed`}
              aria-pressed={visible}
              onClick={() => setVisible(!visible)}
            >
              {visible ? <EyeOff size={14} aria-hidden="true" /> : <Eye size={14} aria-hidden="true" />}
            </button>
          }
        />
        {mayBeStored && (
          <Button size="sm" variant="ghost" icon={Undo2} onClick={keep}>
            Keep what is stored
          </Button>
        )}
      </div>
    </Field>
  );
}
