/**
 * The device list: everything Nocturne manages, in one table that stays
 * readable at two thousand rows.
 *
 * The whole inventory is read once (`devices.list`) and filtered here, so
 * typing in the search box and ticking a facet are instant and do not put a
 * call on the wire per keystroke. The backend refreshes it by event.
 */

import {
  Archive,
  Download,
  Pencil,
  Plus,
  RefreshCw,
  Search,
  Server,
  SlidersHorizontal,
  Trash,
  Upload,
  X,
} from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { Link, useNavigate } from 'react-router';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type { CountResult, DeviceRow, Facets, ProfileList } from '@/api/types/core';
import { registerCommand } from '@/app/CommandPalette/commands';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { LabPill } from '@/design/StatusPill';
import { SearchInput } from '@/design/SearchInput';
import { StatusDot } from '@/design/StatusDot';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { Tooltip } from '@/design/Tooltip';
import { EMPTY, formatCount, formatDuration } from '@/lib/format';
import { compareIp } from '@/lib/net';
import { BulkEditDialog } from './BulkEditDialog';
import { CollectBanner } from './CollectBanner';
import {
  deviceStatus,
  EMPTY_FILTER,
  effectiveProfile,
  filterDevices,
  isFiltered,
  TRANSPORT_LABEL,
  type DeviceFilterState,
} from './device';
import { DeviceDrawer } from './DeviceDrawer';
import { DeviceFacets, type StatusCounts } from './Facets';
import { ExportDialog } from './ExportDialog';
import { ImportDialog } from './ImportDialog';
import { useHasMethod } from '@/api/hooks';
import { useBackup, useCollect } from './useCollect';
import styles from './DeviceList.module.css';

const LAB_SEGMENTS: { value: DeviceFilterState['lab']; label: string; title: string }[] = [
  { value: 'all', label: 'All', title: 'Real and simulated devices' },
  { value: 'hide', label: 'Real', title: 'Hide simulated lab devices' },
  { value: 'only', label: 'Lab', title: 'Only simulated lab devices' },
];

function uptime(device: DeviceRow): number | null {
  const seconds = device.state.facts?.uptime_s;
  return seconds == null ? null : seconds * 1000;
}

