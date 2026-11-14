/**
 * Every incident, with the ones nobody has finished with first.
 *
 * The list is the way in, so it carries only what decides which incident to
 * open: how bad, what state, which devices, since when, who has it, and
 * whether its evidence has a hole in it. The prose lives on the incident's
 * own page.
 */

import { Plus, Siren } from 'lucide-react';
import { useState } from 'react';
import { Link } from 'react-router';
import { useHasMethod } from '@/api/hooks';
import type { IncidentHead, IncidentSeverity } from '@/api/types/ops';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { formatCount, plural } from '@/lib/format';
import { useIncidents } from './data';
import { NewIncidentDialog } from './NewIncident';
import styles from './Ops.module.css';
import {
  SEVERITY_LABEL,
  SEVERITY_MEANING,
  SEVERITY_TONE,
  STATE_LABEL,
  STATE_MEANING,
  STATE_TONE,
  instant,
} from './ops';

const STATES = [
  { value: 'unresolved', label: 'Unresolved' },
  { value: 'open', label: 'Open' },
  { value: 'mitigated', label: 'Mitigated' },
  { value: 'resolved', label: 'Resolved' },
  { value: 'any', label: 'All' },
];

const SEVERITIES = [
  { value: '', label: 'Any severity' },
  ...(['sev1', 'sev2', 'sev3', 'sev4'] as IncidentSeverity[]).map((severity) => ({
    value: severity,
    label: SEVERITY_LABEL[severity],
  })),
];

export interface IncidentListProps {
  onOpen: (id: string) => void;
}

