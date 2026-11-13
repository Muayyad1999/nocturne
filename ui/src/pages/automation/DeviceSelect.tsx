/**
 * Where something runs: a filter, and what it means right now.
 *
 * The backend resolves a selection when the run starts, so this edits the
 * filter itself and asks `tasks.devices` what it currently matches. The count
 * and the list under the form are the backend's answer to the exact selection
 * on screen — not a guess made in the browser — which is the only way a task
 * written for "every access switch in HQ" can be checked before it is saved.
 */

import { Server, Shield } from 'lucide-react';
import { useMemo } from 'react';
import type { SelectedDevice, Selector } from '@/api/types/automation';
import { Badge } from '@/design/Badge';
import { Combobox } from '@/design/Combobox';
import { Field } from '@/design/Field';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { LabPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { formatCount, plural } from '@/lib/format';
import styles from './Automation.module.css';
import { useDevices, useFacets, useSelectedDevices } from './data';
import { describeSelection, filterIsOpen, toSelector, type SelectionDraft } from './selector';

export interface DeviceSelectProps {
  value: SelectionDraft;
  onChange: (next: SelectionDraft) => void;
  /** Hides the resolved list, leaving the sentence and the count. */
  compact?: boolean;
  /** An id so two of these on one page do not share a table's stored state. */
  id?: string;
}

const STATUS_OPTIONS = [
  { value: '', label: 'Any state' },
  { value: 'up', label: 'Last seen up' },
  { value: 'down', label: 'Last seen down' },
  { value: 'unknown', label: 'Never checked' },
];

const LAB_OPTIONS = [
  { value: 'any', label: 'Lab and real' },
  { value: 'none', label: 'Real devices only' },
  { value: 'only', label: 'Lab devices only' },
];

export function DeviceSelect({ value, onChange, compact = false, id = 'selection' }: DeviceSelectProps) {
  const facets = useFacets();
  const devices = useDevices();
  const selector = useMemo<Selector>(() => toSelector(value), [value]);
  const selected = useSelectedDevices(selector);

  const set = <K extends keyof SelectionDraft>(key: K, next: SelectionDraft[K]) =>
    onChange({ ...value, [key]: next });

  const options = (counts: readonly [string, number][] | undefined, anyLabel: string) => [
    { value: '', label: anyLabel },
    ...(counts ?? []).map(([entry, count]) => ({ value: entry, label: `${entry} (${formatCount(count)})` })),
  ];

  const labById = new Map((devices.data ?? []).map((device) => [device.id, device.lab != null]));
  const named = value.deviceIds
    .map((deviceId) => (devices.data ?? []).find((device) => device.id === deviceId))
    .filter((device) => device !== undefined);

  const columns: TableColumn<SelectedDevice>[] = [
    {
      id: 'name',
      header: 'Device',
      required: true,
      sortValue: (device) => device.name,
      cell: (device) => (
        <span className={styles.cell}>
          <span className={styles.cellText} title={device.name}>
            {device.name}
          </span>
          {labById.get(device.device_id) === true && <LabPill />}
          {device.protected && (
            <Badge tone="maint" variant="outline" title="Protected: a change needs an explicit approval">
              <Icon icon={Shield} size={11} /> Protected
            </Badge>
          )}
        </span>
      ),
    },
    {
      id: 'address',
      header: 'Address',
      width: 130,
      mono: true,
      defaultHidden: true,
      cell: (device) => device.address || <span className={styles.muted}>No address</span>,
    },
    {
      id: 'profile',
      header: 'Type',
      width: 130,
      sortValue: (device) => device.profile,
      cell: (device) => device.profile || <span className={styles.muted}>Detect on connect</span>,
    },
    { id: 'site', header: 'Site', width: 120, sortValue: (device) => device.site, cell: (device) => device.site },
    {
      id: 'role',
      header: 'Role',
      width: 110,
      defaultHidden: true,
      sortValue: (device) => device.role,
      cell: (device) => device.role,
    },
  ];

  const count = selected.data?.count;
  const matching =
    selected.error !== null
      ? selected.error.message
      : count === undefined
        ? 'Working out what this matches…'
        : `${plural(count, 'device')} right now`;

  return (
    <div className={styles.selection}>
      <div className={styles.filterGrid}>
        <Field label="Name, address or tag contains">
          <Input
            value={value.query}
            placeholder="access-sw"
            onChange={(event) => set('query', event.target.value)}
          />
        </Field>
        <Field label="Site">
          <Select
            value={value.site}
            options={options(facets.data?.sites, 'Any site')}
            onChange={(event) => set('site', event.target.value)}
          />
        </Field>
        <Field label="Role">
          <Select
            value={value.role}
            options={options(facets.data?.roles, 'Any role')}
            onChange={(event) => set('role', event.target.value)}
          />
        </Field>
        <Field label="Device type">
          <Select
            value={value.profile}
            options={options(facets.data?.profiles, 'Any type')}
            onChange={(event) => set('profile', event.target.value)}
          />
        </Field>
        <Field label="Tag">
          <Select
            value={value.tag}
            options={options(facets.data?.tags, 'Any tag')}
            onChange={(event) => set('tag', event.target.value)}
          />
        </Field>
        <Field label="Last seen">
          <Select
            value={value.status}
            options={STATUS_OPTIONS}
            onChange={(event) => set('status', event.target.value as SelectionDraft['status'])}
          />
        </Field>
        <Field label="Lab devices" hint="Simulated devices are devices; say whether they are in.">
          <Select
            value={value.lab}
            options={LAB_OPTIONS}
            onChange={(event) => set('lab', event.target.value as SelectionDraft['lab'])}
          />
        </Field>
        <Field label="At most" hint="Leave empty for no cap.">
          <Input
            value={value.limit}
            inputMode="numeric"
            placeholder="No cap"
            onChange={(event) => set('limit', event.target.value.replace(/[^0-9]/g, ''))}
          />
        </Field>
      </div>

      <Field
        label="And these devices by name"
        hint="Added to whatever the filter matches. A device named here that no longer exists stops the run rather than shrinking it."
      >
        <Combobox
          options={(devices.data ?? [])
            .filter((device) => !value.deviceIds.includes(device.id))
            .map((device) => ({
              value: device.id,
              label: device.name,
              description: device.address || device.site,
            }))}
          value={null}
          placeholder="Add a device…"
          emptyText="No device matches"
          onChange={(deviceId) => {
            if (deviceId !== null) set('deviceIds', [...value.deviceIds, deviceId]);
          }}
        />
      </Field>
      {named.length > 0 && (
        <ul className={styles.chips} aria-label="Devices named one by one">
          {named.map((device) => (
            <li key={device.id}>
              <button
                type="button"
                className={styles.chip}
                title="Remove this device from the selection"
                onClick={() => set('deviceIds', value.deviceIds.filter((entry) => entry !== device.id))}
              >
                {device.name}
                {device.lab != null && <LabPill />}
                <span aria-hidden="true">×</span>
                <span className="visually-hidden">remove</span>
              </button>
            </li>
          ))}
        </ul>
      )}

      <p className={styles.resolved} data-error={selected.error !== null || undefined}>
        <Icon icon={Server} size={13} />
        <span>
          <strong>{describeSelection(selector)}</strong> — {matching}.
          {filterIsOpen(value) && value.deviceIds.length === 0 && count !== undefined && (
            <> Nothing is set in the filter, so this is the whole inventory.</>
          )}
        </span>
      </p>

      {!compact && selected.data !== undefined && selected.data.devices.length > 0 && (
        <Table
          id={`automation.${id}`}
          aria-label="Devices this selection matches"
          columns={columns}
          rows={selected.data.devices}
          rowKey={(device) => device.device_id}
          maxHeight={240}
          empty={null}
        />
      )}
    </div>
  );
}
