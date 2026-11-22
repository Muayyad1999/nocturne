/**
 * A customer's or an invoice's credit notes, as lines.
 *
 * Every note is shown, void ones included — a credit note is never deleted —
 * and each says the four things somebody checking a balance asks of it: how
 * much it writes off, why, when it was agreed, and where it stands. A void
 * note keeps its line with the reason it was withdrawn, and its amount is
 * muted, because it no longer counts in any figure.
 *
 * Dates are days of the business's calendar (`calendar.ts`): a note is a new
 * record, not a date on an invoice, so it is the setting's offset that
 * applies even inside an invoice written under an older one.
 */

import type { ReactNode } from 'react';
import type { CreditNote } from '@/api/types/isp';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { dayText, type Offset } from './calendar';
import { creditActions, creditStateWords, standingText } from './credit-notes';
import { Amount, Words } from './Figures';
import styles from './isp.module.css';

export interface CreditNoteListProps {
  credits: readonly CreditNote[];
  /** The business's calendar, for the dates. */
  calendar: Offset;
  /** What to say when there are none. */
  empty: ReactNode;
  /**
   * Say where each note stands. Left out inside an invoice's own drawer,
   * where every note stands against that invoice.
   */
  standing?: boolean;
  /** The number of an invoice by its id, where the caller has it. */
  invoiceNumber?: (invoiceId: string) => string | null | undefined;
  onOpenInvoice?: (invoiceId: string) => void;
  onVoid: (credit: CreditNote) => void;
  onAllocate?: (credit: CreditNote) => void;
}

export function CreditNoteList({
  credits,
  calendar,
  empty,
  standing = false,
  invoiceNumber,
  onOpenInvoice,
  onVoid,
  onAllocate,
}: CreditNoteListProps) {
  if (credits.length === 0) {
    return (
      <p className={styles.note}>
        <Words>{empty}</Words>
      </p>
    );
  }
  return (
    <table className={styles.lines}>
      <thead>
        <tr>
          <th>Credit note</th>
          <th>Writes off</th>
          <th aria-label="Actions" />
        </tr>
      </thead>
      <tbody>
        {credits.map((credit) => {
          const actions = creditActions(credit);
          const words = creditStateWords(credit);
          const number = credit.invoice_id ? invoiceNumber?.(credit.invoice_id) : undefined;
          return (
            <tr key={credit.id}>
              <td>
                <span className={styles.lineDescription}>
                  <span className={styles.amount}>{credit.number}</span> · {credit.reason}
                </span>
                <span className={styles.lineSubtitle}>
                  agreed {dayText(credit.at, calendar)}
                  {credit.allocated_at !== null &&
                    ` · set against the invoice on ${dayText(credit.allocated_at, calendar)}`}
                  {credit.note === '' ? '' : ` · ${credit.note}`}
                </span>
                {standing && (
                  <span className={styles.lineSubtitle}>
                    {credit.invoice_id && onOpenInvoice && credit.state === 'issued' ? (
                      <button
                        type="button"
                        className={styles.rowLink}
                        onClick={() => onOpenInvoice(credit.invoice_id ?? '')}
                      >
                        {standingText(credit, number)}
                      </button>
                    ) : (
                      standingText(credit, number)
                    )}
                  </span>
                )}
                {credit.state === 'void' && (
                  <span className={styles.lineSubtitle}>
                    void since {dayText(credit.voided_at, calendar)}: {credit.void_reason}
                  </span>
                )}
              </td>
              <td>
                <Amount money={credit.amount} muted={credit.state === 'void'} />
                <span className={styles.lineSubtitle}>
                  <Badge tone={words.tone} variant="outline">
                    {words.label}
                  </Badge>
                </span>
              </td>
              <td>
                <span className={styles.inlineGroup}>
                  {actions.allocate && onAllocate && (
                    <Button
                      size="sm"
                      aria-label={`Set ${credit.number} against an invoice`}
                      onClick={() => onAllocate(credit)}
                    >
                      Set against an invoice…
                    </Button>
                  )}
                  {actions.void && (
                    <Button
                      size="sm"
                      aria-label={`Void ${credit.number}`}
                      onClick={() => onVoid(credit)}
                    >
                      Void…
                    </Button>
                  )}
                </span>
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
