/**
 * Operations → Evidence: what was kept, newest first.
 *
 * An evidence item is something the engine read — the last lines of a
 * terminal session, a piece of a session log, the map as it was drawn, the
 * samples behind a graph — or text a person handed in, which is labelled as
 * that wherever it appears. Each is redacted before it is stored and carries
 * its source, its capture time, its size and the SHA-256 of what is stored.
 *
 * The list holds heads only; a body is read when one item is opened
 * (`?item=<id>`, a link one can send). It pages by the engine's cursor, which
 * gives no total, so the line under the filters says what is on screen and
 * whether older items exist — never a count the engine did not give.
 *
 * The search is the engine's: it matches the title, the device, the session
 * and the note. It does not look inside the content, and the field says so,
 * because "nothing matches" must not be read as "no device ever printed
 * that".
 */

import { FileCheck2 } from 'lucide-react';
import { useEffect, useLayoutEffect, useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import type { Evidence, EvidenceKind } from '@/api/types/ops';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { LabPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { Toolbar } from '@/design/Toolbar';
import { cx } from '@/lib/cx';
import { formatBytes, plural } from '@/lib/format';
import { formatTimestamp } from '@/lib/time';
import { useDevices, useEvidenceList, useIncidents } from './data';
import { EvidenceItem } from './EvidenceItem';
import styles from './Evidence.module.css';
import { KINDS, KIND_LABEL, KIND_MEANING, isEvidenceKind, isOperatorSupplied, listCoverage } from './evidenceReading';
import opsStyles from './Ops.module.css';
import { instant } from './ops';

const PAGE = 50;

/**
 * Below this width of the list (1280 with the assistant dock open is about
 * 640) six columns do not fit: the table scrolled sideways and the title, the
 * one column a row cannot do without, was cut to a dozen characters. The two
 * columns an item's own page repeats give way to it.
 */
const ROOMY = 860;
const WHEN_ROOMY: ReadonlySet<string> = new Set(['size', 'incidents']);

/**
 * Whether an element is at least `width` wide; true until it has been
 * measured. The element is held in state, not in a ref: the list is not in
 * the page while it shows an error, and has to be measured when it returns.
 */
function useAtLeast(width: number) {
  const [element, setElement] = useState<HTMLDivElement | null>(null);
  const [roomy, setRoomy] = useState(true);
  useLayoutEffect(() => {
    if (!element) return;
    const measure = () => setRoomy(element.clientWidth >= width);
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [element, width]);
  return [setElement, roomy] as const;
}

export function EvidenceTab() {
  const [search, setSearch] = useSearchParams();
  const itemId = search.get('item');

  const open = (id: string | null) => {
    const next = new URLSearchParams(search);
    if (id) next.set('item', id);
    else next.delete('item');
    setSearch(next, { replace: id === null });
  };

  return itemId ? (
    <EvidenceItem key={itemId} id={itemId} onClose={() => open(null)} />
  ) : (
    <EvidenceList />
  );
}

function EvidenceList() {
  const [search, setSearch] = useSearchParams();
  const [query, setQuery] = useState('');
  // The cursors of the pages walked so far: the last one is the page on screen.
  const [cursors, setCursors] = useState<string[]>([]);
  const [box, roomy] = useAtLeast(ROOMY);

  // The three filters live in the address, so "the evidence on this incident"
  // is a link the incident page can make and a person can send.
  const kind = isEvidenceKind(search.get('kind')) ? (search.get('kind') as EvidenceKind) : null;
  const deviceId = search.get('device');
  const incidentId = search.get('attached');

  // What the three selects show. The address follows, but the router applies
  // an address change as a transition, and React puts a controlled select
  // whose value only changes in a transition back to its old value after the
  // key press that changed it: arrow keys bounced back, and a quick second
  // press read the old choice and skipped an option. The select holds the
  // choice at once; the address catches up, and a link that changes the
  // address from elsewhere still sets the selects.
  const [shown, setShown] = useState({ kind: kind ?? '', device: deviceId ?? '', attached: incidentId ?? '' });
  useEffect(() => {
    setShown({ kind: kind ?? '', device: deviceId ?? '', attached: incidentId ?? '' });
  }, [kind, deviceId, incidentId]);

  const setFilter = (name: 'kind' | 'device' | 'attached', value: string) => {
    setShown((current) => ({ ...current, [name]: value }));
    // From the address as it is by then, not as this render saw it: two
    // filters changed in quick succession must both arrive.
    setSearch(
      (current) => {
        const next = new URLSearchParams(current);
        if (value === '') next.delete(name);
        else next.set(name, value);
        return next;
      },
      { replace: true },
    );
    setCursors([]);
  };

  const cursor = cursors.at(-1);
  const list = useEvidenceList({
    limit: PAGE,
    ...(kind ? { kind } : {}),
    ...(deviceId ? { device_id: deviceId } : {}),
    ...(incidentId ? { incident_id: incidentId } : {}),
    ...(query.trim() === '' ? {} : { query: query.trim() }),
    ...(cursor ? { cursor } : {}),
  });
  const devices = useDevices();
  const incidents = useIncidents({ state: 'any', limit: 500 });

  const filtered = Boolean(kind || deviceId || incidentId || query.trim() !== '');
  const items = list.data?.items ?? [];
  const more = list.data?.next_cursor;
  const incidentTitle = (id: string) => incidents.data?.incidents.find((incident) => incident.id === id)?.title;

  const every: readonly TableColumn<Evidence>[] = [
    {
      id: 'kind',
      header: 'Kind',
      width: 150,
      required: true,
      // Text a person handed in says so in the list too, in the one column
      // that is never squeezed out.
      cell: (row) =>
        isOperatorSupplied(row.source) ? (
          <Badge
            tone="warn"
            variant="outline"
            title="Supplied by a person: the engine redacted it but did not read it from a device."
          >
            {KIND_LABEL[row.kind] ?? row.kind}, by a person
          </Badge>
        ) : (
          <Badge tone="info" variant="outline" title={KIND_MEANING[row.kind]}>
            {KIND_LABEL[row.kind] ?? row.kind}
          </Badge>
        ),
    },
    {
      id: 'title',
      header: 'Evidence',
      required: true,
      cell: (row) => (
        <span className={styles.titleCell}>
          <Link
            className={opsStyles.clip}
            to={`/ops/evidence?item=${encodeURIComponent(row.id)}`}
            title={row.title}
            data-testid="evidence-row"
            data-evidence={row.id}
          >
            {row.title}
          </Link>
        </span>
      ),
    },
    {
      id: 'device',
      header: 'Device',
      width: 180,
      // The lab label sits with what it is about: where the item came from.
      cell: (row) => (
        <span className={styles.titleCell}>
          {row.source.device_id ? (
            <Link className={opsStyles.clip} to={`/devices/${encodeURIComponent(row.source.device_id)}`}>
              {row.source.device ?? row.source.device_id}
            </Link>
          ) : (
            // Not a missing name: these kinds are not about one device.
            <span className={cx(opsStyles.clip, opsStyles.muted)}>
              {row.kind === 'topology' ? 'the drawing' : row.kind === 'chart' ? 'several' : 'none recorded'}
            </span>
          )}
          {row.source.lab && <LabPill />}
        </span>
      ),
    },
    {
      id: 'captured',
      header: 'Captured',
      width: 104,
      align: 'right',
      cell: (row) => <TimeAgo at={instant(row.captured_at)} title={formatTimestamp(row.captured_at)} />,
    },
    {
      id: 'size',
      header: 'Size',
      // Room for the most an item holds: "10 000 lines · 256 KiB".
      width: 176,
      align: 'right',
      cell: (row) =>
        row.lines === undefined ? formatBytes(row.bytes) : `${plural(row.lines, 'line')} · ${formatBytes(row.bytes)}`,
    },
    {
      id: 'incidents',
      header: 'Attached to',
      width: 170,
      cell: (row) => {
        const [first] = row.incident_ids;
        if (first === undefined) return <span className={opsStyles.muted}>not attached</span>;
        return (
          <span className={styles.titleCell}>
            <Link className={opsStyles.clip} to={`/ops/incidents?incident=${encodeURIComponent(first)}`}>
              {incidentTitle(first) ?? `incident ${first}`}
            </Link>
            {row.incident_ids.length > 1 && (
              <Badge variant="outline" title={`Attached to ${plural(row.incident_ids.length, 'incident')}`}>
                +{row.incident_ids.length - 1}
              </Badge>
            )}
          </span>
        );
      },
    },
    {
      id: 'sha256',
      header: 'SHA-256',
      width: 130,
      mono: true,
      defaultHidden: true,
      cell: (row) => <span title={row.sha256}>{row.sha256.slice(0, 12)}…</span>,
    },
  ];
  const columns = roomy ? every : every.filter((column) => !WHEN_ROOMY.has(column.id));

  return (
    <>
      <Toolbar>
        <SearchInput
          aria-label="Search evidence"
          value={query}
          onChange={(value) => {
            setQuery(value);
            setCursors([]);
          }}
          placeholder="Search titles, devices, sessions and notes"
          className={styles.search}
        />
        <Select
          aria-label="Kind"
          size="sm"
          className={opsStyles.narrow}
          value={shown.kind}
          onChange={(event) => setFilter('kind', event.target.value)}
        >
          <option value="">Every kind</option>
          {KINDS.map((entry) => (
            <option key={entry} value={entry}>
              {KIND_LABEL[entry]}
            </option>
          ))}
        </Select>
        <Select
          aria-label="Device"
          size="sm"
          className={opsStyles.narrow}
          value={shown.device}
          onChange={(event) => setFilter('device', event.target.value)}
        >
          <option value="">Every device</option>
          {/* A filter that came in the address stays a choice even when the inventory no longer has the device. */}
          {deviceId && !devices.data?.some((device) => device.id === deviceId) && (
            <option value={deviceId}>device {deviceId}</option>
          )}
          {devices.data?.map((device) => (
            <option key={device.id} value={device.id}>
              {device.name}
            </option>
          ))}
        </Select>
        <Select
          aria-label="Incident"
          size="sm"
          className={opsStyles.filter}
          value={shown.attached}
          onChange={(event) => setFilter('attached', event.target.value)}
        >
          <option value="">Attached or not</option>
          {incidentId && !incidents.data?.incidents.some((incident) => incident.id === incidentId) && (
            <option value={incidentId}>incident {incidentId}</option>
          )}
          {incidents.data?.incidents.map((incident) => (
            <option key={incident.id} value={incident.id}>
              On: {incident.title}
            </option>
          ))}
        </Select>
      </Toolbar>

      <p className={opsStyles.facts} data-testid="evidence-coverage-line">
        {list.data && <span>{listCoverage(items.length, Boolean(more), filtered, cursors.length === 0)}</span>}
        <span className={opsStyles.muted}>
          The search reads titles, devices, sessions and notes — not the content of an item.
        </span>
        <span className={opsStyles.muted}>Everything here was redacted before it was stored.</span>
      </p>

      {list.isError ? (
        <ErrorState
          title="The evidence could not be read"
          error={list.error}
          onRetry={() => void list.refetch()}
        />
      ) : (
        <div ref={box} className={styles.list}>
          <Table
            id="ops.evidence"
            aria-label="Evidence"
            columns={columns}
            rows={items}
            rowKey={(row) => row.id}
            loading={list.isPending}
            density="comfortable"
            onRowActivate={(row) => {
              const next = new URLSearchParams(search);
              next.set('item', row.id);
              setSearch(next);
            }}
            empty={
              filtered || cursors.length > 0 ? (
                <EmptyState
                  size="compact"
                  icon={FileCheck2}
                  title="Nothing matches that"
                  description="The search covers titles, devices, sessions and notes, not what an item contains. Widen the filters to see the rest."
                  action={
                    <Button
                      variant="secondary"
                      onClick={() => {
                        setQuery('');
                        setCursors([]);
                        const next = new URLSearchParams(search);
                        for (const name of ['kind', 'device', 'attached']) next.delete(name);
                        setSearch(next, { replace: true });
                      }}
                    >
                      Clear the filters
                    </Button>
                  }
                />
              ) : (
                <EmptyState
                  size="compact"
                  icon={FileCheck2}
                  title="No evidence has been kept yet"
                  description="Evidence is what a device printed, the map as it was drawn or the samples behind a graph, kept with its source, its time and a hash. Keep some from an incident’s timeline (Attach evidence…), from a chart’s export menu or from the topology’s export dialog."
                />
              )
            }
          />
          {(cursors.length > 0 || more) && (
            <div className={styles.paging}>
              {cursors.length > 0 && (
                <Button variant="secondary" onClick={() => setCursors([])}>
                  Back to the newest
                </Button>
              )}
              {cursors.length > 0 && (
                <Button variant="ghost" onClick={() => setCursors(cursors.slice(0, -1))}>
                  Newer
                </Button>
              )}
              {more && (
                <Button variant="secondary" onClick={() => setCursors([...cursors, more])}>
                  Older evidence
                </Button>
              )}
            </div>
          )}
        </div>
      )}
    </>
  );
}
