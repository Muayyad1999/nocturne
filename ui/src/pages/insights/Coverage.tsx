/**
 * Coverage in full: every check of the last scan accounted for, device by
 * device, with the reason for every answer it could not give.
 *
 * The verdict column is the point of this screen. "3 findings", "Clear",
 * "Clear on 1 of 3", "Not evaluated" and "Off" are five different statements
 * and they are never shown as the same thing — `insight.ts` decides which one
 * a row is, in one place.
 */

import { EyeOff, Radar, ScanSearch } from 'lucide-react';
import { useMemo, useState } from 'react';
import type { CheckCoverage } from '@/api/types/insights';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { KeyValue } from '@/design/KeyValue';
import { Panel } from '@/design/Panel';
import { SearchInput } from '@/design/SearchInput';
import { Skeleton } from '@/design/Skeleton';
import { StatusPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { Tooltip } from '@/design/Tooltip';
import { formatCount, formatDuration, plural } from '@/lib/format';
import { formatTimestamp } from '@/lib/time';
import { ReasonRow } from './CoverageBand';
import { useCoverage } from './data';
import { categoryLabel, coverageTotals, reasonTotals, verdict, word } from './insight';
import styles from './Coverage.module.css';
import type { Scan } from './useScan';

export interface CoverageTabProps {
  scan: Scan;
}

export function CoverageTab({ scan }: CoverageTabProps) {
  const coverage = useCoverage();
  const [query, setQuery] = useState('');

  const rows = useMemo(() => {
    const all = coverage.data?.checks ?? [];
    const needle = query.trim().toLowerCase();
    if (needle === '') return all;
    return all.filter((row) =>
      [row.check, row.title, categoryLabel(row.category), ...row.reasons.map((r) => r.reason)].some(
        (field) => field.toLowerCase().includes(needle),
      ),
    );
  }, [coverage.data, query]);

  if (coverage.isPending) return <Skeleton shape="block" height={320} />;
  if (coverage.error) {
    return (
      <ErrorState
        title="The coverage report could not be read"
        error={coverage.error}
        onRetry={() => void coverage.refetch()}
      />
    );
  }
  if (!coverage.data) {
    return (
      <EmptyState
        icon={Radar}
        title="No scan has run yet"
        description="Coverage is the record of one scan: which checks reached a verdict for which devices, and why the others could not. There is nothing to account for until one has run."
        action={
          <Button variant="primary" icon={Radar} loading={scan.running} onClick={() => scan.start()}>
            Run the first scan
          </Button>
        }
      />
    );
  }

  const data = coverage.data;
  const totals = coverageTotals(data);
  const reasons = reasonTotals(data);

  const columns: TableColumn<CheckCoverage>[] = [
    {
      id: 'check',
      header: 'Check',
      required: true,
      sortValue: (row) => row.title,
      cell: (row) => (
        <span className={styles.checkCell}>
          <span className={styles.checkTitle}>{row.title}</span>
          <span className={styles.checkId}>{row.check}</span>
        </span>
      ),
    },
    {
      id: 'category',
      header: 'Category',
      width: 110,
      sortValue: (row) => categoryLabel(row.category),
      cell: (row) => categoryLabel(row.category),
    },
    {
      id: 'verdict',
      header: 'Verdict',
      width: 150,
      sortValue: (row) => verdict(row).label,
      cell: (row) => {
        const words = verdict(row);
        return (
          <Tooltip content={words.detail}>
            <StatusPill tone={words.tone}>{words.label}</StatusPill>
          </Tooltip>
        );
      },
    },
    {
      id: 'evaluated',
      header: 'Evaluated',
      width: 90,
      align: 'right',
      sortValue: (row) => row.evaluated,
      cell: (row) => formatCount(row.evaluated),
    },
    {
      id: 'skipped',
      header: 'Not evaluated',
      width: 110,
      align: 'right',
      sortValue: (row) => row.skipped,
      cell: (row) =>
        row.skipped === 0 ? (
          <span className={styles.zero}>0</span>
        ) : (
          <span className={styles.skipped}>{formatCount(row.skipped)}</span>
        ),
    },
    {
      id: 'findings',
      header: 'Findings',
      width: 80,
      align: 'right',
      sortValue: (row) => row.findings,
      cell: (row) =>
        row.findings === 0 ? (
          <span className={styles.zero}>0</span>
        ) : (
          <Badge tone="crit">{formatCount(row.findings)}</Badge>
        ),
    },
    {
      id: 'reasons',
      header: 'Why not',
      sortValue: (row) => row.reasons[0]?.reason ?? '',
      cell: (row) =>
        row.reasons.length === 0 ? (
          <span className={styles.zero}>—</span>
        ) : (
          <Tooltip
            content={row.reasons
              .map((reason) => `${reason.reason} (${formatCount(reason.devices)})`)
              .join(' · ')}
          >
            <span className={styles.reasonCell}>
              {row.reasons[0]?.reason}
              {row.reasons.length > 1 && (
                <span className={styles.more}> +{formatCount(row.reasons.length - 1)} more</span>
              )}
            </span>
          </Tooltip>
        ),
    },
  ];

  return (
    <div className={styles.tab}>
      <Panel title="The last scan" padding="md">
        <KeyValue
          items={[
            { label: 'Ran', value: formatTimestamp(data.scanned_at) },
            { label: 'Took', value: formatDuration(data.took_ms) },
            {
              label: 'Devices',
              value: plural(data.devices, 'device'),
            },
            {
              label: 'Links',
              value: `${formatCount(data.links)} from ${
                data.link_source === 'topology' ? 'the topology' : 'collected neighbours'
              }`,
            },
            {
              label: 'Checks',
              value: `${formatCount(totals.checksOn)} on, ${formatCount(totals.checksOff)} off`,
            },
            {
              label: 'Answers',
              value: `${formatCount(totals.evaluated)} evaluated, ${formatCount(totals.skipped)} not evaluated, of ${formatCount(totals.considered)} asked for`,
            },
            {
              label: 'Verdicts',
              // Only the answers this scan actually gave: "0 clear only where
              // they could look" is noise in a sentence that has to be read.
              value:
                [
                  totals.checksWithFindings > 0 &&
                    `${plural(totals.checksWithFindings, 'check')} found something`,
                  totals.checksClear > 0 && `${formatCount(totals.checksClear)} clear`,
                  totals.checksPartly > 0 &&
                    `${formatCount(totals.checksPartly)} clear only where ${word(totals.checksPartly, 'it', 'they')} could look`,
                  totals.checksNotEvaluated > 0 &&
                    `${formatCount(totals.checksNotEvaluated)} with no verdict at all`,
                  totals.checksOff > 0 && `${formatCount(totals.checksOff)} switched off`,
                ]
                  .filter((part) => typeof part === 'string')
                  .join(' · ') || 'none: no check was enabled',
            },
            {
              label: 'Missing features',
              value:
                data.missing_sources.length === 0 ? null : (
                  <ul className={styles.plainList}>
                    {data.missing_sources.map((source) => (
                      <li key={source}>{source}</li>
                    ))}
                  </ul>
                ),
            },
          ]}
        />
      </Panel>

      <Panel
        title="Not examined, and why"
        padding="md"
        actions={
          <span className={styles.panelCount}>
            {plural(totals.skipped, 'answer')} missing
          </span>
        }
      >
        {reasons.length === 0 ? (
          <p className={styles.clean}>
            Every enabled check reached a verdict for every one of the{' '}
            {plural(data.devices, 'device')} in this scan. Nothing was
            skipped.
          </p>
        ) : (
          <>
            <p className={styles.lead}>
              <Icon icon={EyeOff} size={13} /> These are the engine&rsquo;s own words for
              why it would not answer, grouped by how many device-by-check answers each accounts
              for. None of them means &ldquo;no problem&rdquo;.
            </p>
            <ul className={styles.reasonList}>
              {reasons.map((reason) => (
                <ReasonRow key={reason.reason} reason={reason} />
              ))}
            </ul>
          </>
        )}
      </Panel>

      <Panel
        title="Every check in this scan"
        padding="none"
        actions={
          <SearchInput
            value={query}
            onChange={setQuery}
            size="sm"
            aria-label="Search checks"
            placeholder="Search checks and reasons…"
          />
        }
      >
        <Table
          id="insights.coverage"
          columns={columns}
          rows={rows}
          rowKey={(row) => row.check}
          aria-label="Coverage of every check"
          density="comfortable"
          defaultSort={{ columnId: 'findings', direction: 'desc' }}
          rowActions={(row) => [
            {
              label: 'Run only this check, over everything',
              icon: ScanSearch,
              onSelect: () => scan.start({ checks: [row.check] }),
            },
          ]}
          empty={
            <EmptyState
              size="compact"
              icon={ScanSearch}
              title="No check matches"
              description="Every check of the last scan is listed here; this search finds none of them."
            />
          }
        />
      </Panel>
    </div>
  );
}
