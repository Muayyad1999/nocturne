/**
 * Who owes what, and for how long — one aged report per currency, side by
 * side, because an amount owed in dinars and an amount owed in dollars are
 * two facts and adding them would be a third thing that is not true. The
 * engine returns them that way and has nowhere to put a total across them;
 * neither has this screen.
 *
 * The first band is "not yet due", and it is not coloured as late, because it
 * is not. A customer with nothing overdue has `oldest_days: null`, which is
 * written as a sentence rather than as nought days. What is overdue, and by
 * how many days, is the engine's reading in the business's calendar.
 *
 * Beside the debts, and never taken off them, is what stands in customers'
 * favour: invoices settled beyond their total. It has its own card per
 * currency, because a customer who owes on one invoice and is in credit on
 * another should not be chased as if the credit did not exist — and because
 * nobody has set the one against the other.
 *
 * And nothing here cuts anybody off. Arrears are a record: `nocturne-isp` has
 * no write path to a device, so the screen says who is behind and leaves the
 * decision where it belongs.
 */

import { Banknote } from 'lucide-react';
import { Link } from 'react-router';
import type { AgedReport, FavourReport } from './ageing';
import type { Bucket } from '@/api/types/isp';
import { Badge } from '@/design/Badge';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Panel } from '@/design/Panel';
import { ProgressBar } from '@/design/ProgressBar';
import { Skeleton } from '@/design/Skeleton';
import { plural } from '@/lib/format';
import {
  bucketMeaning,
  bucketTone,
  byCurrency,
  inFavour,
  oldestText,
  oldestTone,
  BOOKKEEPING_NOTE,
} from './ageing';
import { dayTimeText } from './calendar';
import { useArrears, useCalendar } from './data';
import { Amount, Words } from './Figures';
import { STATE_WORDS } from './records';
import styles from './isp.module.css';

export interface ArrearsProps {
  /** One customer's ageing, for their own page. */
  customerId?: string;
  /** Inside a customer's page the heading and the standing note are already there. */
  compact?: boolean;
}

export function Arrears({ customerId, compact = false }: ArrearsProps) {
  const arrears = useArrears(customerId);
  const calendar = useCalendar();

  if (arrears.isError) {
    return (
      <div className={compact ? undefined : styles.scrollPane}>
        <ErrorState
          size={compact ? 'compact' : 'page'}
          title="The arrears could not be read"
          error={arrears.error}
          onRetry={() => void arrears.refetch()}
        />
      </div>
    );
  }

  if (arrears.isPending) {
    return (
      <div className={compact ? undefined : styles.scrollPane}>
        <Skeleton shape="block" height={compact ? 120 : 260} />
      </div>
    );
  }

  const reports = byCurrency(arrears.data);
  // On a customer's own page their balance already says what is in their
  // favour; here it is the report's second half.
  const favour = compact ? [] : inFavour(arrears.data);

  if (reports.length === 0 && favour.length === 0) {
    const nothing = (
      <EmptyState
        icon={Banknote}
        size="compact"
        title="Nothing is owed"
        description="No issued invoice is short. This is a reading of the ledger at this moment, not a claim that nothing has been billed."
      />
    );
    return compact ? nothing : <div className={styles.scrollPane}>{nothing}</div>;
  }

  const body = (
    <>
      {!compact && (
        <p className={styles.note}>
          {BOOKKEEPING_NOTE}{' '}
          <span className={styles.quiet}>As of {dayTimeText(arrears.data.as_of, calendar)}.</span>
        </p>
      )}
      {reports.length === 0 && (
        <p className={styles.note}>
          <Words>nothing is owed: no issued invoice is short</Words>
        </p>
      )}
      <div className={styles.cards}>
        {reports.map((report) => (
          <CurrencyCard key={report.currency} report={report} compact={compact} />
        ))}
        {favour.map((report) => (
          <FavourCard key={report.currency} report={report} />
        ))}
      </div>
      {!compact && <p className={styles.quiet}>{arrears.data.note}</p>}
    </>
  );

  return compact ? body : <div className={styles.scrollPane}>{body}</div>;
}

