/**
 * Making a lab: an empty canvas, or one of the simulator's own teaching labs.
 *
 * The examples are `labs.examples` — the simulator's, not this screen's — and
 * each one says what it is there to show in its own words.
 */

import { useState } from 'react';
import { useRpc } from '@/api/hooks';
import type { ExampleInfo } from '@/api/types/lab';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Skeleton } from '@/design/Skeleton';
import styles from './NewLabDialog.module.css';

export interface NewLabDialogProps {
  open: boolean;
  onClose: () => void;
  onCreate: (params: { name: string; example?: string }) => void;
  busy: boolean;
}

/** The id the backend uses for "nothing on the canvas". */
const EMPTY = '';

export function NewLabDialog({ open, onClose, onCreate, busy }: NewLabDialogProps) {
  const examples = useRpc<ExampleInfo[]>('labs.examples', undefined, { enabled: open });
  const [name, setName] = useState('');
  const [example, setExample] = useState(EMPTY);

  const chosen = examples.data?.find((item) => item.id === example);

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="lg"
      title="New lab"
      description="A lab is a network simulated in this process. Nothing in it ever touches a real device."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="primary"
            loading={busy}
            onClick={() =>
              onCreate({
                name: name.trim() || chosen?.name || 'New lab',
                ...(example === EMPTY ? {} : { example }),
              })
            }
          >
            Create
          </Button>
        </>
      }
    >
      <Field label="Name" hint="Lab names are unique: every node becomes a device called “lab/node”.">
        <Input
          data-autofocus
          value={name}
          placeholder={chosen?.name ?? 'New lab'}
          onChange={(event) => setName(event.target.value)}
        />
      </Field>

      <h3 className={styles.heading}>Start from</h3>
      {examples.isPending && <Skeleton height={120} shape="block" />}
      {examples.error && (
        <ErrorState
          title="The examples could not be read"
          error={examples.error}
          onRetry={() => void examples.refetch()}
          size="compact"
        />
      )}
      {examples.data && (
        <div className={styles.choices} role="radiogroup" aria-label="Start from">
          <button
            type="button"
            role="radio"
            aria-checked={example === EMPTY}
            className={styles.choice}
            data-selected={example === EMPTY || undefined}
            onClick={() => setExample(EMPTY)}
          >
            <span className={styles.choiceName}>An empty canvas</span>
            <span className={styles.choiceWhat}>
              Nothing on it. Add the devices you want and cable them yourself.
            </span>
          </button>
          {examples.data.map((item) => (
            <button
              key={item.id}
              type="button"
              role="radio"
              aria-checked={example === item.id}
              className={styles.choice}
              data-selected={example === item.id || undefined}
              onClick={() => setExample(item.id)}
            >
              <span className={styles.choiceName}>
                {item.name}
                <span className={styles.nodes}>{item.nodes} devices</span>
              </span>
              <span className={styles.choiceWhat}>{item.description}</span>
              {item.teaches.length > 0 && (
                <ul className={styles.teaches}>
                  {item.teaches.map((line) => (
                    <li key={line}>{line}</li>
                  ))}
                </ul>
              )}
            </button>
          ))}
        </div>
      )}
    </Dialog>
  );
}
