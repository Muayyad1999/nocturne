/**
 * An API key the backend will never hand back.
 *
 * `ai.providers.save` puts the key in the vault under `ai:provider:<id>:key`;
 * nothing — not `ai.providers.list`, not an event, not a log line — returns
 * it. So this field shows only whether one is **set**, and offers the two
 * honest things that can be done to it: replace it, or clear it.
 *
 * What the parent sends as `api_key`:
 *
 *   `undefined`    leave the stored key alone (the field is left out)
 *   `''`           clear it
 *   anything else  replace it
 */

import { Eye, EyeOff, Trash, Undo2 } from 'lucide-react';
import { useState } from 'react';
import { Button } from '@/design/Button';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { StatusDot } from '@/design/StatusDot';
import styles from './Assistant.module.css';

export interface SecretFieldProps {
  label: string;
  hint?: string;
  /** Whether the vault already holds a key for this provider. */
  isSet: boolean;
  value: string | undefined;
  onChange: (value: string) => void;
  /** Go back to leaving the stored key untouched. */
  onReset: () => void;
}

type Mode = 'stored' | 'editing' | 'clearing';

export function SecretField({ label, hint, isSet, value, onChange, onReset }: SecretFieldProps) {
  const [mode, setMode] = useState<Mode>('stored');
  const [visible, setVisible] = useState(false);

  const keep = () => {
    setMode('stored');
    setVisible(false);
    onReset();
  };

  if (mode === 'clearing' && value === '') {
    return (
      <Field label={label} hint={hint}>
        <div className={styles.secretState}>
          <span className={styles.clearing}>Will be removed when you save.</span>
          <Button size="sm" icon={Undo2} onClick={keep}>
            Keep it
          </Button>
        </div>
      </Field>
    );
  }

  if (mode === 'stored') {
    return (
      <Field label={label} hint={hint}>
        <div className={styles.secretState}>
          <span className={styles.secretStatus}>
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

  return (
    <Field
      label={label}
      hint={isSet ? `${hint ? `${hint} ` : ''}The stored key is replaced when you save.` : hint}
    >
      <div className={styles.secretEditing}>
        <Input
          autoComplete="new-password"
          aria-label={label}
          type={visible ? 'text' : 'password'}
          placeholder="sk-…"
          value={value ?? ''}
          onChange={(event) => {
            // An empty box means "I changed my mind", not "clear the key".
            if (event.target.value === '') onReset();
            else onChange(event.target.value);
          }}
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
        {isSet && (
          <Button size="sm" variant="ghost" icon={Undo2} onClick={keep}>
            Keep the stored one
          </Button>
        )}
      </div>
    </Field>
  );
}
