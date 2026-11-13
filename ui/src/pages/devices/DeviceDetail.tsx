/**
 * One device: what it is, what it is made of, and what Nocturne last read
 * from it.
 *
 * Every tab reads from `collect.get`, which returns `null` for a kind that
 * was never collected. That is said in words — and a kind this device type
 * cannot collect at all says that instead. Nothing on this page invents a
 * zero for something the device did not answer.
 */

import {
  Archive,
  ArrowLeft,
  CircleAlert,
  Database,
  FileText,
  Network,
  Ellipsis,
  Pencil,
  Plug,
  RefreshCw,
  Server,
  Trash,
} from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type {
  ConnectionTest,
  CountResult,
  Credential,
  DeviceRow,
  DeviceUpdateParams,
  Health,
  Interface,
  Kind,
  KindStatus,
  ProfileList,
  Stored,
} from '@/api/types/core';
import { KINDS } from '@/api/types/core';
import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { Badge } from '@/design/Badge';
import { Button, IconButton } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { DropdownMenu } from '@/design/DropdownMenu';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { KeyValue, type KeyValueItem } from '@/design/KeyValue';
import { ProgressBar } from '@/design/ProgressBar';
import { Panel } from '@/design/Panel';
import { Skeleton } from '@/design/Skeleton';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { TabPanel, Tabs } from '@/design/Tabs';
import { Textarea } from '@/design/Textarea';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { EMPTY, formatBytes, formatCount, formatDuration, formatPercent } from '@/lib/format';
import { CollectBanner } from './CollectBanner';
import { KindTable } from './collected';
import {
  deviceStatus,
  effectiveProfile,
  KIND_DESCRIPTION,
  KIND_LABEL,
  KIND_PLURAL,
  TRANSPORT_LABEL,
} from './device';
import { DeviceDrawer } from './DeviceDrawer';
import { InterfaceDrawer } from './InterfaceDrawer';
import { TestResult, TestResultPending } from './TestResult';
import { useHasMethod } from '@/api/hooks';
import { useBackup, useCollect } from './useCollect';
import styles from './DeviceDetail.module.css';

const { label: areaLabel, path: areaPath } = destination('devices');

const TABS = [
  { id: 'summary', label: 'Summary', icon: Server },
  { id: 'interfaces', label: 'Interfaces', icon: Network },
  { id: 'data', label: 'Data', icon: Database },
  { id: 'notes', label: 'Notes', icon: FileText },
];

