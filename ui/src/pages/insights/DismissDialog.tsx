/**
 * Dismissing a finding: for how long, and why.
 *
 * A dismissal is a decision about the network that somebody will have to
 * understand later, so the reason is asked for and kept with the finding, and
 * the dialog says plainly what happens when the time runs out: the scan finds
 * it again and it comes back marked as a repeat, not as a new discovery.
 */

import { useEffect, useState } from 'react';
import type { Insight } from '@/api/types/insights';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Select } from '@/design/Select';
import { Textarea } from '@/design/Textarea';
import { toast } from '@/design/Toast';
import { formatCount } from '@/lib/format';
import { formatDate } from '@/lib/time';
import { useDismiss } from './data';
import styles from './Findings.module.css';

/** `forever` is the backend's "no `days`": a dismissal with no end. */
const PERIODS = [
  { value: '7', label: '7 days' },
  { value: '30', label: '30 days' },
  { value: '90', label: '90 days' },
  { value: 'forever', label: 'For good' },
];

export interface DismissDialogProps {
  insight: Insight | null;
  onClose: () => void;
}

export function DismissDialog({ insight, onClose }: DismissDialogProps) {
  const [period, setPeriod] = useState('30');
  const [note, setNote] = useState('');
  const dismiss = useDismiss();

  // Each finding is dismissed on its own terms: no reason carried over.
  useEffect(() => {
    if (insight) {
      setPeriod('30');
      setNote('');
    }
  }, [insight?.id]);

  if (!insight) return null;

  const days = period === 'forever' ? undefined : Number(period);
  const until = days === undefined ? null : Date.now() + days * 86_400_000;

  const submit = () => {
    dismiss.mutate(
      {
        id: insight.id,
        ...(days === undefined ? {} : { days }),
        ...(note.trim() === '' ? {} : { note: note.trim() }),
      },
      {
        onSuccess: () => {
          toast.info('Finding dismissed', {
            description:
              until === null
                ? 'It will stay out of the open list until somebody reopens it.'
                : `It comes back on ${formatDate(until)} if the scan still finds it.`,
          });
          onClose();
        },
      },
    );
  };

  return (
    <Dialog
      open
      onClose={onClose}
      title="Dismiss this finding"
      // The title is on the card the button was pressed on; what this needs
      // to be sure of is which device and which port.
      description={`${insight.device} ${insight.subject}`}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" loading={dismiss.isPending} onClick={submit}>
            Dismiss
          </Button>
        </>
      }
    >
      <div className={styles.dismissForm}>
        <Field
          label="For how long"
          hint={
            until === null
              ? 'Scans will keep finding it and keep leaving it alone until somebody reopens it.'
              : `Until ${formatDate(until)}. If the scan still finds it then, it comes back — as a repeat (occurrence ${formatCount(insight.occurrences + 1)}), not as a new discovery.`
          }
        >
          <Select
            options={PERIODS}
            value={period}
            onChange={(event) => setPeriod(event.target.value)}
          />
        </Field>
        <Field
          label="Reason"
          hint="Kept with the finding and written to the audit log. The next person to see this will read it."
        >
          <Textarea
            value={note}
            rows={3}
            placeholder="Known, waiting for the maintenance window on the 14th"
            onChange={(event) => setNote(event.target.value)}
          />
        </Field>
        {dismiss.error && <p className={styles.formError}>{dismiss.error.message}</p>}
      </div>
    </Dialog>
  );
}