export function IncidentList({ onOpen }: IncidentListProps) {
  const [state, setState] = useState('unresolved');
  const [severity, setSeverity] = useState('');
  const [query, setQuery] = useState('');
  const [opening, setOpening] = useState(false);
  // Opening an incident from an alert needs the alerting feature; by hand
  // does not, so the dialog offers what this build can do.
  const hasAlerts = useHasMethod('alerts.list');

  const incidents = useIncidents({
    state,
    severity: severity === '' ? undefined : (severity as IncidentSeverity),
    query: query.trim() === '' ? undefined : query.trim(),
    limit: 200,
  });

  const columns: readonly TableColumn<IncidentHead>[] = [
    {
      id: 'severity',
      header: 'Severity',
      width: 104,
      required: true,
      sortValue: (row) => row.severity,
      cell: (row) => (
        <StatusPill tone={SEVERITY_TONE[row.severity]} title={SEVERITY_MEANING[row.severity]}>
          {SEVERITY_LABEL[row.severity]}
        </StatusPill>
      ),
    },
    {
      id: 'title',
      header: 'Incident',
      required: true,
      sortValue: (row) => row.title,
      cell: (row) => (
        <Link className={styles.clip} to={`/ops/incidents?incident=${encodeURIComponent(row.id)}`} title={row.title}>
          {row.title}
        </Link>
      ),
    },
    {
      id: 'state',
      header: 'State',
      width: 116,
      sortValue: (row) => row.state,
      cell: (row) => (
        <StatusPill tone={STATE_TONE[row.state]} title={STATE_MEANING[row.state]}>
          {STATE_LABEL[row.state]}
        </StatusPill>
      ),
    },
    {
      id: 'devices',
      header: 'Devices',
      width: 200,
      cell: (row) => <Devices incident={row} />,
    },
    {
      id: 'started',
      header: 'Started',
      width: 110,
      align: 'right',
      sortValue: (row) => row.started_at,
      cell: (row) => <TimeAgo at={instant(row.started_at)} />,
    },
    {
      id: 'assignee',
      header: 'Who',
      width: 120,
      sortValue: (row) => row.assignee,
      cell: (row) =>
        row.assignee ? (
          <span className={styles.clip}>{row.assignee}</span>
        ) : (
          <span className={styles.muted}>unassigned</span>
        ),
    },
    {
      id: 'entries',
      header: 'Timeline',
      width: 130,
      align: 'right',
      sortValue: (row) => row.entries,
      cell: (row) => (
        <span className={styles.row}>
          <span>{plural(row.entries, 'entry', 'entries')}</span>
          {row.gaps > 0 && (
            <Badge
              tone="warn"
              variant="outline"
              title={`${plural(row.gaps, 'source')} could not be read, so this timeline may be missing entries.`}
            >
              {formatCount(row.gaps)} gap
            </Badge>
          )}
        </span>
      ),
    },
    {
      id: 'follow_ups',
      header: 'Follow-ups',
      width: 110,
      align: 'right',
      defaultHidden: true,
      sortValue: (row) => row.open_follow_ups,
      cell: (row) =>
        row.open_follow_ups > 0 ? formatCount(row.open_follow_ups) : <span className={styles.muted}>none open</span>,
    },
  ];

  const counts = incidents.data?.counts;

  return (
    <>
      <Toolbar>
        <SearchInput
          aria-label="Search incidents"
          value={query}
          onChange={setQuery}
          placeholder="Search titles, prose and timelines"
          className={styles.filter}
        />
        <Select
          aria-label="State"
          size="sm"
          className={styles.narrow}
          options={STATES}
          value={state}
          onChange={(event) => setState(event.target.value)}
        />
        <Select
          aria-label="Severity"
          size="sm"
          className={styles.narrow}
          options={SEVERITIES}
          value={severity}
          onChange={(event) => setSeverity(event.target.value)}
        />
        <ToolbarSpacer />
        <Button variant="primary" icon={Plus} onClick={() => setOpening(true)}>
          Open an incident
        </Button>
      </Toolbar>

      {counts && (
        <p className={styles.facts}>
          <span>
            <b>{formatCount(counts.by_state.open)}</b> open
          </span>
          <span>
            <b>{formatCount(counts.by_state.mitigated)}</b> mitigated
          </span>
          <span>
            <b>{formatCount(counts.by_state.resolved)}</b> resolved
          </span>
          <span className={styles.muted}>
            {plural(counts.total, 'incident')} on record · counted by the engine, not by this list
          </span>
        </p>
      )}

      {incidents.isError ? (
        <ErrorState
          title="The incidents could not be read"
          error={incidents.error}
          onRetry={() => void incidents.refetch()}
        />
      ) : (
        <Table
          id="ops.incidents"
          aria-label="Incidents"
          columns={columns}
          rows={incidents.data?.incidents ?? []}
          rowKey={(row) => row.id}
          density="compact"
          loading={incidents.isPending}
          defaultSort={{ columnId: 'started', direction: 'desc' }}
          onRowActivate={(row) => onOpen(row.id)}
          empty={
            <EmptyState
              size="compact"
              icon={Siren}
              title={
                query.trim() !== '' || severity !== '' || state !== 'unresolved'
                  ? 'No incident matches that'
                  : 'No incident is being worked on'
              }
              description={
                query.trim() !== '' || severity !== '' || state !== 'unresolved'
                  ? 'Widen the filters, or look at all states.'
                  : 'An incident gathers the alerts, the changes and the configuration movement around an outage into one timeline you can hand to somebody else. Open one when something is wrong — or from an alert, on the Monitoring screen.'
              }
              action={
                <Button variant="primary" icon={Plus} onClick={() => setOpening(true)}>
                  Open an incident
                </Button>
              }
            />
          }
        />
      )}

      <NewIncidentDialog
        open={opening}
        canUseAlerts={hasAlerts === true}
        onClose={() => setOpening(false)}
        onOpened={(id) => {
          setOpening(false);
          onOpen(id);
        }}
      />
    </>
  );
}

/** The devices an incident touches, by the names the inventory has now. */
function Devices({ incident }: { incident: IncidentHead }) {
  if (incident.devices.length === 0) return <span className={styles.muted}>none recorded</span>;
  const [first, ...rest] = incident.devices;
  if (!first) return null;
  return (
    <span className={styles.row}>
      <Link
        className={styles.clip}
        to={`/devices/${encodeURIComponent(first.id)}`}
        title={first.deleted ? `${first.name} — deleted from the inventory since` : first.name}
      >
        {first.name}
      </Link>
      {first.lab && <LabPill />}
      {rest.length > 0 && (
        <span className={styles.muted} title={rest.map((device) => device.name).join(', ')}>
          +{formatCount(rest.length)}
        </span>
      )}
    </span>
  );
}
