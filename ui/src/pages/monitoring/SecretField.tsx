/**
 * A secret that is already in the vault cannot be shown: the backend never
 * returns one, only whether there is one (`Channel.has_secret`). So this
 * field offers the two honest things — replace it, or clear it — and says
 * which of those will happen when the drawer is saved.
 *
 * What the parent sends to `notify.channels.save` is the value this reports:
 *
 *   `undefined`    leave the stored secret exactly as it is (nothing is sent)
 *   `''`           clear it
 *   anything else  replace it with that
 *
 * The same field exists in the Settings screen for credentials. It belongs in
 * `design/` now that two screens need it; see NOTES.md.
 */

import { Eye, EyeOff, Trash, Undo2 } from 'lucide-react';
import { useState } from 'react';
import { Button } from '@/design/Button';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { StatusDot } from '@/design/StatusDot';
import styles from './Monitoring.module.css';

export interface SecretFieldProps {
  label: string;
  help?: string;
  /** Whether the vault already holds this secret. */
  isSet: boolean;
  /** What will be sent: see the note above. */
  value: string | undefined;
  onChange: (value: string) => void;
  /** Go back to leaving the stored secret untouched. */
  onReset: () => void;
  placeholder?: string;
}

type Mode = 'stored' | 'editing' | 'clearing';

export function SecretField({ label, help, isSet, value, onChange, onReset, placeholder }: SecretFieldProps) {
  const [mode, setMode] = useState<Mode>('stored');
  const [visible, setVisible] = useState(false);

  const keep = () => {
    setMode('stored');
    setVisible(false);
    onReset();
  };

  if (mode === 'clearing' && value === '') {
    return (
      <Field label={label} hint={help}>
        <div className={styles.secret}>
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
        <div className={styles.secret}>
          <span className={styles.cell}>
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
      hint={isSet ? `${help ? `${help} ` : ''}The stored value is replaced when you save.` : help}
    >
      <div className={styles.secretEdit}>
        <Input
          autoComplete="new-password"
          aria-label={label}
          type={visible ? 'text' : 'password'}
          placeholder={placeholder}
          value={value ?? ''}
          onChange={(event) => {
            // An empty box means "I changed my mind", not "clear the secret".
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
        <Button size="sm" variant="ghost" icon={Undo2} onClick={keep}>
          {isSet ? 'Keep the stored one' : 'Cancel'}
        </Button>
      </div>
    </Field>
  );
}
