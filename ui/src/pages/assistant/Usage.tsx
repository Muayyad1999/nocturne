import { Coins } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useRpc } from '@/api/hooks';
import type { AiUsage } from '@/api/types/ai';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Panel } from '@/design/Panel';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { Stat } from '@/design/Stat';
import { Table, type TableColumn } from '@/design/Table';
import { formatCount } from '@/lib/format';
import styles from './Assistant.module.css';

const DAY = 86_400_000;

const RANGES: { value: string; label: string; since: () => number }[] = [
  { value: 'today', label: 'Today', since: () => new Date().setHours(0, 0, 0, 0) },
  { value: '7d', label: 'Last 7 days', since: () => Date.now() - 7 * DAY },
  { value: '30d', label: 'Last 30 days', since: () => Date.now() - 30 * DAY },
  { value: 'all', label: 'Everything', since: () => 0 },
];

type Row = AiUsage['by'][number];

/**
 * What the assistant has cost. Tokens, not money: Nocturne does not know what
 * anyone pays per token, and a made-up price would be worse than none.
 */
export function Usage() {
  const [range, setRange] = useState('7d');
  // Fixed when the period changes, not on every render: `since` is part of
  // the query's key, and a moving key would refetch for ever.
  const since = useMemo(
    () => (RANGES.find((item) => item.value === range) ?? RANGES[1]!).since(),
    [range],
  );
  const usage = useRpc<AiUsage>('ai.usage', { since }, { invalidateOn: ['ai.chats.changed'] });

  const columns: TableColumn<Row>[] = [
    { id: 'model', header: 'Model', mono: true, required: true, cell: (row) => row.model },
    {
      id: 'turns',
      header: 'Turns',
      align: 'right',
      sortValue: (row) => row.turns,
      cell: (row) => formatCount(row.turns),
    },
    {
      id: 'input',
      header: 'Sent',
      align: 'right',
      sortValue: (row) => row.input_tokens,
      cell: (row) => formatCount(row.input_tokens),
    },
    {
      id: 'cached',
      header: 'From cache',
      align: 'right',
      sortValue: (row) => row.cache_read_tokens,
      cell: (row) => formatCount(row.cache_read_tokens),
    },
    {
      id: 'output',
      header: 'Written',
      align: 'right',
      sortValue: (row) => row.output_tokens,
      cell: (row) => formatCount(row.output_tokens),
    },
  ];

  if (usage.isPending) {
    return (
      <div className={styles.loading} aria-busy="true" aria-label="Loading usage">
        <Skeleton width="30%" />
        <Skeleton height={72} shape="block" />
      </div>
    );
  }
  if (usage.error) {
    return (
      <ErrorState
        title="Usage could not be read"
        error={usage.error}
        onRetry={() => void usage.refetch()}
      />
    );
  }

  const { totals, by } = usage.data;

  return (
    <div className={styles.usagePage}>
      <div className={styles.usageHead}>
        <Select
          size="sm"
          aria-label="Period"
          options={RANGES.map(({ value, label }) => ({ value, label }))}
          value={range}
          onChange={(event) => setRange(event.target.value)}
        />
      </div>
      {totals.turns === 0 ? (
        <EmptyState
          icon={Coins}
          title="Nothing has been asked yet"
          description="Every answer records what it cost in tokens, per model, so you can see where the budget goes."
        />
      ) : (
        <>
          <div className={styles.stats}>
            <Stat label="Turns" value={formatCount(totals.turns)} />
            <Stat label="Tokens sent" value={formatCount(totals.input_tokens)} />
            <Stat label="From cache" value={formatCount(totals.cache_read_tokens)} />
            <Stat label="Tokens written" value={formatCount(totals.output_tokens)} />
          </div>
          <Panel title="By model" padding="none">
            <Table
              id="assistant-usage"
              aria-label="Tokens by model"
              columns={columns}
              rows={by}
              rowKey={(row) => `${row.provider_id}:${row.model}`}
              empty={null}
              defaultSort={{ columnId: 'input', direction: 'desc' }}
            />
          </Panel>
        </>
      )}
    </div>
  );
}