export default function DeviceDetail({ id }: { id: string }) {
  const navigate = useNavigate();
  const [tab, setTab] = useState('summary');
  /** Set while one collected kind is open from the Data tab. */
  const [openKind, setOpenKind] = useState<Kind | null>(null);
  const [editing, setEditing] = useState(false);
  const [test, setTest] = useState<ConnectionTest | null>(null);
  const [openInterface, setOpenInterface] = useState<Interface | null>(null);

  const inventory = useRpc<DeviceRow[]>('devices.list', undefined, {
    invalidateOn: ['devices.changed'],
  });
  const device = useRpc<DeviceRow>(
    'devices.get',
    { id },
    { invalidateOn: ['devices.changed', 'devices.state'] },
  );
  const status = useRpc<KindStatus[]>(
    'collect.status',
    { id },
    { invalidateOn: ['collect.updated'] },
  );
  const profiles = useRpc<ProfileList>('profiles.list', undefined, {
    invalidateOn: ['profiles.changed'],
  });
  const credentials = useRpc<Credential[]>('credentials.list', undefined, {
    invalidateOn: ['credentials.changed'],
  });

  const collect = useCollect();
  const backup = useBackup();
  // `=== true` on purpose: until `app.methods` has answered, leave the
  // button out rather than show one that may fail with `unknown_method`.
  const canBackUp = useHasMethod('config.backup') === true;
  const runTest = useRpcMutation<ConnectionTest, { id: string }>('devices.test', {
    errorToast: 'The test could not be run',
  });
  const remove = useRpcMutation<CountResult, { ids: string[] }>('devices.delete', {
    invalidates: ['devices.'],
    errorToast: 'The device was not deleted',
  });

  const data = device.data;
  const profileId = data ? effectiveProfile(data)?.id : undefined;
  const profile = profiles.data?.profiles.find((row) => row.id === profileId);
  /**
   * The device names a device type this build does not have. Everything it
   * could collect is then unknown, not impossible — and the page says which.
   */
  const missingProfile = profileId !== undefined && profiles.data !== undefined && !profile;
  const knownType = profile !== undefined;
  const profileName = (value: string) =>
    profiles.data?.profiles.find((row) => row.id === value)?.name ?? value;

  /** The kinds this device type knows how to collect at all. */
  const collectable = useMemo(() => new Set(profile?.collects ?? []), [profile]);
  const collected = useMemo(
    () => new Map((status.data ?? []).map((entry) => [entry.kind, entry])),
    [status.data],
  );

  usePageHeader({
    title: data?.name ?? id,
    breadcrumbs: [{ label: areaLabel, to: areaPath }],
  });

  // A fresh test result belongs to the device it was run on.
  useEffect(() => {
    setTest(null);
    setOpenKind(null);
  }, [id]);

  const onDelete = async () => {
    if (!data) return;
    const confirmed = await confirm({
      title: `Delete ${data.name}?`,
      description:
        'It is removed from Nocturne together with everything collected from it, its configuration history and its metrics. The device itself is not touched.',
      confirmLabel: 'Delete',
      tone: 'danger',
      confirmText: data.protected ? data.name : undefined,
    });
    if (!confirmed) return;
    await remove.mutateAsync({ ids: [data.id] });
    toast.success(`Deleted ${data.name}`);
    void navigate(areaPath);
  };

  const onTest = async () => {
    setTest(null);
    setTab('summary');
    setTest(await runTest.mutateAsync({ id }));
  };

  if (device.isError) {
    return (
      <Page>
        <ErrorState
          title={
            device.error.code === 'not_found' ? 'That device is gone' : 'The device could not be loaded'
          }
          error={device.error}
          onRetry={() => void device.refetch()}
          actions={<Button onClick={() => void navigate(areaPath)}>Back to devices</Button>}
        />
      </Page>
    );
  }

  if (!data) {
    return (
      <Page>
        <Skeleton width={260} height={24} />
        <Skeleton width={420} height={14} />
        <Skeleton height={240} shape="block" />
      </Page>
    );
  }

  const state = deviceStatus(data);
  const facts = data.state.facts;

  const identity: KeyValueItem[] = [
    { label: 'Address', value: data.address || null, mono: true },
    {
      label: 'Connection',
      value: `${TRANSPORT_LABEL[data.transport]}${data.port == null ? '' : ` · port ${data.port}`}`,
    },
    {
      label: 'Device type',
      value: !profileId ? null : missingProfile ? (
        <span className={styles.warnText}>{profileId} — not installed in this build</span>
      ) : (
        `${profileName(profileId)}${effectiveProfile(data)?.detected ? ' (detected)' : ''}`
      ),
    },
    { label: 'Site', value: data.site || null },
    { label: 'Role', value: data.role || null },
    { label: 'Tags', value: data.tags.length > 0 ? data.tags.join(', ') : null },
    {
      label: 'Credential',
      value: data.credential_id
        ? (credentials.data?.find((row) => row.id === data.credential_id)?.name ?? data.credential_id)
        : null,
    },
    {
      label: 'Jump host',
      value: data.jump_device_id ? <JumpHost id={data.jump_device_id} /> : null,
    },
    { label: 'Protected', value: data.protected ? 'Yes — changes need approval' : 'No' },
  ];

  const reachability: KeyValueItem[] = [
    { label: 'Last seen', value: data.state.last_seen ? <TimeAgo at={data.state.last_seen} /> : null },
    { label: 'Last checked', value: data.state.last_checked ? <TimeAgo at={data.state.last_checked} /> : null },
    {
      label: 'Round trip',
      value: data.state.rtt_ms == null ? null : `${data.state.rtt_ms.toFixed(1)} ms`,
    },
    { label: 'Monitoring', value: data.monitor.enabled ? `Every ${formatDuration(data.monitor.interval_s * 1000)}` : 'Off' },
    { label: 'Added', value: <TimeAgo at={data.created_at} /> },
  ];

  const hardware: KeyValueItem[] = [
    { label: 'Host name', value: facts?.hostname || null, mono: true },
    { label: 'Vendor', value: facts?.vendor || null },
    { label: 'Model', value: facts?.model || null, mono: true },
    { label: 'Operating system', value: facts?.os || null },
    { label: 'Version', value: facts?.os_version || null, mono: true },
    { label: 'Serial number', value: facts?.serial || null, mono: true },
    {
      label: 'Uptime',
      value: facts?.uptime_s == null ? null : formatDuration(facts.uptime_s * 1000),
    },
  ];

  return (
    <Page>
      <div className={styles.header}>
        <div className={styles.headRow}>
        <div className={styles.identity}>
          {/* The shell's header already carries the page's h1; this is the
              device's identity block, so it is one level down. */}
          <h2 className={styles.title}>{data.name}</h2>
          {data.lab && <LabPill />}
          <StatusPill tone={state.tone}>{state.label}</StatusPill>
          {data.protected && <Badge tone="maint" variant="outline">Protected</Badge>}
        </div>
        <div className={styles.actions}>
          <Button
            icon={Plug}
            loading={runTest.isPending}
            disabled={data.transport === 'none'}
            onClick={() => void onTest()}
          >
            Test
          </Button>
          <Button
            variant="primary"
            icon={RefreshCw}
            loading={collect.starting || collect.running}
            disabled={data.transport === 'none'}
            onClick={() => collect.start([data.id])}
          >
            Collect
          </Button>
          <DropdownMenu
            aria-label="More device actions"
            placement="bottom-end"
            trigger={<IconButton aria-label="More device actions" icon={Ellipsis} />}
            items={[
              { label: 'Edit device…', icon: Pencil, onSelect: () => setEditing(true) },
              ...(canBackUp
                ? [
                    {
                      label: 'Back up its configuration',
                      icon: Archive,
                      disabled: data.transport === 'none',
                      onSelect: () => backup.start([data.id]),
                    } as const,
                  ]
                : []),
              { type: 'separator' },
              {
                label: 'Delete device…',
                icon: Trash,
                tone: 'danger',
                onSelect: () => void onDelete(),
              },
            ]}
          />
        </div>
        </div>
        <div className={styles.subtitle}>
          <span className="mono">{data.address || EMPTY}</span>
          <span className={styles.dot}>{TRANSPORT_LABEL[data.transport]}</span>
          {data.site && <span className={styles.dot}>{data.site}</span>}
          {data.role && <span className={styles.dot}>{data.role}</span>}
          {profileId && <span className={styles.dot}>{profileName(profileId)}</span>}
        </div>
      </div>

      {data.state.last_error && (
        <p className={styles.error}>
          <strong>Last error:</strong> {data.state.last_error}
        </p>
      )}

      <CollectBanner collection={collect} />

      <Tabs
        id="device"
        aria-label="Device"
        tabs={TABS}
        value={tab}
        onChange={(next) => {
          setTab(next);
          setOpenKind(null);
        }}
      />

      {tab === 'summary' && (
        <TabPanel tabs="device" value="summary" className={styles.tabBody}>
          {runTest.isPending && (
            <div className={styles.testSlot}>
              <TestResultPending />
            </div>
          )}
          {!runTest.isPending && test && (
            <div className={styles.testSlot}>
              <TestResult result={test} profileName={profileName} />
            </div>
          )}

          <div className={styles.panels}>
            <Panel title="Device">
              <KeyValue items={identity} />
            </Panel>
            <Panel title="Reachability">
              <KeyValue items={reachability} />
            </Panel>
            <Panel
              title="Hardware and software"
              actions={
                data.state.facts_at ? (
                  <span className={styles.freshness}>
                    read <TimeAgo at={data.state.facts_at} />
                  </span>
                ) : undefined
              }
            >
              {facts ? (
                <KeyValue items={hardware} />
              ) : (
                <NeverCollected
                  kind="facts"
                  collectable={collectable.has('facts')}
                  knownType={knownType}
                  onCollect={() => collect.start([data.id], ['facts'])}
                />
              )}
            </Panel>
            <HealthPanel deviceId={data.id} />
            <div className={styles.wide}>
              <NeighboursPanel
                deviceId={data.id}
                kind="neighbors"
                collectable={collectable.has('neighbors')}
                knownType={knownType}
                onCollect={() => collect.start([data.id], ['neighbors'])}
              />
            </div>
          </div>
        </TabPanel>
      )}

      {tab === 'interfaces' && (
        <TabPanel tabs="device" value="interfaces" className={styles.tabBody}>
          <CollectedKind
            deviceId={data.id}
            kind="interfaces"
            collectable={collectable.has('interfaces')}
            knownType={knownType}
            onCollect={() => collect.start([data.id], ['interfaces'])}
            onRowActivate={(row) => setOpenInterface(row as Interface)}
            fill
          />
          <InterfaceDrawer
            open={openInterface !== null}
            row={openInterface}
            deviceId={data.id}
            onClose={() => setOpenInterface(null)}
          />
        </TabPanel>
      )}

      {tab === 'data' && openKind === null && (
        <TabPanel tabs="device" value="data" className={styles.tabBody}>
          <Toolbar>
            <span className={styles.freshness}>
              Everything Nocturne has read from this device.
              {profile
                ? ` ${profile.name} can read ${
                    collectable.size === KINDS.length
                      ? `all ${KINDS.length} kinds`
                      : `${formatCount(collectable.size)} of the ${KINDS.length} kinds`
                  }.`
                : missingProfile
                  ? ` Its device type “${profileId}” is not installed, so what it can read is unknown.`
                  : ' Its device type has not been worked out yet.'}
            </span>
            <ToolbarSpacer />
            <Button
              icon={RefreshCw}
              loading={collect.starting || collect.running}
              disabled={data.transport === 'none'}
              onClick={() => collect.start([data.id])}
            >
              Collect everything
            </Button>
          </Toolbar>
          <div className={styles.kinds}>
            {KINDS.map((kind) => (
              <KindCard
                key={kind}
                kind={kind}
                status={collected.get(kind)}
                collectable={collectable.has(kind)}
                knownType={knownType}
                busy={collect.running}
                onCollect={() => collect.start([data.id], [kind])}
                onOpen={() => setOpenKind(kind)}
              />
            ))}
          </div>
        </TabPanel>
      )}

      {tab === 'data' && openKind !== null && (
        <TabPanel tabs="device" value="data" className={styles.tabBody}>
          <Toolbar>
            <Button size="sm" icon={ArrowLeft} onClick={() => setOpenKind(null)}>
              All data
            </Button>
            <span className={styles.freshness}>{KIND_LABEL[openKind]}</span>
            <ToolbarSpacer />
            <Button
              size="sm"
              icon={RefreshCw}
              loading={collect.running}
              onClick={() => collect.start([data.id], [openKind])}
            >
              Collect this
            </Button>
          </Toolbar>
          <CollectedKind
            deviceId={data.id}
            kind={openKind}
            collectable={collectable.has(openKind)}
            knownType={knownType}
            onCollect={() => collect.start([data.id], [openKind])}
            fill
          />
        </TabPanel>
      )}

      {tab === 'notes' && (
        <TabPanel tabs="device" value="notes" className={styles.tabBody}>
          <Notes device={data} />
        </TabPanel>
      )}

      <DeviceDrawer
        open={editing}
        device={data}
        devices={inventory.data ?? []}
        onClose={() => setEditing(false)}
      />
    </Page>
  );
}