function CurrencyCard({ report, compact }: { report: AgedReport; compact: boolean }) {
  const { report: aged, customers } = report;
  // On a customer's own page the figures are already above this, and the
  // customer is themselves: what is left to say is how old it is, so the
  // bands that have an invoice in them are the whole card.
  const bands = compact ? aged.buckets.filter((bucket) => bucket.invoices > 0) : aged.buckets;
  return (
    <Panel title={`Owed in ${aged.currency}`} padding="none">
      <div className={styles.panelBody}>
        {!compact && (
          <div className={styles.stats}>
            <div className={styles.statGroup}>
              <span className={styles.statLabel}>Owed</span>
              <Amount money={aged.total} className={styles.statValue} />
              <span className={styles.quiet}>
                {plural(aged.invoices, 'invoice')} · {plural(aged.customers, 'customer')}
              </span>
            </div>
            <div className={styles.statGroup}>
              <span className={styles.statLabel}>Not yet due</span>
              <Amount money={aged.not_yet_due} className={styles.statValue} />
              <span className={styles.quiet}>owed, and nobody is late</span>
            </div>
            <div className={styles.statGroup}>
              <span className={styles.statLabel}>Overdue</span>
              <Amount money={aged.overdue} className={styles.statValue} />
              <span className={styles.quiet}>the due day is over</span>
            </div>
          </div>
        )}

        <ul className={styles.buckets}>
          {bands.map((bucket) => (
            <BucketRow key={bucket.label} bucket={bucket} totalMinor={aged.total.minor} />
          ))}
        </ul>

        {!compact && (
          <table className={styles.lines}>
            <thead>
              <tr>
                <th>Customer</th>
                <th>Owed</th>
              </tr>
            </thead>
            <tbody>
              {customers.length === 0 && (
                <tr>
                  <td colSpan={2}>
                    <Words>nobody owes anything in this currency</Words>
                  </td>
                </tr>
              )}
              {customers.map((row) => (
                <tr key={`${row.customer_id}-${row.outstanding.currency}`}>
                  <td>
                    <Link to={`/isp/customer/${row.customer_id}`}>{row.customer_name}</Link>
                    <span className={styles.lineSubtitle}>
                      {plural(row.invoices, 'invoice')} ·{' '}
                      <Badge tone={STATE_WORDS[row.state].tone} variant="outline">
                        {STATE_WORDS[row.state].label}
                      </Badge>
                    </span>
                  </td>
                  <td>
                    <Amount money={row.outstanding} />
                    <span className={styles.lineSubtitle} data-tone={oldestTone(row.oldest_days)}>
                      {oldestText(row.oldest_days)}
                    </span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </Panel>
  );
}

/** What stands in customers' favour in one currency. Beside the debts, never off them. */
function FavourCard({ report }: { report: FavourReport }) {
  return (
    <Panel title={`In customers’ favour, in ${report.currency}`} padding="none">
      <div className={styles.panelBody}>
        <div className={styles.stats}>
          <div className={styles.statGroup}>
            <span className={styles.statLabel}>Settled beyond the total</span>
            <Amount money={report.total} className={styles.statValue} />
            <span className={styles.quiet}>
              paid or credited past what an invoice charged; not taken off anything owed
            </span>
          </div>
        </div>
        <table className={styles.lines}>
          <thead>
            <tr>
              <th>Customer</th>
              <th>In their favour</th>
            </tr>
          </thead>
          <tbody>
            {report.customers.map((row) => (
              <tr key={`${row.customer_id}-${row.overpaid.currency}`}>
                <td>
                  <Link to={`/isp/customer/${row.customer_id}`}>{row.customer_name}</Link>
                  <span className={styles.lineSubtitle}>
                    {plural(row.invoices, 'invoice')} ·{' '}
                    <Badge tone={STATE_WORDS[row.state].tone} variant="outline">
                      {STATE_WORDS[row.state].label}
                    </Badge>
                  </span>
                </td>
                <td>
                  <Amount money={row.overpaid} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </Panel>
  );
}

function BucketRow({ bucket, totalMinor }: { bucket: Bucket; totalMinor: number }) {
  // The bar is a proportion of this currency's own total, for the eye only:
  // the figure beside it is the amount, and no number is derived from this.
  const share = totalMinor === 0 ? 0 : (bucket.amount.minor / totalMinor) * 100;
  return (
    <li className={styles.bucket}>
      <span className={styles.bucketLabel} title={bucketMeaning(bucket)}>
        {bucket.label}
      </span>
      <ProgressBar
        kind="meter"
        value={share}
        tone={bucketTone(bucket)}
        size="sm"
        valueText={`${bucket.amount.text} ${bucket.amount.currency}`}
        aria-label={`${bucket.label}: ${bucket.amount.text} ${bucket.amount.currency}`}
      />
      <span className={styles.bucketCount}>
        <Amount money={bucket.amount} />
        <span className={styles.rowSub}>{plural(bucket.invoices, 'invoice')}</span>
      </span>
    </li>
  );
}
