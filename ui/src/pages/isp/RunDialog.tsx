/**
 * Billing a period.
 *
 * `isp.invoice.run` previews unless it is explicitly told `dry_run: false`,
 * and the preview writes nothing at all — every row comes back with
 * `invoice_id: null`. That is the whole shape of this dialog: it can only
 * preview until there is a preview to act on, and the preview is labelled as
 * one in a block that cannot be mistaken for an outcome. Issuing is a second,
 * separate act with a typed confirmation, because an issued invoice is frozen
 * for good: a wrong one is voided with a reason and reissued, which is a
 * trail, not an undo.
 *
 * Two things the preview shows that a total would hide:
 *
 * * **what it comes to, per currency** — customers billed in dinars and
 *   customers billed in dollars are two figures, and the run does not add
 *   them;
 * * **everybody it leaves out, and why** — a monthly plan in a seven-day
 *   window is not billed at all, because nothing here works out what a part
 *   of a monthly period costs on its own. Those are not zeroes, and each one
 *   says what to do instead.
 */

import { Play } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import type { RunResult } from '@/api/types/isp';
import { Button } from '@/design/Button';
import { confirm, Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Select } from '@/design/Select';
import { toast } from '@/design/Toast';
import { formatCount, plural } from '@/lib/format';
import { formatDate } from '@/lib/time';
import { recentMonths, runHasWork, runOutcome, windowOf } from './billing-rows';
import { useCustomers, usePreviewRun, useRunBilling } from './data';
import { Amount, Figures, Words } from './Figures';
import { totalsLine } from './money';
import styles from './isp.module.css';

/** How many customers are read to put names on the preview's rows. */
const NAMES = 2_000;

export interface RunDialogProps {
  open: boolean;
  onClose: () => void;
}