/** The jump host's name, so the page does not show a bare id. */
function JumpHost({ id }: { id: string }) {
  const device = useRpc<DeviceRow>('devices.get', { id });
  return <>{device.data?.name ?? id}</>;
}

interface MissingProps {
  kind: Kind;
  /** `page` centres it in a whole tab; `compact` fits inside a panel. */
  size?: 'page' | 'compact';
  /** The device's type defines steps for this kind. */
  collectable: boolean;
  /** The device's type is known at all. */
  knownType: boolean;
  onCollect?: () => void;
}

/**
 * Why there is nothing here: never collected, or a device type that has no
 * way to read this at all. The two are different and the user is told which.
 */
function NeverCollected({ kind, collectable, knownType, onCollect, size = 'compact' }: MissingProps) {
  if (knownType && !collectable) {
    return (
      <EmptyState
        size={size}
        icon={CircleAlert}
        title={`${KIND_LABEL[kind]} cannot be read from this device type`}
        description={`Its device-type profile defines no command for ${KIND_LABEL[kind].toLowerCase()}. Settings → Device types says what each type can read.`}
      />
    );
  }
  return (
    <EmptyState
      size={size}
      icon={Database}
      title={`${KIND_LABEL[kind]} ${KIND_PLURAL[kind] ? 'have' : 'has'} never been collected`}
      description={KIND_DESCRIPTION[kind]}
      action={
        onCollect && (
          <Button size="sm" variant="primary" icon={RefreshCw} onClick={onCollect}>
            Collect {KIND_LABEL[kind].toLowerCase()}
          </Button>
        )
      }
    />
  );
}

