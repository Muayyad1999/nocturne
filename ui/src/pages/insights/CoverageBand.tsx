/**
 * What the last scan examined — above the findings, on every view.
 *
 * This band is the difference between a tool an engineer trusts and one they
 * stop believing. It answers four questions without being asked: when did
 * this run, how much of the network did it reach, how many answers are
 * missing, and why — in the backend's own words (`CoverageReason`), never
 * paraphrased and never rounded away.
 */

import { EyeOff, ScanSearch } from 'lucide-react';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { Skeleton } from '@/design/Skeleton';
import { StatusPill } from '@/design/StatusPill';
import { TimeAgo } from '@/design/TimeAgo';
import { Tooltip } from '@/design/Tooltip';
import { formatCount, formatDuration, plural } from '@/lib/format';
import { coverageTotals, freshness, reasonTotals, word, type ReasonTotal } from './insight';
import type { Scan } from './useScan';
import { indexDevices, useCoverage, useDevices, useInsightsSettings } from './data';
import styles from './Coverage.module.css';

export interface CoverageBandProps {
  /** Opens the Coverage view, where every check is accounted for. */
  onOpen: () => void;
  scan: Scan;
}

/** How many reasons fit in the band before the rest are behind the link. */
const REASONS_SHOWN = 3;

/** The band is a landmark: it is named once, for every state it can be in. */
const LABEL = 'Coverage of the last scan';

export function CoverageBand({ onOpen, scan }: CoverageBandProps) {
  const coverage = useCoverage();
  const devices = useDevices();
  const settings = useInsightsSettings();

  if (coverage.isPending) {
    return (
      <section className={styles.band} aria-label={LABEL}>
        <Skeleton shape="block" height={64} />
      </section>
    );
  }

  if (coverage.error) {
    return (
      <section className={styles.band} aria-label={LABEL}>
        <ErrorState
          size="compact"
          title="The coverage of the last scan could not be read"
          error={coverage.error}
          onRetry={() => void coverage.refetch()}
        />
      </section>
    );
  }

  if (!coverage.data) {
    return (
      <section className={styles.band} data-tone="idle" aria-label={LABEL}>
        <div className={styles.headline}>
          <Icon icon={ScanSearch} size={16} />
          <strong>Nothing has been examined yet</strong>
          <span className={styles.detail}>
            Until a scan has run, this screen can tell you neither that the network is healthy nor
            that it is not. A scan reads what has already been collected; it does not connect to a
            device.
          </span>
        </div>
        {/* No button here: the screen's one primary action is in the toolbar,
            and the empty state below it offers the same scan. */}
      </section>
    );
  }

  const data = coverage.data;
  const totals = coverageTotals(data);
  const reasons = reasonTotals(data);
  const age = freshness(data.scanned_at);
  const index = indexDevices(devices.data ?? [], settings.data?.include_lab ?? true);
  const uncovered = devices.data === undefined ? 0 : Math.max(0, index.scannable - data.devices);

  return (
    <section className={styles.band} data-tone={age} aria-label={LABEL}>
      <div className={styles.headline}>
        <Icon icon={ScanSearch} size={16} />
        <strong>
          {formatCount(totals.checks)} checks over {plural(totals.devices, 'device')}
        </strong>
        <span className={styles.detail}>
          scanned <TimeAgo at={data.scanned_at} /> in {formatDuration(data.took_ms)} ·{' '}
          {plural(data.links, 'link')} from{' '}
          {data.link_source === 'topology' ? 'the topology' : 'collected neighbours'}
        </span>
        {age !== 'fresh' && (
          <StatusPill tone={age === 'stale' ? 'crit' : 'warn'}>
            {age === 'stale' ? 'Over a day old' : 'Over an hour old'}
          </StatusPill>
        )}
      </div>

      <dl className={styles.tiles}>
        <Tile
          tone="crit"
          value={formatCount(totals.findings)}
          label={word(totals.findings, 'finding')}
          title="Problems this scan found, each with the values it was decided from."
        />
        <Tile
          tone="ok"
          value={formatCount(totals.evaluated)}
          label="evaluated"
          title="Device-by-check answers the scan actually reached: it looked, and this is what it saw."
        />
        <Tile
          tone="idle"
          value={formatCount(totals.skipped)}
          label="not evaluated"
          title="Device-by-check answers the scan could not reach. Not a pass: no verdict at all."
        />
        <Tile
          tone="neutral"
          value={formatCount(totals.checksOff)}
          label="checks off"
          title="Checks that are switched off, so they said nothing about any device."
        />
      </dl>

      {totals.skipped > 0 && (
        <div className={styles.reasons}>
          <h3 className={styles.reasonsTitle}>
            <Icon icon={EyeOff} size={13} />
            Not examined, and why
          </h3>
          <ul className={styles.reasonList}>
            {reasons.slice(0, REASONS_SHOWN).map((reason) => (
              <ReasonRow key={reason.reason} reason={reason} />
            ))}
          </ul>
        </div>
      )}

      <div className={styles.notes}>
        {totals.skipped === 0 && totals.checksOn > 0 && (
          <p className={styles.note} data-tone="ok">
            Every enabled check reached a verdict for every device in this scan.
          </p>
        )}
        {uncovered > 0 && (
          <p className={styles.note} data-tone="warn">
            The last scan covered {formatCount(data.devices)} of {formatCount(index.scannable)}{' '}
            devices in the inventory. The other {formatCount(uncovered)}{' '}
            {word(uncovered, 'was', 'were')} not re-examined, so anything on{' '}
            {uncovered === 1 ? 'it' : 'them'} is as old as its own last scan.{' '}
            <button type="button" className={styles.inlineAction} onClick={() => scan.start()}>
              Scan every device
            </button>
          </p>
        )}
        {index.labExcluded > 0 && (
          <p className={styles.note} data-tone="idle">
            {plural(index.labExcluded, 'simulated device')}{' '}
            {word(index.labExcluded, 'is', 'are')} left out of scans by the{' '}
            <code>include_lab</code> setting.
          </p>
        )}
        {data.missing_sources.map((source) => (
          <p key={source} className={styles.note} data-tone="idle">
            A feature this check wanted is not in this build — {source}
          </p>
        ))}
      </div>

      <div className={styles.bandActions}>
        <Button size="sm" variant="secondary" onClick={onOpen}>
          Account for all {formatCount(totals.checks)} checks
        </Button>
      </div>
    </section>
  );
}

function Tile({
  tone,
  value,
  label,
  title,
}: {
  tone: 'crit' | 'ok' | 'idle' | 'neutral';
  value: string;
  label: string;
  title: string;
}) {
  return (
    <Tooltip content={title}>
      {/* The label is the term and the number the description; CSS puts the
          number first, where the eye wants it. */}
      <div className={styles.tile} data-tone={tone}>
        <dt className={styles.tileLabel}>{label}</dt>
        <dd className={styles.tileValue}>{value}</dd>
      </div>
    </Tooltip>
  );
}

export function ReasonRow({ reason }: { reason: ReasonTotal }) {
  return (
    <li className={styles.reason}>
      <Badge tone="idle" variant="outline">
        {plural(reason.devices, 'device')}
      </Badge>
      {/* The backend's sentence, exactly as it gave it. */}
      <span className={styles.reasonText}>{reason.reason}</span>
      <span className={styles.reasonChecks}>
        {reason.checks
          .slice(0, 3)
          .map((check) => check.check)
          .join(', ')}
        {reason.checks.length > 3 && ` +${formatCount(reason.checks.length - 3)}`}
      </span>
    </li>
  );
}
