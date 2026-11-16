/**
 * VLANs: the record, and what each device calls the same id.
 *
 * `ipam.vlans` merges a record with the collected VLANs and keeps every
 * device's own name, so a VLAN that two switches call different things is
 * visible rather than averaged. A row with no record is marked as such — and
 * carries no "written" time, because the backend sends zero for one that does
 * not exist, and zero through a relative clock reads as 1970.
 */

import { Layers, Pencil, Plus, Trash2 } from 'lucide-react';
import type { VlanRow } from '@/api/types/ipam';
import { Badge } from '@/design/Badge';
import { Button, IconButton } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { LabPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { Tooltip } from '@/design/Tooltip';
import { formatCount } from '@/lib/format';
import { siteLabel } from './tree';
import styles from './Vlans.module.css';
import {
  hasLabDevice,
  nameDisagreements,
  presenceText,
  recordAge,
  VLAN_STATE_WORDS,
  vlanState,
} from './vlan-rows';

export interface VlansProps {
  rows: readonly VlanRow[];
  labNames: ReadonlyMap<string, string>;
  loading: boolean;
  onEdit: (row: VlanRow | null) => void;
  onDelete: (row: VlanRow) => void;
}

export function Vlans({ rows, labNames, loading, onEdit, onDelete }: VlansProps) {
  const columns: readonly TableColumn<VlanRow>[] = [
    {
      id: 'vid',
      header: 'VLAN id',
      width: 96,
      align: 'right',
      mono: true,
      required: true,
      sortValue: (row) => row.vid,
      cell: (row) => row.vid,
    },
    {
      id: 'name',
      header: 'Documented name',
      sortValue: (row) => row.name,
      cell: (row) =>
        row.documented ? (
          row.name
        ) : (
          <span className={styles.quiet}>no record</span>
        ),
    },
    {
      id: 'state',
      header: 'Record and network',
      width: 170,
      sortValue: (row) => vlanState(row),
      cell: (row) => {
        const words = VLAN_STATE_WORDS[vlanState(row)];
        return (
          <Tooltip content={words.meaning}>
            <Badge tone={words.tone} variant="outline">
              {words.label}
            </Badge>
          </Tooltip>
        );
      },
    },
    {
      id: 'site',
      header: 'Site',
      width: 150,
      sortValue: (row) => row.site ?? '',
      cell: (row) => (
        <span className={styles.site}>
          {siteLabel(row.site, labNames) ?? <span className={styles.quiet}>—</span>}
          {hasLabDevice(row) && <LabPill />}
        </span>
      ),
    },
    {
      id: 'where',
      header: 'Where it exists',
      sortValue: (row) => row.devices.length,
      cell: (row) => (
        <Tooltip
          content={
            row.devices.length === 0
              ? 'No collected device reports this VLAN.'
              : row.devices
                  .map((device) => `${device.device}: ${device.name || 'unnamed'} (${device.ports} ports)`)
                  .join(' · ')
          }
        >
          <span className={styles.where}>{presenceText(row)}</span>
        </Tooltip>
      ),
    },
    {
      id: 'names',
      header: 'Name mismatch',
      width: 230,
      cell: (row) => {
        const found = nameDisagreements(row);
        if (!found) return <span className={styles.quiet}>none</span>;
        return (
          <Tooltip
            content={
              found.against === 'record'
                ? `The record says “${found.expected}”. These devices call it something else: ${found.devices
                    .map((device) => `${device.device} (${device.name})`)
                    .join(', ')}.`
                : `There is no record. Most devices call it “${found.expected}”; these do not: ${found.devices
                    .map((device) => `${device.device} (${device.name})`)
                    .join(', ')}.`
            }
          >
            <span className={styles.mismatch}>
              <Badge tone="warn" variant="soft">
                {found.devices.length === 1
                  ? found.devices[0]?.name
                  : `${formatCount(found.devices.length)} devices differ`}
              </Badge>
              <span className={styles.against}>
                vs {found.against === 'record' ? 'the record' : 'the others'}
              </span>
            </span>
          </Tooltip>
        );
      },
    },
    {
      id: 'prefixes',
      header: 'Blocks',
      width: 160,
      mono: true,
      cell: (row) =>
        row.prefixes.length === 0 ? (
          <span className={styles.quiet}>none linked</span>
        ) : (
          row.prefixes.map((prefix) => prefix.cidr).join(', ')
        ),
    },
    {
      id: 'written',
      header: 'Written',
      width: 110,
      defaultHidden: true,
      sortValue: (row) => recordAge(row) ?? Number.NEGATIVE_INFINITY,
      cell: (row) => {
        const at = recordAge(row);
        return at === null ? <span className={styles.quiet}>no record</span> : <TimeAgo at={at} />;
      },
    },
  ];

  return (
    <div className={styles.wrap}>
      <div className={styles.bar}>
        <p className={styles.about}>
          A VLAN’s row exists if a person documented it, if a device reports it, or both. Nothing
          here is turned into a record on its own.
        </p>
        <Button size="sm" variant="primary" icon={Plus} onClick={() => onEdit(null)}>
          Add a VLAN
        </Button>
      </div>
      <Table
        id="ipam.vlans"
        aria-label="VLANs"
        columns={columns}
        rows={rows}
        rowKey={(row) => `${row.site ?? ''}-${row.vid}`}
        density="compact"
        loading={loading}
        defaultSort={{ columnId: 'vid', direction: 'asc' }}
        rowActions={(row) => [
          {
            label: row.documented ? 'Edit this record' : 'Write a record for it',
            icon: Pencil,
            onSelect: () => onEdit(row),
          },
          {
            label: 'Delete the record',
            icon: Trash2,
            tone: 'danger',
            disabled: !row.documented,
            onSelect: () => onDelete(row),
          },
        ]}
        empty={
          <EmptyState
            icon={Layers}
            size="compact"
            title="No VLAN is documented and none has been collected"
            description="Write a VLAN down, or collect from the switches: the VLANs they report appear here even before anybody documents them."
            action={
              <Button variant="primary" icon={Plus} onClick={() => onEdit(null)}>
                Add a VLAN
              </Button>
            }
          />
        }
      />
    </div>
  );
}

export interface VlanRowActionProps {
  row: VlanRow;
  onEdit: (row: VlanRow) => void;
}

/** The edit button used in the row actions menu of narrow layouts. */
export function VlanEditButton({ row, onEdit }: VlanRowActionProps) {
  return (
    <IconButton
      size="sm"
      variant="ghost"
      icon={Pencil}
      aria-label={`Edit VLAN ${row.vid}`}
      onClick={() => onEdit(row)}
    />
  );
}