interface CollectedKindProps extends MissingProps {
  deviceId: string;
  fill?: boolean;
  onRowActivate?: (row: unknown) => void;
}

/** One kind as a table, with the state that says why it is not there. */
function CollectedKind({
  deviceId,
  kind,
  collectable,
  knownType,
  onCollect,
  fill,
  onRowActivate,
}: CollectedKindProps) {
  const stored = useRpc<Stored | null>(
    'collect.get',
    { id: deviceId, kind },
    { invalidateOn: ['collect.updated'] },
  );

  if (stored.isError) {
    return (
      <ErrorState
        size="compact"
        title={`${KIND_LABEL[kind]} could not be read from the database`}
        error={stored.error}
        onRetry={() => void stored.refetch()}
      />
    );
  }
  if (stored.isPending) return <Skeleton height={fill ? 260 : 140} shape="block" />;
  if (!stored.data) {
    return (
      <NeverCollected
        kind={kind}
        size={fill ? 'page' : 'compact'}
        collectable={collectable}
        knownType={knownType}
        onCollect={onCollect}
      />
    );
  }

  const rows = stored.data.rows;
  return (
    <>
      <div className={styles.freshness}>
        <span>
          Read <TimeAgo at={stored.data.collected_at} />
        </span>
        <span className={styles.source}>over {stored.data.source}</span>
        <span>
          {formatCount(rows.length)} row{rows.length === 1 ? '' : 's'}
        </span>
        <span className={styles.spacer} />
        {onCollect && (
          <Button size="sm" variant="ghost" icon={RefreshCw} onClick={onCollect}>
            Collect again
          </Button>
        )}
      </div>
      <div className={fill ? styles.fill : undefined}>
        <KindTable
          kind={kind}
          rows={rows}
          id={`device-${kind}`}
          fill={fill}
          onRowActivate={onRowActivate}
          empty={
            <EmptyState
              size="compact"
              icon={Database}
              title={`The device reported no ${KIND_LABEL[kind].toLowerCase()}`}
              description="The read succeeded; the table itself is empty on the device."
            />
          }
        />
      </div>
    </>
  );
}

