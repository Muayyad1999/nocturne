/**
 * A secret that is already in the vault cannot be shown — the backend never
 * returns one. So this field shows only whether one is **set**, and offers
 * the two things that can honestly be done to it: replace it, or clear it.
 *
 * What the parent sends to the backend is the `value` this field reports:
 *
 *   `undefined`  leave the stored secret exactly as it is (nothing is sent)
 *   `''`         clear it
 *   anything else  replace it with that
 */

import { Eye, EyeOff, Trash, Undo2 } from 'lucide-react';
import { useState } from 'react';
import { Button } from '@/design/Button';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { StatusDot } from '@/design/StatusDot';
import { Textarea } from '@/design/Textarea';
import styles from './SecretField.module.css';

export interface SecretFieldProps {
  label: string;
  help?: string;
  /** Whether the vault already holds this secret. */
  isSet: boolean;
  /** What will be sent: see the note above. */
  value: string | undefined;
  /** Called with the new value; an empty box reports `undefined` through `onReset`. */
  onChange: (value: string) => void;
  /** Go back to leaving the stored secret untouched. */
  onReset: () => void;
  /** A private key needs more than one line. */
  multiline?: boolean;
}

type Mode = 'stored' | 'editing' | 'clearing';

export function SecretField({
  label,
  help,
  isSet,
  value,
  onChange,
  onReset,
  multiline,
}: SecretFieldProps) {
  const [mode, setMode] = useState<Mode>('stored');
  const [visible, setVisible] = useState(false);

  const typed = (text: string) => {
    // An empty box means "I changed my mind", not "clear the secret".
    if (text === '') onReset();
    else onChange(text);
  };

  const keep = () => {
    setMode('stored');
    setVisible(false);
    onReset();
  };

  if (mode === 'clearing' && value === '') {
    return (
      <Field label={label} hint={help}>
        <div className={styles.state}>
          <span className={styles.clearing}>Will be removed when you save.</span>
          <Button size="sm" icon={Undo2} onClick={keep}>
            Keep it
          </Button>
        </div>
      </Field>
    );
  }

  if (mode !== 'editing') {
    return (
      <Field label={label} hint={help}>
        <div className={styles.state}>
          <span className={styles.status}>
            <StatusDot tone={isSet ? 'ok' : 'idle'} label={isSet ? 'Set' : 'Not set'} />
            {isSet ? 'Set' : 'Not set'}
          </span>
          <Button size="sm" onClick={() => setMode('editing')}>
            {isSet ? 'Replace…' : 'Set…'}
          </Button>
          {isSet && (
            <Button
              size="sm"
              variant="ghost"
              icon={Trash}
              onClick={() => {
                setMode('clearing');
                onChange('');
              }}
            >
              Clear
            </Button>
          )}
        </div>
      </Field>
    );
  }

  const text = value ?? '';

  return (
    <Field
      label={label}
      hint={
        isSet
          ? `${help ? `${help} ` : ''}The stored ${label.toLowerCase()} is replaced when you save.`
          : help
      }
    >
      <div className={styles.editing}>
        {multiline ? (
          <Textarea
            mono
            rows={5}
            autoComplete="off"
            spellCheck={false}
            aria-label={label}
            placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
            value={text}
            onChange={(event) => typed(event.target.value)}
          />
        ) : (
          <Input
            autoComplete="new-password"
            aria-label={label}
            type={visible ? 'text' : 'password'}
            value={text}
            onChange={(event) => typed(event.target.value)}
            trailing={
              <button
                type="button"
                className={styles.reveal}
                aria-label={visible ? 'Hide what you typed' : 'Show what you typed'}
                aria-pressed={visible}
                onClick={() => setVisible(!visible)}
              >
                {visible ? <EyeOff size={14} /> : <Eye size={14} />}
              </button>
            }
          />
        )}
        <Button size="sm" variant="ghost" icon={Undo2} onClick={keep}>
          {isSet ? 'Keep the stored one' : 'Cancel'}
        </Button>
      </div>
    </Field>
  );
}
