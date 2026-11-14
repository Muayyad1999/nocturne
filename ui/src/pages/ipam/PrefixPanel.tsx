/**
 * One block: what it is, what is inside it, and on whose authority.
 *
 * The header says in one line whether this block is somebody's plan or only a
 * discovery, and a discovered block shows the interfaces it was inferred from
 * rather than a bare badge. Utilisation is a percentage only where a
 * percentage can mean something; for a block larger than 64 bits it is what is
 * known to be in use, in the backend's own exact figure.
 */

import {
  CircleDot,
  Pencil,
  Plus,
  Scissors,
  Server,
  Trash2,
  Wand2,
} from 'lucide-react';
import type { PrefixDetail } from '@/api/types/ipam';
import { Badge } from '@/design/Badge';
import { Button, IconButton } from '@/design/Button';
import { Icon } from '@/design/Icon';
import { KeyValue, type KeyValueItem } from '@/design/KeyValue';
import { Panel } from '@/design/Panel';
import { LabPill } from '@/design/StatusPill';
import { TimeAgo } from '@/design/TimeAgo';
import { Tooltip } from '@/design/Tooltip';
import { formatCount, formatPercent, plural } from '@/lib/format';
import { dhcpText } from './addresses';
import { AddressGrid } from './AddressGrid';
import styles from './PrefixPanel.module.css';
import { recordState, siteLabel, sightingText, utilisation } from './tree';

export interface PrefixPanelProps {
  detail: PrefixDetail;
  labNames: ReadonlyMap<string, string>;
  selectedAddress: string | null;
  onSelectAddress: (ip: string) => void;
  onEdit: () => void;
  onSplit: () => void;
  onFreeSpace: () => void;
  onReserve: () => void;
  onDelete: () => void;
  onAddRecord: () => void;
}