/** A card per kind on the Data tab: its age, its size, and what to do about it. */
function KindCard({
  kind,
  status,
  collectable,
  knownType,
  busy,
  onCollect,
  onOpen,
}: {
  kind: Kind;
  status: KindStatus | undefined;
  collectable: boolean;
  knownType: boolean;
  busy: boolean;
  onCollect: () => void;
  onOpen: () => void;
}) {
  const unsupported = knownType && !collectable;
  const state = status ? 'present' : unsupported ? 'unsupported' : 'missing';

  return (
    <div className={styles.kind} data-state={state}>
      <div className={styles.kindHead}>
        <h3 className={styles.kindName}>{KIND_LABEL[kind]}</h3>
        {status && (
          <span className={styles.kindCount}>
            {formatCount(status.rows)} row{status.rows === 1 ? '' : 's'}
          </span>
        )}
      </div>

      {status ? (
        <p className={styles.kindText}>
          Read <TimeAgo at={status.collected_at} /> over {status.source}.
        </p>
      ) : unsupported ? (
        <p className={styles.kindText}>This device type has no command for it.</p>
      ) : (
        <p className={styles.kindText}>Never collected. {KIND_DESCRIPTION[kind]}</p>
      )}

      <div className={styles.kindFoot}>
        {status && (
          <Button size="sm" onClick={onOpen}>
            Open
          </Button>
        )}
        {!unsupported && (
          <Button
            size="sm"
            variant={status ? 'ghost' : 'secondary'}
            icon={RefreshCw}
            loading={busy}
            onClick={onCollect}
          >
            {status ? 'Collect again' : 'Collect'}
          </Button>
        )}
      </div>
    </div>
  );
}

