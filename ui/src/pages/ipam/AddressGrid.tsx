/**
 * The addresses of one block: documented, seen, both, or nothing known.
 *
 * The grid is the classic one — sixteen columns of a /24 — but the colours do
 * not mean what they usually do. They are not "free / used": they are the four
 * combinations of a record and a sighting, which is the only honest reading of
 * two tables that are deliberately kept apart. A cell with nothing in it says
 * "no record, nothing seen", and the sentence under the legend is the
 * backend's own `note` about what that does and does not mean, next to what
 * was actually collected.
 *
 * A block too large to list address by address gets a table of the addresses
 * something is known about, and says that is what it is.
 */

import { CircleAlert, Search } from 'lucide-react';
import { useRef, useState, type KeyboardEvent } from 'react';
import type { Address, DeviceCoverage, PrefixDetail } from '@/api/types/ipam';
import { Badge } from '@/design/Badge';
import { EmptyState } from '@/design/EmptyState';
import { Icon } from '@/design/Icon';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { Tooltip } from '@/design/Tooltip';
import { formatCount, plural } from '@/lib/format';
import { compareIp } from '@/lib/net';
import {
  addressState,
  gridCounts,
  gridScopeText,
  PLAN_MARK_WORDS,
  planMarks,
  seenText,
  STATE_WORDS,
  STATUS_LABEL,
  type AddressState,
  type PlanMark,
} from './addresses';
import styles from './AddressGrid.module.css';

/** The order the legend and the grid use. `conflict` is last: it is loudest. */
const STATES: readonly AddressState[] = ['both', 'documented', 'observed', 'unknown', 'conflict'];

const COLUMNS = 16;

export interface AddressGridProps {
  detail: PrefixDetail;
  selected: string | null;
  onSelect: (ip: string) => void;
}

/** The last label of an address: `.37` for IPv4, the final group for IPv6. */
function cellLabel(ip: string): string {
  if (ip.includes('.')) return ip.slice(ip.lastIndexOf('.') + 1);
  const groups = ip.split(':');
  return groups[groups.length - 1] || '0';
}

function cellTitle(address: Address, mark: PlanMark | null): string {
  const state = STATE_WORDS[addressState(address)];
  const parts = [`${address.ip} — ${state.label}`];
  if (mark) parts.push(PLAN_MARK_WORDS[mark]);
  if (address.hostname) parts.push(address.hostname);
  if (address.documented) parts.push(`record: ${STATUS_LABEL[address.status]}`);
  if (address.observations.length > 0) {
    const first = address.observations[0];
    if (first) {
      parts.push(
        `seen on ${first.device}${first.interface ? ` (${first.interface})` : ''}`,
      );
    }
  }
  return parts.join(' · ');
}