export function PrefixPanel({
  detail,
  labNames,
  selectedAddress,
  onSelectAddress,
  onEdit,
  onSplit,
  onFreeSpace,
  onReserve,
  onDelete,
  onAddRecord,
}: PrefixPanelProps) {
  const row = detail.prefix;
  const record = recordState(row);
  const used = utilisation(row);
  const site = siteLabel(row.site, labNames);
  const isLab = row.site?.startsWith('lab:') ?? false;

  const facts: KeyValueItem[] = [
    { label: 'Site', value: site },
    { label: 'Role', value: row.role ?? null },
    { label: 'VLAN', value: row.vlan_id ?? null },
    { label: 'Gateway', value: row.gateway ?? null, mono: true },
    { label: 'DHCP range', value: dhcpText(row.dhcp), mono: true },
    {
      label: 'Child blocks',
      value:
        row.children === 0
          ? 'none'
          : `${plural(row.children, 'block')} · ${formatPercent(row.child_coverage_pct)} of this block covered`,
    },
    {
      label: 'Written',
      value:
        record.kind === 'documented' ? <TimeAgo at={row.updated_at} /> : 'never — nobody has documented this block',
    },
  ];

  return (
    <div className={styles.panel}>
      <header className={styles.head}>
        <div className={styles.title}>
          <h2 className={styles.cidr}>{row.cidr}</h2>
          {isLab && <LabPill />}
          <Tooltip content={record.meaning}>
            <Badge tone={record.tone} variant="outline">
              {record.label}
            </Badge>
          </Tooltip>
          {row.name !== '' && <span className={styles.name}>{row.name}</span>}
        </div>

        <div className={styles.actions}>
          <Button size="sm" variant="secondary" icon={Pencil} onClick={onEdit}>
            Edit
          </Button>
          <Button size="sm" variant="ghost" icon={Wand2} onClick={onFreeSpace}>
            Free space
          </Button>
          <Button size="sm" variant="ghost" icon={Scissors} onClick={onSplit}>
            Split…
          </Button>
          <Button size="sm" variant="ghost" icon={Plus} onClick={onReserve}>
            Reserve
          </Button>
          <IconButton
            size="sm"
            variant="ghost"
            icon={Trash2}
            aria-label={`Delete the record for ${row.cidr}`}
            onClick={onDelete}
          />
        </div>
      </header>

      {row.description !== '' && <p className={styles.description}>{row.description}</p>}

      {record.kind === 'discovered' && (
        <div className={styles.provenance} role="note">
          <Icon icon={CircleDot} size={13} />
          <div>
            <p className={styles.provenanceTitle}>
              This block is not in anybody’s plan. It exists because an address was seen inside it.
            </p>
            {detail.prefix.discovered_from.length > 0 && (
              <ul className={styles.provenanceList}>
                {detail.prefix.discovered_from.map((sighting) => (
                  <li key={`${sighting.device_id}-${sighting.interface}-${sighting.cidr}`}>
                    <span className={styles.mono}>{sighting.cidr}</span> on{' '}
                    {sightingText(sighting)} · read <TimeAgo at={sighting.collected_at} />
                  </li>
                ))}
              </ul>
            )}
            {/* The one call to action for a discovered block, next to the
                evidence that explains why it is offered. */}
            <Button size="sm" variant="secondary" icon={Pencil} onClick={onEdit}>
              Document this block
            </Button>
          </div>
        </div>
      )}

      <div className={styles.useRow}>
        {used.known ? (
          <>
            <div className={styles.useFigures}>
              <span className={styles.usePct} data-tone={used.tone}>
                {formatPercent(used.pct)}
              </span>
              <span className={styles.useLabel}>
                in use — {formatCount(used.used)} of {used.usableText} usable addresses
              </span>
            </div>
            <span className={styles.bar} data-tone={used.tone}>
              <span className={styles.fill} style={{ width: `${Math.min(100, used.pct)}%` }} />
            </span>
            <p className={styles.useDetail}>
              {formatCount(row.documented)} documented · {formatCount(row.observed)} seen on the
              network · {formatCount(row.reserved)} reserved · {formatCount(used.free)} with no
              record and nothing seen
            </p>
          </>
        ) : (
          <>
            <div className={styles.useFigures}>
              <span className={styles.usePct} data-tone="idle">
                {formatCount(used.used)}
              </span>
              <span className={styles.useLabel}>
                known to be in use, of {used.usableText} addresses
              </span>
            </div>
            <p className={styles.useDetail}>{used.reason}</p>
          </>
        )}
      </div>

      <KeyValue items={facts} layout="rows" className={styles.facts} />

      <Panel title="Addresses" padding="md" actions={<AddRecord onAddRecord={onAddRecord} />}>
        <AddressGrid detail={detail} selected={selectedAddress} onSelect={onSelectAddress} />
      </Panel>

      <Panel
        title="Runs with no record and nothing seen"
        padding="md"
        collapsible
        storageId="ipam.free-ranges"
        actions={<span className={styles.count}>{formatCount(detail.free_ranges.length)}</span>}
      >
        {detail.free_ranges.length === 0 ? (
          <p className={styles.quiet}>
            Every address in this block either has a record or has been seen.
          </p>
        ) : (
          <ul className={styles.ranges}>
            {detail.free_ranges.map((range) => (
              <li key={`${range.start}-${range.end}`}>
                <span className={styles.mono}>
                  {range.start}
                  {range.end !== range.start && ` – ${range.end}`}
                </span>
                <span className={styles.rangeCount}>{range.count_text}</span>
              </li>
            ))}
          </ul>
        )}
        {detail.free_child_prefixes.length > 0 && (
          <p className={styles.quiet}>
            Child blocks nothing covers: {detail.free_child_prefixes.slice(0, 8).join(', ')}
            {detail.free_child_prefixes.length > 8 &&
              ` and ${formatCount(detail.free_child_prefixes.length - 8)} more`}
          </p>
        )}
      </Panel>

      <Panel
        title="Interfaces configured in this block"
        padding="md"
        collapsible
        storageId="ipam.prefix-devices"
        actions={<span className={styles.count}>{formatCount(row.devices.length)}</span>}
      >
        {row.devices.length === 0 ? (
          <p className={styles.quiet}>
            No collected device has an interface with an address inside this block.
          </p>
        ) : (
          <ul className={styles.devices}>
            {row.devices.map((device) => (
              <li key={`${device.device_id}-${device.interface}-${device.ip}`}>
                <Icon icon={Server} size={12} />
                <span className={styles.mono}>{device.ip}</span>
                <span className={styles.deviceName}>
                  {device.device} ({device.interface})
                </span>
                {device.lab && <LabPill />}
                {device.prefix_len !== null && (
                  <MaskNote configured={device.prefix_len} block={row.cidr} />
                )}
                <span className={styles.deviceWhen}>
                  read <TimeAgo at={device.collected_at} />
                </span>
              </li>
            ))}
          </ul>
        )}
      </Panel>
    </div>
  );
}

function AddRecord({ onAddRecord }: { onAddRecord: () => void }) {
  return (
    <Button size="sm" variant="ghost" icon={Plus} onClick={onAddRecord}>
      Add a record
    </Button>
  );
}

/** An interface whose mask is not the block's: the backend reports it too. */
function MaskNote({ configured, block }: { configured: number; block: string }) {
  const length = Number(block.split('/')[1] ?? '0');
  if (configured === length) return null;
  return (
    <Tooltip
      content={`This interface is configured with /${configured}, and the block it sits in is a /${length}. Reconciliation lists this as a mask mismatch; it is not resolved here.`}
    >
      <Badge tone="warn" variant="outline">
        /{configured}
      </Badge>
    </Tooltip>
  );
}