export function RunDialog({ open, onClose }: RunDialogProps) {
  const months = useMemo(() => recentMonths(Date.now()), []);
  const [month, setMonth] = useState(months[1]?.value ?? months[0]?.value ?? '');
  const [result, setResult] = useState<RunResult | null>(null);

  const preview = usePreviewRun();
  const run = useRunBilling();
  const resetPreview = preview.reset;
  const resetRun = run.reset;
  const customers = useCustomers({ limit: NAMES });

  const names = useMemo(() => {
    const index = new Map<string, string>();
    for (const customer of customers.data?.customers ?? []) index.set(customer.id, customer.name);
    return index;
  }, [customers.data]);

  useEffect(() => {
    if (!open) return;
    setResult(null);
    resetPreview();
    resetRun();
  }, [open, resetPreview, resetRun]);

  const window = windowOf(month);
  const outcome = result ? runOutcome(result) : null;
  const error = preview.error ?? run.error;
  const name = (id: string) => names.get(id) ?? id;

  const askPreview = () => {
    if (!window) return;
    preview.mutate(
      { period_start: window.start, period_end: window.end, dry_run: true },
      { onSuccess: setResult },
    );
  };

  const write = (issue: boolean) => {
    if (!window) return;
    void (async () => {
      const totals = result ? totalsLine(result.totals) : '';
      const ok = await confirm(
        issue
          ? {
              title: `Issue ${plural(result?.invoices.length ?? 0, 'invoice')}?`,
              description: `Each one is given its number and frozen: nothing recalculates an issued invoice. A wrong one is voided with a reason and reissued, which stays in the trail. This run comes to ${totals}.`,
              confirmLabel: 'Issue them',
              confirmText: 'issue',
            }
          : {
              title: `Create ${plural(result?.invoices.length ?? 0, 'draft')}?`,
              description: `Drafts can still be changed and are not issued to anybody. This run comes to ${totals}.`,
              confirmLabel: 'Create the drafts',
            },
      );
      if (!ok) return;
      run.mutate(
        { period_start: window.start, period_end: window.end, dry_run: false, issue },
        {
          onSuccess: (written) => {
            setResult(written);
            toast.success(runOutcome(written).headline, { description: written.note });
          },
        },
      );
    })();
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="lg"
      title="Bill a period"
      description="One invoice per active customer, out of what their subscriptions say they agreed to — not out of today's tariffs. It previews first, and nothing is written until you say so."
      footer={
        <>
          <Button onClick={onClose}>Close</Button>
          <Button
            icon={Play}
            loading={preview.isPending}
            disabled={window === null}
            onClick={askPreview}
          >
            {result === null ? 'Preview' : 'Preview again'}
          </Button>
          <Button
            loading={run.isPending && !run.variables?.issue}
            disabled={!runHasWork(result) || outcome?.preview !== true}
            onClick={() => write(false)}
          >
            Create drafts
          </Button>
          <Button
            variant="primary"
            loading={run.isPending && run.variables?.issue === true}
            disabled={!runHasWork(result) || outcome?.preview !== true}
            onClick={() => write(true)}
          >
            Create and issue
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        <Field
          label="Period"
          hint={
            window
              ? `${formatDate(window.start)} up to, but not including, ${formatDate(window.end)}. A subscription is only billed when the window is one whole period of its kind — a monthly plan needs 28 to 31 days.`
              : 'Choose a month.'
          }
        >
          <Select
            value={month}
            onChange={(event) => {
              setMonth(event.target.value);
              setResult(null);
            }}
            options={months}
          />
        </Field>

        {error && <p className={styles.error}>{error.message}</p>}

        {result && outcome && (
          <div className={outcome.preview ? styles.preview : styles.written}>
            <p className={styles.previewTitle}>{outcome.headline}</p>
            <p className={styles.note}>{outcome.note}</p>
            <div className={styles.inlineGroup}>
              <span className={styles.statLabel}>Comes to</span>
              <Figures totals={outcome.totals} layout="inline" empty="nothing at all" />
            </div>
            <p className={styles.quiet}>
              {plural(outcome.invoices, 'customer')} would be billed
              {outcome.skipped > 0 ? `, ${formatCount(outcome.skipped)} left out` : ''}
              {outcome.skippedSubscriptions > 0
                ? `, and ${plural(outcome.skippedSubscriptions, 'subscription')} not charged on an invoice that was still drafted`
                : ''}
              .
            </p>
          </div>
        )}

        {result && result.invoices.length > 0 && (
          <div className={styles.scroll}>
            <table className={styles.lines}>
              <thead>
                <tr>
                  <th>Customer</th>
                  <th>Lines</th>
                  <th>Total</th>
                </tr>
              </thead>
              <tbody>
                {result.invoices.map((row) => (
                  <tr key={row.customer_id}>
                    <td>
                      <span className={styles.lineDescription}>{name(row.customer_id)}</span>
                      {row.number !== null && (
                        <span className={styles.lineSubtitle}>invoice {row.number}</span>
                      )}
                      {row.skipped_subscriptions.map((skipped) => (
                        <span key={skipped.subscription_id} className={styles.lineSubtitle}>
                          {skipped.tariff_name} was not charged: {skipped.reason}
                        </span>
                      ))}
                    </td>
                    <td>{formatCount(row.lines)}</td>
                    <td>
                      <Amount money={row.total} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}

        {result && result.skipped.length > 0 && (
          <div>
            <p className={styles.note}>
              Left out of this run. These are not zeroes — nothing was billed for them, and each
              says why:
            </p>
            <div className={styles.scroll}>
              <table className={`${styles.lines} ${styles.reasons}`}>
                <tbody>
                  {result.skipped.map((skip) => (
                    <tr key={skip.customer_id}>
                      <td>
                        <span className={styles.lineDescription}>{name(skip.customer_id)}</span>
                        <span className={styles.lineSubtitle}>{skip.reason}</span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        )}

        {result === null && (
          <p className={styles.quiet}>
            <Words>nothing has been asked for yet — a preview reads the ledger and writes nothing</Words>
          </p>
        )}
      </div>
    </Dialog>
  );
}