export function AddressGrid({ detail, selected, onSelect }: AddressGridProps) {
  const addresses = detail.addresses;
  const counts = gridCounts(addresses);
  // What the *plan* says about an address, which the grid marks differently
  // from anything a device reported.
  const mark = planMarks(detail.prefix);
  const gridRef = useRef<HTMLDivElement>(null);
  const [focused, setFocused] = useState<string | null>(null);

  const focusCell = (ip: string | undefined) => {
    if (ip === undefined) return;
    setFocused(ip);
    gridRef.current?.querySelector<HTMLElement>(`[data-ip="${CSS.escape(ip)}"]`)?.focus();
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const ip = (event.target as HTMLElement).dataset.ip;
    if (!ip) return;
    const index = addresses.findIndex((address) => address.ip === ip);
    if (index < 0) return;
    const step: Record<string, number> = {
      ArrowRight: 1,
      ArrowLeft: -1,
      ArrowDown: COLUMNS,
      ArrowUp: -COLUMNS,
    };
    const move = step[event.key];
    if (move === undefined) return;
    event.preventDefault();
    focusCell(addresses[Math.min(addresses.length - 1, Math.max(0, index + move))]?.ip);
  };

  const tabStop = focused ?? selected ?? addresses[0]?.ip;

  return (
    <div className={styles.wrap}>
      <Legend
        counts={counts}
        grid={detail.grid}
        note={detail.note}
        coverage={detail.coverage}
        gateway={detail.prefix.gateway ?? null}
        dhcp={detail.prefix.dhcp ?? null}
      />

      {addresses.length === 0 ? (
        <EmptyState
          icon={Search}
          size="compact"
          title="Nothing is known about any address in this block"
          description="No address here has a record, and no device has reported one. That is not the same as the block being empty — see what was collected, above."
        />
      ) : detail.grid ? (
        <div
          className={styles.grid}
          ref={gridRef}
          onKeyDown={onKeyDown}
          role="grid"
          aria-label={`Addresses in ${detail.prefix.cidr}`}
        >
          {addresses.map((address) => {
            const state = addressState(address);
            const planned = mark(address.ip);
            return (
              <button
                key={address.ip}
                type="button"
                role="gridcell"
                data-ip={address.ip}
                data-state={state}
                data-status={address.status}
                data-plan={planned ?? undefined}
                data-selected={address.ip === selected || undefined}
                tabIndex={address.ip === tabStop ? 0 : -1}
                className={styles.cell}
                title={cellTitle(address, planned)}
                aria-label={cellTitle(address, planned)}
                onFocus={() => setFocused(address.ip)}
                onClick={() => onSelect(address.ip)}
              >
                <span className={styles.cellLabel}>{cellLabel(address.ip)}</span>
                {address.disagreements.length > 0 && (
                  <span className={styles.flag} aria-hidden="true" />
                )}
              </button>
            );
          })}
        </div>
      ) : (
        <KnownTable detail={detail} onSelect={onSelect} />
      )}
    </div>
  );
}

interface LegendProps {
  counts: ReturnType<typeof gridCounts>;
  grid: boolean;
  note: string;
  coverage: readonly DeviceCoverage[];
  /** What the block documents, which the grid marks on top of the states. */
  gateway: string | null;
  dhcp: { start: string; end: string } | null;
}

function Legend({ counts, grid, note, coverage, gateway, dhcp }: LegendProps) {
  const read = coverage.filter((device) => device.interfaces_at !== null);
  const newest = read.reduce<number | null>(
    (latest, device) =>
      device.interfaces_at !== null && (latest === null || device.interfaces_at > latest)
        ? device.interfaces_at
        : latest,
    null,
  );

  return (
    <div className={styles.legend}>
      <ul className={styles.keys}>
        {STATES.map((state) => {
          const words = STATE_WORDS[state];
          return (
            <li key={state} className={styles.key}>
              <Tooltip content={words.meaning}>
                <span className={styles.keyInner}>
                  <span className={styles.swatch} data-state={state} aria-hidden="true" />
                  <span className={styles.keyLabel}>{words.label}</span>
                  <span className={styles.keyCount}>{formatCount(counts[state])}</span>
                </span>
              </Tooltip>
            </li>
          );
        })}
        {counts.reserved > 0 && (
          <li className={styles.key}>
            <Badge tone="info" variant="outline">
              {formatCount(counts.reserved)} reserved
            </Badge>
          </li>
        )}
        {counts.dhcp > 0 && (
          <li className={styles.key}>
            <Badge tone="info" variant="outline">
              {formatCount(counts.dhcp)} recorded as DHCP
            </Badge>
          </li>
        )}
        {gateway !== null && gateway !== '' && (
          <li className={styles.key}>
            <Tooltip content={`This block documents ${gateway} as its gateway. Whether anything has that address is a different question — see the conflicts.`}>
              <span className={styles.keyInner}>
                <span className={styles.swatch} data-plan="gateway" aria-hidden="true" />
                <span className={styles.keyLabel}>gateway {gateway}</span>
              </span>
            </Tooltip>
          </li>
        )}
        {dhcp && (
          <li className={styles.key}>
            <Tooltip content={`This block documents ${dhcp.start} – ${dhcp.end} as its DHCP range. The marks show the plan, not a lease.`}>
              <span className={styles.keyInner}>
                <span className={styles.swatch} data-plan="dhcp" aria-hidden="true" />
                <span className={styles.keyLabel}>
                  DHCP range {dhcp.start} – {dhcp.end}
                </span>
              </span>
            </Tooltip>
          </li>
        )}
      </ul>

      <p className={styles.note}>
        <Icon icon={CircleAlert} size={12} />
        {/* The backend's own sentence about what the empty cells mean. */}
        <span>{note}</span>
      </p>

      <p className={styles.coverage}>
        {read.length === 0 ? (
          <span className={styles.coverageWarn}>
            No device that could have seen this block has ever been collected from, so “nothing
            seen” here is not evidence of anything.
          </span>
        ) : (
          <>
            <span className={styles.nowrap}>
              {plural(read.length, 'device')} could have seen addresses in this block
            </span>
            {newest !== null && (
              <span className={styles.nowrap}>
                {'\u00a0· '}
                last read <TimeAgo at={newest} />
              </span>
            )}
            {read.some((device) => device.lab) && (
              <span className={styles.nowrap}>{'\u00a0· includes lab devices'}</span>
            )}
          </>
        )}
      </p>

      <p className={styles.scope}>{gridScopeText(grid, counts)}</p>
    </div>
  );
}