export default function DeviceList() {
  const navigate = useNavigate();
  const [filter, setFilter] = useState<DeviceFilterState>(EMPTY_FILTER);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<DeviceRow | null>(null);
  const [importing, setImporting] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [bulkEditing, setBulkEditing] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);

  const devices = useRpc<DeviceRow[]>('devices.list', undefined, {
    invalidateOn: ['devices.changed', 'devices.state', 'collect.updated'],
  });
  const facets = useRpc<Facets>('devices.facets', undefined, {
    invalidateOn: ['devices.changed'],
  });
  const profiles = useRpc<ProfileList>('profiles.list', undefined, {
    invalidateOn: ['profiles.changed'],
  });

  const remove = useRpcMutation<CountResult, { ids: string[] }>('devices.delete', {
    invalidates: ['devices.'],
    errorToast: 'The devices were not deleted',
  });
  const collect = useCollect();
  const backup = useBackup();
  // `config.backup` comes from the configuration module, which is not in
  // every build.
  const canBackUp = useHasMethod('config.backup') === true;

  const profileName = useMemo(() => {
    const names = new Map(profiles.data?.profiles.map((profile) => [profile.id, profile.name]));
    return (id: string) => names.get(id) ?? id;
  }, [profiles.data]);

  const all = useMemo(() => devices.data ?? [], [devices.data]);
  const rows = useMemo(() => filterDevices(all, filter), [all, filter]);

  const statusCounts = useMemo<StatusCounts>(() => {
    const counts: StatusCounts = { up: 0, down: 0, unknown: 0 };
    for (const device of all) counts[deviceStatus(device).value] += 1;
    return counts;
  }, [all]);

  // A selection only ever means devices that are still on screen.
  const visible = useMemo(() => new Set(rows.map((row) => row.id)), [rows]);
  const selectedIds = useMemo(
    () => [...selected].filter((id) => visible.has(id)),
    [selected, visible],
  );
  const selectedDevices = useMemo(
    () => all.filter((device) => selectedIds.includes(device.id)),
    [all, selectedIds],
  );

  usePageHeader({ title: 'Devices' });

  useEffect(
    () =>
      registerCommand({
        id: 'devices.search',
        title: 'Search devices',
        group: 'Devices',
        keywords: ['filter', 'find'],
        // A bare key: the shell ignores it while the user is typing.
        shortcut: '/',
        icon: Search,
        run: () => searchRef.current?.focus(),
      }),
    [],
  );
  useEffect(
    () =>
      registerCommand({
        id: 'devices.add',
        title: 'Add device',
        group: 'Devices',
        keywords: ['new', 'create', 'inventory'],
        icon: Plus,
        run: () => setAdding(true),
      }),
    [],
  );
  useEffect(
    () =>
      registerCommand({
        id: 'devices.import',
        title: 'Import devices',
        group: 'Devices',
        keywords: ['csv', 'ansible', 'ssh config', 'inventory'],
        icon: Upload,
        run: () => setImporting(true),
      }),
    [],
  );
  useEffect(
    () =>
      registerCommand({
        id: 'devices.export',
        title: 'Export devices',
        group: 'Devices',
        keywords: ['csv', 'ansible', 'inventory', 'download'],
        icon: Download,
        run: () => setExporting(true),
      }),
    [],
  );

  const columns = useMemo<TableColumn<DeviceRow>[]>(
    () => [
      {
        id: 'status',
        header: 'Status',
        width: 96,
        sortValue: (device) => deviceStatus(device).label,
        cell: (device) => {
          const status = deviceStatus(device);
          const detail =
            device.state.last_error ??
            (device.state.last_checked
              ? `Last checked ${new Date(device.state.last_checked).toLocaleString()}`
              : 'Not checked yet');
          return (
            <Tooltip content={detail}>
              <span className={styles.status}>
                <StatusDot tone={status.tone} label={status.label} />
                {status.label}
              </span>
            </Tooltip>
          );
        },
      },
      {
        id: 'name',
        header: 'Name',
        required: true,
        sortValue: (device) => device.name,
        cell: (device) => (
          <span className={styles.name}>
            <Link className={styles.link} to={`/devices/${device.id}`}>
              <span className={styles.nameText}>{device.name}</span>
            </Link>
            {device.lab && <LabPill />}
            {device.protected && (
              <Tooltip content="Protected: automation and the assistant never change it without approval">
                <Badge tone="maint" variant="outline">
                  Protected
                </Badge>
              </Tooltip>
            )}
          </span>
        ),
      },
      {
        id: 'address',
        header: 'Address',
        width: 140,
        mono: true,
        compare: (a, b) => compareIp(a.address, b.address),
        cell: (device) => device.address || EMPTY,
      },
      {
        id: 'type',
        header: 'Device type',
        width: 150,
        sortValue: (device) => profileName(effectiveProfile(device)?.id ?? ''),
        cell: (device) => {
          const profile = effectiveProfile(device);
          if (!profile) return <span className={styles.muted}>Detect on connect</span>;
          return (
            <Tooltip content={profile.detected ? `${profile.id} — detected on connection` : profile.id}>
              <span className={styles.nameText}>
                {profileName(profile.id)}
                {profile.detected && <span className={styles.muted}> (detected)</span>}
              </span>
            </Tooltip>
          );
        },
      },
      {
        id: 'site',
        header: 'Site',
        width: 126,
        sortValue: (device) => device.site,
        cell: (device) => device.site || EMPTY,
      },
      {
        id: 'role',
        header: 'Role',
        width: 118,
        sortValue: (device) => device.role,
        cell: (device) => device.role || EMPTY,
      },
      {
        id: 'model',
        header: 'Model',
        width: 150,
        mono: true,
        // A fact, not an inventory field: it exists only once the device has
        // been collected from. One click away in the column chooser.
        defaultHidden: true,
        sortValue: (device) => device.state.facts?.model ?? '',
        cell: (device) => device.state.facts?.model || EMPTY,
      },
      {
        id: 'version',
        header: 'Version',
        width: 120,
        mono: true,
        defaultHidden: true,
        sortValue: (device) => device.state.facts?.os_version ?? '',
        cell: (device) => device.state.facts?.os_version || EMPTY,
      },
      {
        id: 'uptime',
        header: 'Uptime',
        width: 96,
        align: 'right',
        sortValue: (device) => uptime(device),
        cell: (device) => formatDuration(uptime(device)),
      },
      {
        id: 'transport',
        header: 'Connection',
        width: 120,
        defaultHidden: true,
        sortValue: (device) => device.transport,
        cell: (device) => TRANSPORT_LABEL[device.transport],
      },
      {
        id: 'seen',
        header: 'Last seen',
        width: 110,
        align: 'right',
        defaultHidden: true,
        sortValue: (device) => device.state.last_seen,
        cell: (device) => <TimeAgo at={device.state.last_seen} />,
      },
      {
        id: 'tags',
        header: 'Tags',
        width: 160,
        sortValue: (device) => device.tags.join(' '),
        cell: (device) =>
          device.tags.length === 0 ? (
            EMPTY
          ) : (
            <span className={styles.tags}>
              {device.tags.map((tag) => (
                <Badge key={tag} variant="outline">
                  {tag}
                </Badge>
              ))}
            </span>
          ),
      },
    ],
    [profileName],
  );

  const deleteDevices = async (targets: readonly DeviceRow[]) => {
    if (targets.length === 0) return;
    const names = targets.map((device) => device.name);
    const shown = names.slice(0, 5).join(', ');
    const confirmed = await confirm({
      title:
        targets.length === 1
          ? `Delete ${names[0]}?`
          : `Delete ${formatCount(targets.length)} devices?`,
      description: `${shown}${names.length > 5 ? ` and ${names.length - 5} more` : ''} will be removed from Nocturne, together with everything collected from them, their configuration history and their metrics. The devices themselves are not touched.`,
      confirmLabel: 'Delete',
      tone: 'danger',
      confirmText: targets.length > 1 ? 'delete' : undefined,
    });
    if (!confirmed) return;
    const result = await remove.mutateAsync({ ids: targets.map((device) => device.id) });
    setSelected(new Set());
    toast.success(`Deleted ${formatCount(result.count)} device${result.count === 1 ? '' : 's'}`);
  };

  if (devices.isError) {
    return (
      <Page>
        <ErrorState
          title="The device list could not be loaded"
          error={devices.error}
          onRetry={() => void devices.refetch()}
        />
      </Page>
    );
  }

  const nothingYet = !devices.isPending && all.length === 0;

  return (
    <Page>
      <Toolbar>
        <SearchInput
          ref={searchRef}
          className={styles.search}
          aria-label="Search devices"
          placeholder="Search name, address, site, model…"
          value={filter.query}
          onChange={(query) => setFilter((state) => ({ ...state, query }))}
        />

        {(facets.data?.lab ?? 0) > 0 && (
          <div className={styles.segmented} role="group" aria-label="Show lab devices">
            {LAB_SEGMENTS.map((segment) => (
              <button
                key={segment.value}
                type="button"
                className={styles.segment}
                aria-pressed={filter.lab === segment.value}
                title={segment.title}
                onClick={() => setFilter((state) => ({ ...state, lab: segment.value }))}
              >
                {segment.label}
              </button>
            ))}
          </div>
        )}

        <DeviceFacets
          facets={facets.data}
          statusCounts={statusCounts}
          filter={filter}
          onChange={(change) => setFilter((state) => ({ ...state, ...change }))}
          profileName={profileName}
        />

        {isFiltered(filter) && (
          <Button size="sm" variant="ghost" icon={X} onClick={() => setFilter(EMPTY_FILTER)}>
            Clear
          </Button>
        )}

        <ToolbarSpacer />

        <span className={styles.count}>
          {devices.isPending
            ? 'Loading…'
            : rows.length === all.length
              ? `${formatCount(all.length)} device${all.length === 1 ? '' : 's'}`
              : `${formatCount(rows.length)} of ${formatCount(all.length)}`}
        </span>
        <Button icon={Upload} onClick={() => setImporting(true)}>
          Import
        </Button>
        <Button icon={Download} onClick={() => setExporting(true)}>
          Export
        </Button>
        <Button variant="primary" icon={Plus} onClick={() => setAdding(true)}>
          Add device
        </Button>
      </Toolbar>

      <CollectBanner collection={collect} />

      {selectedIds.length > 0 && (
        <div className={styles.bulk} role="region" aria-label="Actions for the selected devices">
          <span className={styles.bulkCount}>
            {formatCount(selectedIds.length)} selected
          </span>
          <Button
            size="sm"
            icon={RefreshCw}
            loading={collect.starting || collect.running}
            onClick={() => collect.start(selectedIds)}
          >
            Collect
          </Button>
          {canBackUp && (
            <Button
              size="sm"
              icon={Archive}
              loading={backup.starting || backup.running}
              onClick={() => backup.start(selectedIds)}
            >
              Back up
            </Button>
          )}
          <Button size="sm" icon={SlidersHorizontal} onClick={() => setBulkEditing(true)}>
            Edit…
          </Button>
          <Button size="sm" icon={Download} onClick={() => setExporting(true)}>
            Export
          </Button>
          <Button
            size="sm"
            variant="danger"
            icon={Trash}
            onClick={() => void deleteDevices(selectedDevices)}
          >
            Delete
          </Button>
          <ToolbarSpacer />
          <Button size="sm" variant="ghost" onClick={() => setSelected(new Set())}>
            Clear selection
          </Button>
        </div>
      )}

      <Table
        className={styles.table}
        id="devices"
        aria-label="Devices"
        fill
        columns={columns}
        rows={rows}
        rowKey={(device) => device.id}
        // The device types are needed to name the type column: showing the
        // rows a moment earlier would flash raw profile ids.
        loading={devices.isPending || profiles.isPending}
        defaultSort={{ columnId: 'name', direction: 'asc' }}
        selection={{ selected, onChange: setSelected }}
        onRowActivate={(device) => void navigate(`/devices/${device.id}`)}
        rowActions={(device) => [
          { label: 'Open', icon: Server, onSelect: () => void navigate(`/devices/${device.id}`) },
          { label: 'Collect now', icon: RefreshCw, onSelect: () => collect.start([device.id]) },
          { label: 'Edit…', icon: Pencil, onSelect: () => setEditing(device) },
          { type: 'separator' },
          {
            label: 'Delete…',
            icon: Trash,
            tone: 'danger',
            onSelect: () => void deleteDevices([device]),
          },
        ]}
        empty={
          nothingYet ? (
            <EmptyState
              icon={Server}
              title="No devices yet"
              description="Add the first router, switch or firewall and Nocturne will reach out to it, work out what it is, and keep an eye on it from then on."
              action={
                <>
                  <Button variant="primary" icon={Plus} onClick={() => setAdding(true)}>
                    Add device
                  </Button>
                  <Button icon={Upload} onClick={() => setImporting(true)}>
                    Import a list
                  </Button>
                </>
              }
            />
          ) : (
            <EmptyState
              size="compact"
              icon={Server}
              title="No device matches"
              description="Nothing in the inventory matches this search and these filters."
              action={
                <Button size="sm" icon={X} onClick={() => setFilter(EMPTY_FILTER)}>
                  Clear filters
                </Button>
              }
            />
          )
        }
      />

      <DeviceDrawer
        open={adding || editing !== null}
        device={editing}
        devices={all}
        onClose={() => {
          setAdding(false);
          setEditing(null);
        }}
      />
      <ImportDialog open={importing} onClose={() => setImporting(false)} />
      <ExportDialog
        open={exporting}
        selection={selectedIds}
        onClose={() => setExporting(false)}
      />
      <BulkEditDialog
        open={bulkEditing}
        ids={selectedIds}
        onClose={() => setBulkEditing(false)}
      />
    </Page>
  );
}
