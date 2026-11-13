/**
 * Every change Nocturne knows about, newest first.
 *
 * The list is the waiting room: what is in draft, what is waiting for
 * somebody to read it, what is running right now and what has already
 * happened. The one primary action is **New change**; everything else is a
 * filter.
 */

import { GitPullRequestArrow, Plus } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router';
import { useRpc } from '@/api/hooks';
import type { ChangeSummary } from '@/api/types/changes';
import { registerCommand } from '@/app/CommandPalette/commands';
import { usePageHeader } from '@/app/Header';
import { useNavBadge } from '@/app/navBadges';
import { Page } from '@/app/Page';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { StatusPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { Tooltip } from '@/design/Tooltip';
import { EMPTY } from '@/lib/format';
import { ORIGIN_LABEL, plural, RISK_LABEL, riskTone, STATE_LABEL, stateTone } from './change';
import { NewChange } from './NewChange';
import styles from './ChangeList.module.css';

const STATE_OPTIONS = [
  { value: 'open', label: 'Open' },
  { value: '', label: 'All states' },
  { value: 'draft', label: 'Draft' },
  { value: 'planned', label: 'Waiting for approval' },
  { value: 'approved', label: 'Approved' },
  { value: 'applying', label: 'Applying' },
  { value: 'applied', label: 'Applied' },
  { value: 'partially_applied', label: 'Partly applied' },
  { value: 'failed', label: 'Failed' },
  { value: 'rolled_back', label: 'Rolled back' },
  { value: 'cancelled', label: 'Cancelled' },
];

export default function ChangeList() {
  const [state, setState] = useState('open');
  const [search, setSearch] = useState('');
  const [drafting, setDrafting] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);

  const changes = useRpc<ChangeSummary[]>(
    'changes.list',
    { state: state === '' ? undefined : state, limit: 200 },
    { invalidateOn: ['changes.changed'] },
  );
  // The badge counts everything waiting, not only what this filter shows.
  const waiting = useRpc<ChangeSummary[]>(
    'changes.list',
    { state: 'planned', limit: 200 },
    { invalidateOn: ['changes.changed'] },
  );

  usePageHeader({ title: 'Changes' });
  useNavBadge(
    'changes',
    waiting.data?.length ? { count: waiting.data.length, tone: 'warn', label: 'changes waiting for approval' } : null,
  );

  useEffect(
    () =>
      registerCommand({
        id: 'changes.new',
        title: 'New change',
        group: 'Changes',
        keywords: ['configure', 'plan', 'apply'],
        icon: Plus,
        run: () => setDrafting(true),
      }),
    [],
  );

  useEffect(
    () =>
      registerCommand({
        id: 'changes.search',
        title: 'Search changes',
        group: 'Changes',
        keywords: ['filter', 'find'],
        shortcut: '/',
        run: () => searchRef.current?.focus(),
      }),
    [],
  );

  const rows = useMemo(() => {
    const all = changes.data ?? [];
    const needle = search.trim().toLowerCase();
    if (needle === '') return all;
    return all.filter((change) =>
      [change.title, change.description, change.ticket ?? '', ...change.devices.map((device) => device.name)].some(
        (field) => field.toLowerCase().includes(needle),
      ),
    );
  }, [changes.data, search]);

  const columns: TableColumn<ChangeSummary>[] = useMemo(
    () => [
      {
        id: 'title',
        header: 'Change',
        required: true,
        sortValue: (row) => row.title,
        cell: (row) => (
          <span className={styles.titleCell}>
            {/* The description is a tooltip, not a second column: at 28px rows
                it only ever stole the width the title needed. */}
            <Tooltip content={row.description === '' ? row.title : row.description}>
              <Link to={`/changes/${row.id}`} className={styles.title}>
                {row.title}
              </Link>
            </Tooltip>
            {row.origin === 'assistant' && (
              <Tooltip content="Drafted by the assistant. Nothing is applied until a person approves it.">
                <Badge tone="ai">AI</Badge>
              </Tooltip>
            )}
          </span>
        ),
      },
      {
        id: 'state',
        header: 'State',
        width: 170,
        sortValue: (row) => row.state,
        cell: (row) => <StatusPill tone={stateTone(row.state)}>{STATE_LABEL[row.state]}</StatusPill>,
      },
      {
        id: 'devices',
        header: 'Devices',
        width: 220,
        sortValue: (row) => row.devices.length,
        cell: (row) => (
          <Tooltip content={row.devices.map((device) => device.name).join(', ')}>
            <span className={styles.devices}>
              {row.devices.length === 1 ? row.devices[0]?.name : plural(row.devices.length, 'device')}
            </span>
          </Tooltip>
        ),
      },
      {
        id: 'risk',
        header: 'Risk',
        width: 120,
        sortValue: (row) => row.risk ?? '',
        cell: (row) =>
          row.risk ? <Badge tone={riskTone(row.risk)}>{RISK_LABEL[row.risk]}</Badge> : <span className={styles.muted}>{EMPTY}</span>,
      },
      {
        id: 'protected',
        header: 'Protected',
        width: 110,
        align: 'right',
        sortValue: (row) => row.protected_hits,
        cell: (row) =>
          row.protected_hits > 0 ? (
            <Badge tone="crit">{row.protected_hits}</Badge>
          ) : (
            <span className={styles.muted}>{EMPTY}</span>
          ),
      },
      {
        id: 'origin',
        header: 'Origin',
        width: 120,
        defaultHidden: true,
        sortValue: (row) => row.origin,
        cell: (row) => <span className={styles.muted}>{ORIGIN_LABEL[row.origin]}</span>,
      },
      {
        id: 'ticket',
        header: 'Reference',
        width: 130,
        mono: true,
        sortValue: (row) => row.ticket ?? '',
        cell: (row) => row.ticket ?? <span className={styles.muted}>{EMPTY}</span>,
      },
      {
        id: 'created',
        header: 'Created',
        width: 130,
        sortValue: (row) => row.created_at,
        cell: (row) => <TimeAgo at={row.created_at} />,
      },
    ],
    [],
  );

  return (
    <Page>
      <Toolbar>
        <SearchInput
          ref={searchRef}
          className={styles.search}
          value={search}
          onChange={setSearch}
          aria-label="Search changes"
          placeholder="Search by title, reference or device"
        />
        <Select
          className={styles.state}
          aria-label="Filter by state"
          value={state}
          onChange={(event) => setState(event.target.value)}
          options={STATE_OPTIONS}
        />
        <ToolbarSpacer />
        <Button variant="primary" icon={Plus} onClick={() => setDrafting(true)}>
          New change
        </Button>
      </Toolbar>

      {changes.error ? (
        <ErrorState
          title="The changes could not be read"
          error={changes.error}
          onRetry={() => void changes.refetch()}
        />
      ) : (
        <Table
          id="changes.list"
          columns={columns}
          rows={rows}
          rowKey={(row) => row.id}
          loading={changes.isPending}
          density="compact"
          defaultSort={{ columnId: 'created', direction: 'desc' }}
          aria-label="Changes"
          fill
          empty={
            <EmptyState
              icon={GitPullRequestArrow}
              title={search !== '' ? 'No change matches' : state === 'open' ? 'Nothing is in flight' : 'No changes yet'}
              description={
                search !== ''
                  ? 'Nothing in this list matches that search.'
                  : 'A change takes what should happen on a set of devices, works out the exact commands, has them approved, and applies them with a backup, a verification and a way back.'
              }
              action={
                search === '' ? (
                  <Button variant="primary" icon={Plus} onClick={() => setDrafting(true)}>
                    New change
                  </Button>
                ) : (
                  <Button onClick={() => setSearch('')}>Clear the search</Button>
                )
              }
            />
          }
        />
      )}

      <NewChange open={drafting} onClose={() => setDrafting(false)} />
    </Page>
  );
}