interface KnownTableProps {
  detail: PrefixDetail;
  onSelect: (ip: string) => void;
}

/** A block too large for a grid: only what something is known about. */
function KnownTable({ detail, onSelect }: KnownTableProps) {
  const columns: readonly TableColumn<Address>[] = [
    {
      id: 'ip',
      header: 'Address',
      mono: true,
      required: true,
      compare: (a, b) => compareIp(a.ip, b.ip),
      cell: (row) => row.ip,
    },
    {
      id: 'state',
      header: 'Record and network',
      width: 150,
      sortValue: (row) => addressState(row),
      cell: (row) => {
        const words = STATE_WORDS[addressState(row)];
        return (
          <Badge tone={words.tone} variant="outline">
            {words.label}
          </Badge>
        );
      },
    },
    {
      id: 'hostname',
      header: 'Documented as',
      sortValue: (row) => row.hostname ?? '',
      cell: (row) => row.hostname ?? <span className={styles.muted}>no record</span>,
    },
    {
      id: 'where',
      header: 'Seen on',
      width: 190,
      mono: true,
      cell: (row) => {
        const first = row.observations[0];
        if (!first) return <span className={styles.muted}>never seen</span>;
        return `${first.device}${first.interface ? ` (${first.interface})` : ''}`;
      },
    },
    {
      id: 'last_seen',
      header: 'Last seen',
      width: 104,
      sortValue: (row) => row.last_seen ?? Number.NEGATIVE_INFINITY,
      cell: (row) =>
        row.last_seen === null ? (
          <span className={styles.muted}>never</span>
        ) : (
          <TimeAgo at={row.last_seen} />
        ),
    },
  ];

  return (
    <Table
      id="ipam.addresses"
      aria-label={`Addresses known in ${detail.prefix.cidr}`}
      columns={columns}
      rows={detail.addresses}
      rowKey={(row) => row.ip}
      density="compact"
      defaultSort={{ columnId: 'ip', direction: 'asc' }}
      onRowActivate={(row) => onSelect(row.ip)}
      empty={
        <EmptyState
          icon={Search}
          size="compact"
          title="No address in this block has a record or a sighting"
          description="Nothing has been documented here and no device has reported an address inside it."
        />
      }
    />
  );
}

/** What `seenText` says, for the row the panel is showing. */
export { seenText };