/** Processor, memory and temperature, when the device was read for them. */
function HealthPanel({ deviceId }: { deviceId: string }) {
  const stored = useRpc<Stored | null>(
    'collect.get',
    { id: deviceId, kind: 'health' },
    { invalidateOn: ['collect.updated'] },
  );
  const health = stored.data?.rows[0] as Health | undefined;

  return (
    <Panel
      title="Health"
      actions={
        stored.data ? (
          <span className={styles.freshness}>
            read <TimeAgo at={stored.data.collected_at} />
          </span>
        ) : undefined
      }
    >
      {stored.isPending ? (
        <Skeleton height={72} shape="block" />
      ) : !health ? (
        <EmptyState
          size="compact"
          icon={Database}
          title="No health data"
          description="Processor, memory and temperature appear here once this device has been collected from."
        />
      ) : (
        <div className={styles.gauges}>
          <Gauge label="Processor" percent={health.cpu_pct} text={formatPercent(health.cpu_pct)} />
          <Gauge
            label="Memory"
            percent={
              health.memory_used != null && health.memory_total
                ? (health.memory_used / health.memory_total) * 100
                : null
            }
            text={
              health.memory_used == null
                ? EMPTY
                : health.memory_total == null
                  ? formatBytes(health.memory_used)
                  : `${formatBytes(health.memory_used)} of ${formatBytes(health.memory_total)}`
            }
          />
          <Gauge
            label="Temperature"
            percent={null}
            text={health.temperature_c == null ? EMPTY : `${health.temperature_c.toFixed(1)} °C`}
          />
        </div>
      )}
    </Panel>
  );
}

/** A level the device reported. A level it did not report shows no bar at all. */
function Gauge({
  label,
  percent,
  text,
}: {
  label: string;
  percent: number | null | undefined;
  text: string;
}) {
  const tone = percent == null ? 'idle' : percent >= 90 ? 'crit' : percent >= 75 ? 'warn' : 'ok';
  return (
    <div>
      <KeyValue layout="stacked" items={[{ label, value: text }]} />
      {percent != null && (
        <ProgressBar
          kind="meter"
          size="sm"
          tone={tone}
          value={percent}
          max={100}
          aria-label={label}
          valueText={formatPercent(percent)}
        />
      )}
    </div>
  );
}

/** What this device sees on its ports. */
function NeighboursPanel({ deviceId, collectable, knownType, onCollect }: MissingProps & { deviceId: string }) {
  const stored = useRpc<Stored | null>(
    'collect.get',
    { id: deviceId, kind: 'neighbors' },
    { invalidateOn: ['collect.updated'] },
  );

  return (
    <Panel
      title="Neighbours"
      padding="none"
      actions={
        stored.data ? (
          <span className={styles.freshness}>
            read <TimeAgo at={stored.data.collected_at} />
          </span>
        ) : undefined
      }
    >
      {stored.isPending ? (
        <Skeleton height={120} shape="block" />
      ) : !stored.data ? (
        <NeverCollected
          kind="neighbors"
          collectable={collectable}
          knownType={knownType}
          onCollect={onCollect}
        />
      ) : (
        <KindTable
          kind="neighbors"
          rows={stored.data.rows}
          id="device-neighbors"
          maxHeight={280}
          empty={
            <EmptyState
              size="compact"
              icon={Network}
              title="No neighbours were reported"
              description="The device answered, but it sees nothing on its ports over CDP or LLDP."
            />
          }
        />
      )}
    </Panel>
  );
}

/** Free text about the device, saved to `devices.update`. */
function Notes({ device }: { device: DeviceRow }) {
  const [text, setText] = useState(device.notes);
  const [savedAt, setSavedAt] = useState<number | null>(null);
  useEffect(() => setText(device.notes), [device.id, device.notes]);

  const save = useRpcMutation<DeviceRow, DeviceUpdateParams>('devices.update', {
    invalidates: ['devices.'],
    errorToast: 'The note was not saved',
  });

  const dirty = text !== device.notes;

  return (
    <div className={styles.notes}>
      <Textarea
        rows={12}
        aria-label={`Notes about ${device.name}`}
        placeholder="Anything the next person on shift should know about this device."
        value={text}
        onChange={(event) => setText(event.target.value)}
      />
      <div className={styles.notesFoot}>
        <Button
          variant="primary"
          loading={save.isPending}
          disabled={!dirty}
          onClick={() =>
            save.mutate(
              { id: device.id, notes: text },
              { onSuccess: () => setSavedAt(Date.now()) },
            )
          }
        >
          Save note
        </Button>
        {dirty ? (
          <span className={styles.saved}>Not saved yet.</span>
        ) : savedAt ? (
          <span className={styles.saved}>
            Saved <TimeAgo at={savedAt} />.
          </span>
        ) : null}
      </div>
    </div>
  );
}
