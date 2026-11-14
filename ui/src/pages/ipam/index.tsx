/**
 * IPAM — the plan, the network, and the difference between them.
 *
 * The whole screen is built on one distinction the backend's author made in
 * the data model and refuses to blur: a **record** is what a person wrote
 * down, an **observation** is what a device said, and nothing turns one into
 * the other by itself (`crates/nocturne-ipam/NOTES.md`,
 * `api/types/ipam.ts`). So:
 *
 * * a block with `source: "discovered"` is labelled as nobody's plan, and
 *   shows the interface it was inferred from rather than a bare badge;
 * * the address grid has four states, not two, and the empty one reads "no
 *   record, nothing seen" beside the engine's own note about what that does
 *   and does not mean, and beside what was actually collected;
 * * disagreements are shown as disagreements, with both sides and the
 *   evidence, and nothing offers to resolve one;
 * * reconciliation leads with coverage — "nobody looked" and "it is not there"
 *   are different sentences — and prints the engine's `refused` list, which is
 *   why there is no button on this screen that makes one of those decisions.
 *
 * The one write that turns evidence into records is `ipam.learn`, which
 * previews first; it is offered as an explicit act, in the reconciliation tab,
 * with the list of what it would create.
 *
 * IPAM is a feature crate, so this build may not have it: the screen asks
 * `app.methods` once and says so in a sentence rather than showing controls
 * that fail.
 */

import {
  FileUp,
  Layers,
  Network,
  Plus,
  RotateCw,
  ScanSearch,
  Search,
  ShieldAlert,
  Wand2,
} from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useSearchParams } from 'react-router';
import type {
  Address,
  Conflict,
  ImportResult,
  PrefixRow,
  SplitResult,
  VlanRow,
} from '@/api/types/ipam';
import { registerCommand } from '@/app/CommandPalette/commands';
import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { useStatusItem } from '@/app/StatusBar';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { Splitter } from '@/design/Splitter';
import { TabPanel, Tabs, type TabItem } from '@/design/Tabs';
import { toast } from '@/design/Toast';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { formatCount, plural } from '@/lib/format';
import { AddressDetail } from './AddressDetail';
import { AddressDialog, type AddressDialogMode } from './AddressDialog';
import { Conflicts } from './Conflicts';
import {
  useConflicts,
  useDeleteAddress,
  useDeletePrefix,
  useDeleteVlan,
  useExport,
  useImport,
  useIpamInstalled,
  useLabNames,
  useLearn,
  useNextAddress,
  useNextPrefix,
  usePrefix,
  usePrefixes,
  useReconcile,
  useReserveAddress,
  useSaveAddress,
  useSavePrefix,
  useSaveVlan,
  useSearch,
  useSplitPrefix,
  useSummary,
  useSync,
  useVlans,
  useDeviceNames,
} from './data';
import { FreeSpaceDialog } from './FreeSpaceDialog';
import styles from './ipam.module.css';
import { LearnDialog } from './LearnDialog';
import { PrefixDialog } from './PrefixDialog';
import { PrefixPanel } from './PrefixPanel';
import { PrefixTree, treeEmptyText } from './PrefixTree';
import { Reconcile } from './Reconcile';
import { SearchPanel } from './SearchPanel';
import { SplitDialog } from './SplitDialog';
import { TransferDialog, type TransferKind } from './TransferDialog';
import { groupPrefixes, siteLabel, sitesOf, visibleRows, type Group } from './tree';
import { VlanDialog } from './VlanDialog';
import { Vlans } from './Vlans';

const { label: areaLabel, icon: areaIcon } = destination('ipam');

const TABS: readonly TabItem[] = [
  { id: 'prefixes', label: 'Blocks', icon: Network },
  { id: 'vlans', label: 'VLANs', icon: Layers },
  { id: 'conflicts', label: 'Conflicts', icon: ShieldAlert },
  { id: 'reconcile', label: 'Reconcile', icon: ScanSearch },
];

type Tab = 'prefixes' | 'vlans' | 'conflicts' | 'reconcile';

function isTab(value: string | null): value is Tab {
  return value === 'prefixes' || value === 'vlans' || value === 'conflicts' || value === 'reconcile';
}

export default function IpamPage() {
  const installed = useIpamInstalled();
  if (installed === undefined) return <Loading />;
  if (!installed) return <NotInstalled />;
  return <Ipam />;
}

function Loading() {
  usePageHeader({ title: areaLabel });
  return (
    <Page>
      <Skeleton width={320} />
      <Skeleton shape="block" height={32} />
      <Skeleton shape="block" height={360} />
    </Page>
  );
}

function NotInstalled() {
  usePageHeader({ title: areaLabel });
  return (
    <Page>
      <EmptyState
        icon={areaIcon}
        title="IPAM is not in this build"
        description="This copy of Nocturne was assembled without the address-management feature, so there are no blocks, addresses or VLANs to keep. Nothing else is affected."
      />
    </Page>
  );
}

function Ipam() {
  const [params, setParams] = useSearchParams();
  const tab: Tab = isTab(params.get('tab')) ? (params.get('tab') as Tab) : 'prefixes';
  const setTab = (next: Tab) => {
    const copy = new URLSearchParams(params);
    if (next === 'prefixes') copy.delete('tab');
    else copy.set('tab', next);
    setParams(copy, { replace: true });
  };

  const [site, setSite] = useState('');
  const [query, setQuery] = useState('');
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [selectedIp, setSelectedIp] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const searchRef = useRef<HTMLInputElement>(null);

  // --------------------------------------------------------------- the reads

  const labNames = useLabNames();
  const deviceName = useDeviceNames();
  const summary = useSummary();
  const prefixes = usePrefixes(useMemo(() => ({ ...(site ? { site } : {}) }), [site]));
  const detail = usePrefix(selectedId);
  const search = useSearch(query);
  const conflicts = useConflicts(tab === 'conflicts');
  const reconcile = useReconcile(site, tab === 'reconcile');

  // -------------------------------------------------------------- the writes

  const savePrefix = useSavePrefix();
  const deletePrefix = useDeletePrefix();
  const splitPrefix = useSplitPrefix();
  const saveAddress = useSaveAddress();
  const reserveAddress = useReserveAddress();
  const deleteAddress = useDeleteAddress();
  const saveVlan = useSaveVlan();
  const deleteVlan = useDeleteVlan();
  const sync = useSync();
  const learn = useLearn();
  const importCsv = useImport();

  // ---------------------------------------------------------- dialog state

  const [prefixDialog, setPrefixDialog] = useState<{
    open: boolean;
    prefix: PrefixRow | null;
    cidr: string;
  }>({ open: false, prefix: null, cidr: '' });
  const [addressDialog, setAddressDialog] = useState<{
    open: boolean;
    mode: AddressDialogMode;
    ip: string;
    address: Address | null;
    prefixId: string | null;
  }>({ open: false, mode: 'record', ip: '', address: null, prefixId: null });
  const [splitting, setSplitting] = useState(false);
  const [freeSpace, setFreeSpace] = useState<{ open: boolean; length: number; count: number }>({
    open: false,
    length: 24,
    count: 3,
  });
  const [transfer, setTransfer] = useState<{
    open: boolean;
    tab: 'import' | 'export';
    kind: TransferKind;
    text: string;
    includeObserved: boolean;
  }>({ open: false, tab: 'import', kind: 'prefixes', text: '', includeObserved: false });
  const [learning, setLearning] = useState<{ open: boolean; includeArp: boolean }>({
    open: false,
    includeArp: false,
  });
  const [vlanDialog, setVlanDialog] = useState<{ open: boolean; row: VlanRow | null }>({
    open: false,
    row: null,
  });

  const selectedPrefix = detail.data?.prefix ?? null;

  const nextPrefix = useNextPrefix(
    freeSpace.open && selectedId ? { parent_id: selectedId, len: freeSpace.length } : null,
  );
  const nextAddresses = useNextAddress(
    freeSpace.open && selectedId ? { prefix_id: selectedId, count: freeSpace.count } : null,
  );
  const exported = useExport(
    transfer.open && transfer.tab === 'export'
      ? { kind: transfer.kind, include_observed: transfer.includeObserved }
      : null,
  );

  // ------------------------------------------------------------ derived data

  const rows = prefixes.data ?? [];
  const groups = useMemo(() => groupPrefixes(rows, labNames), [rows, labNames]);
  const sites = useMemo(() => sitesOf(rows, labNames), [rows, labNames]);
  const rowsOf = (group: Group) => visibleRows(group, collapsed);

  // The first block in address order, so the panel is never empty for no
  // reason; a selection the user made always wins.
  useEffect(() => {
    if (selectedId !== null && rows.some((row) => row.id === selectedId)) return;
    const first = groups[0]?.roots[0]?.row.id ?? null;
    setSelectedId(first);
  }, [groups, rows, selectedId]);

  const selectedAddress: Address | null = useMemo(() => {
    if (selectedIp === null) return null;
    return detail.data?.addresses.find((address) => address.ip === selectedIp) ?? null;
  }, [detail.data, selectedIp]);

  const scope = selectedPrefix?.site?.startsWith('lab:') ? selectedPrefix.site : '';

  /**
   * The site of a block, so an address hit knows which namespace it is in.
   * The search answer carries the blocks it matched; the tree has the rest.
   */
  const siteOf = useMemo(() => {
    const sites = new Map<string, string | undefined>();
    for (const row of [...rows, ...(search.data?.prefixes ?? [])]) sites.set(row.id, row.site);
    return (prefixId: string) => sites.get(prefixId);
  }, [rows, search.data]);

  usePageHeader({
    title: areaLabel,
    actions: (
      <Button
        icon={Plus}
        variant="primary"
        onClick={() => setPrefixDialog({ open: true, prefix: null, cidr: '' })}
      >
        Add a block
      </Button>
    ),
  });

  // The one honest summary of this screen's own completeness.
  useStatusItem(
    summary.data
      ? {
          id: 'ipam.summary',
          order: 40,
          label: `${formatCount(summary.data.prefixes)} blocks · ${formatCount(
            summary.data.addresses,
          )} records · ${formatCount(summary.data.observations)} sightings`,
          tone:
            summary.data.conflicts > 0
              ? 'crit'
              : summary.data.devices_never_collected > 0
                ? 'warn'
                : 'ok',
          detail:
            summary.data.devices_never_collected > 0
              ? `${plural(summary.data.devices_never_collected, 'device')} of ${formatCount(
                  summary.data.devices_total,
                )} have never been collected from, so everything here is only that complete.`
              : 'Records are what people wrote down; sightings are what devices reported. They are counted separately.',
          onSelect: () => setTab('reconcile'),
        }
      : null,
  );

  useEffect(
    () =>
      registerCommand({
        id: 'ipam.search',
        title: 'Search blocks, addresses and VLANs',
        group: 'IPAM',
        keywords: ['ip', 'subnet', 'prefix', 'mac', 'host'],
        shortcut: '/',
        icon: Search,
        run: () => searchRef.current?.focus(),
      }),
    [],
  );

  useEffect(
    () =>
      registerCommand({
        id: 'ipam.add',
        title: 'Add an address block',
        group: 'IPAM',
        keywords: ['prefix', 'subnet', 'document'],
        icon: Plus,
        run: () => setPrefixDialog({ open: true, prefix: null, cidr: '' }),
      }),
    [],
  );

  useEffect(
    () =>
      registerCommand({
        id: 'ipam.sync',
        title: 'Re-read the collected addresses',
        group: 'IPAM',
        keywords: ['sync', 'arp', 'interfaces', 'refresh'],
        icon: RotateCw,
        run: () => runSync(),
      }),
    [],
  );

  useEffect(
    () =>
      registerCommand({
        id: 'ipam.reconcile',
        title: 'Compare the plan with the network',
        group: 'IPAM',
        keywords: ['reconcile', 'coverage', 'drift'],
        icon: ScanSearch,
        run: () => setTab('reconcile'),
      }),
    [],
  );

  // ---------------------------------------------------------------- actions

  const runSync = () => {
    sync.mutate(
      {},
      {
        onSuccess: (result) => {
          toast.success('The collected addresses were read again', {
            description: `${plural(result.addresses_updated, 'sighting')} recorded · ${plural(
              result.prefixes_added,
              'block',
            )} discovered · ${plural(result.addresses_aged_out, 'ARP sighting')} forgotten${
              result.conflicts.length > 0
                ? ` · ${plural(result.conflicts.length, 'conflict')} found`
                : ''
            }`,
          });
        },
      },
    );
  };

  /**
   * Free space is asked about one block, so the child length it starts from is
   * that block's own length plus one — from the toolbar as well as from the
   * panel, which otherwise asked for a /24 inside a /24.
   */
  const openFreeSpace = () => {
    const cidr = selectedPrefix?.cidr;
    const length = cidr
      ? Math.min((Number(cidr.split('/')[1] ?? 24) || 24) + 1, cidr.includes(':') ? 128 : 32)
      : freeSpace.length;
    setFreeSpace({ ...freeSpace, open: true, length });
  };

  const toggle = (id: string, open: boolean) => {
    setCollapsed((current) => {
      const next = new Set(current);
      if (open) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const openPrefix = (id: string) => {
    setSelectedId(id);
    setQuery('');
    setTab('prefixes');
  };

  const removePrefix = async (row: PrefixRow) => {
    const sure = await confirm({
      title: `Delete the record for ${row.cidr}?`,
      description:
        `Only the record goes. Child blocks are kept and hang off its parent, address records inside it point at the parent instead, and every sighting stays exactly as it was — no device is touched.` +
        (row.source === 'discovered'
          ? ' This block was discovered from a sighting, so the next sync will find it again.'
          : ''),
      confirmLabel: 'Delete the record',
      tone: 'danger',
    });
    if (!sure) return;
    deletePrefix.mutate(
      { id: row.id },
      {
        onSuccess: (result) => {
          setSelectedId(null);
          toast.success(`Deleted the record for ${result.cidr}`, {
            description: `${plural(result.children_reparented, 'child block')} reparented · ${plural(
              result.addresses_repointed,
              'address record',
            )} repointed`,
          });
        },
      },
    );
  };

  const removeAddress = async (address: Address) => {
    const sure = await confirm({
      title: `Delete the record for ${address.ip}?`,
      description:
        'The record goes; every sighting of this address stays, so it may still be listed as seen on the network.',
      confirmLabel: 'Delete the record',
      tone: 'danger',
    });
    if (!sure) return;
    deleteAddress.mutate(
      { ip: address.ip, scope },
      {
        onSuccess: (result) => {
          setSelectedIp(null);
          toast.success(`Deleted the record for ${result.ip}`, {
            description:
              result.observations_left > 0
                ? `${plural(result.observations_left, 'sighting')} of it remain.`
                : 'Nothing has ever reported this address.',
          });
        },
      },
    );
  };

  const removeVlan = async (row: VlanRow) => {
    const sure = await confirm({
      title: `Delete the record for VLAN ${row.vid}?`,
      description:
        'Only the record goes. Devices that report this VLAN still report it, and it keeps its row here as a VLAN nobody has documented.',
      confirmLabel: 'Delete the record',
      tone: 'danger',
    });
    if (!sure) return;
    deleteVlan.mutate({ id: row.id });
  };

  const [splitResult, setSplitResult] = useState<SplitResult | null>(null);
  const [importResult, setImportResult] = useState<ImportResult | null>(null);

  const runSplit = (request: Parameters<typeof splitPrefix.mutate>[0]) => {
    splitPrefix.mutate(request, {
      onSuccess: (result) => {
        setSplitResult(result);
        if (!result.dry_run) {
          toast.success(`Created ${plural(result.created, 'block')} inside ${result.parent}`);
        }
      },
    });
  };

  const runImport = (commit: boolean) => {
    importCsv.mutate(
      { kind: transfer.kind, text: transfer.text, dry_run: !commit },
      {
        onSuccess: (result) => {
          setImportResult(result);
          if (!result.dry_run) {
            toast.success(
              `Imported ${formatCount(result.created)} new and ${formatCount(result.updated)} updated ${result.kind}`,
            );
          }
        },
      },
    );
  };

  const runLearn = (commit: boolean) => {
    learn.mutate(
      { dry_run: !commit, include_arp: learning.includeArp },
      {
        onSuccess: (plan) => {
          if (!plan.dry_run) {
            toast.success('The records were written', {
              description: `${plural(plan.created_prefixes, 'block')} · ${plural(
                plan.created_addresses,
                'address',
                'addresses',
              )} · ${plural(plan.created_vlans, 'VLAN')}`,
            });
          }
        },
      },
    );
  };

  const openConflictSubject = (conflict: Conflict) => {
    const prefixId = conflict.involved.find((side) => side.prefix_id)?.prefix_id;
    if (prefixId) {
      openPrefix(prefixId);
      return;
    }
    // An address conflict: the search takes it to its record and its block.
    setQuery(conflict.subject);
    setTab('prefixes');
  };

  // ----------------------------------------------------------------- render

  const toolbar = (
    <Toolbar className={styles.toolbar}>
      {sites.length > 1 && (
        <Select
          size="sm"
          className={styles.filter}
          aria-label="Site"
          value={site}
          options={[{ value: '', label: 'Every site' }, ...sites]}
          onChange={(event) => setSite(event.target.value)}
        />
      )}
      <SearchInput
        ref={searchRef}
        size="sm"
        className={styles.search}
        aria-label="Search blocks, addresses and VLANs"
        placeholder="Address, MAC, host, tag…"
        value={query}
        onChange={setQuery}
      />
      <ToolbarSpacer />
      <Button
        size="sm"
        variant="ghost"
        icon={Wand2}
        disabled={selectedId === null}
        onClick={openFreeSpace}
      >
        Free space
      </Button>
      <Button
        size="sm"
        variant="ghost"
        icon={FileUp}
        onClick={() => setTransfer({ ...transfer, open: true })}
      >
        Import / export
      </Button>
      <Button
        size="sm"
        variant="secondary"
        icon={RotateCw}
        loading={sync.isPending}
        onClick={runSync}
      >
        Re-read the devices
      </Button>
    </Toolbar>
  );

  const tabs = (
    <Tabs
      id="ipam"
      className={styles.tabs}
      tabs={TABS.map((entry) =>
        entry.id === 'conflicts' && summary.data && summary.data.conflicts > 0
          ? {
              ...entry,
              badge: (
                <Badge tone="crit" variant="soft">
                  {formatCount(summary.data.conflicts)}
                </Badge>
              ),
            }
          : entry,
      )}
      value={tab}
      onChange={(next) => setTab(next as Tab)}
      aria-label="IPAM views"
    />
  );

  return (
    <Page flush>
      {toolbar}
      {tabs}

      <TabPanel tabs="ipam" value={tab} className={styles.body}>
        {tab === 'prefixes' && (
          <Splitter
            sized="first"
            defaultSize={600}
            minSize={360}
            maxSize={900}
            storageId="ipam.tree"
            handleLabel="Resize the block list"
            className={styles.split}
          >
            <div className={styles.treePane}>
              {prefixes.isError ? (
                <ErrorState
                  title="The blocks could not be read"
                  error={prefixes.error}
                  onRetry={() => void prefixes.refetch()}
                  size="compact"
                />
              ) : prefixes.isPending ? (
                <div className={styles.loading} aria-busy="true">
                  <Skeleton shape="block" height={28} />
                  <Skeleton shape="block" height={28} />
                  <Skeleton shape="block" height={28} />
                </div>
              ) : rows.length === 0 ? (
                <EmptyState
                  icon={Network}
                  size="compact"
                  title={site === '' ? 'No block yet' : 'No block in this site'}
                  description={treeEmptyText(rows, site !== '')}
                  action={
                    <Button
                      variant="primary"
                      icon={Plus}
                      onClick={() => setPrefixDialog({ open: true, prefix: null, cidr: '' })}
                    >
                      Add a block
                    </Button>
                  }
                />
              ) : (
                <PrefixTree
                  groups={groups}
                  collapsed={collapsed}
                  onToggle={toggle}
                  selected={selectedId}
                  onSelect={(id) => {
                    setSelectedId(id);
                    setSelectedIp(null);
                  }}
                  rowsOf={rowsOf}
                />
              )}
            </div>

            <div className={styles.detailPane}>
              {query.trim() !== '' ? (
                search.isPending ? (
                  <div className={styles.loading} aria-busy="true">
                    <Skeleton shape="block" height={64} />
                  </div>
                ) : search.isError ? (
                  <ErrorState
                    title="That search could not be run"
                    error={search.error}
                    onRetry={() => void search.refetch()}
                    size="compact"
                  />
                ) : search.data ? (
                  <SearchPanel
                    query={query}
                    result={search.data}
                    labNames={labNames}
                    siteOf={siteOf}
                    onOpenPrefix={openPrefix}
                    onOpenAddress={(ip) => {
                      const containing = search.data?.prefixes[0]?.id ?? null;
                      if (containing) setSelectedId(containing);
                      setQuery('');
                      setSelectedIp(ip);
                    }}
                    onClear={() => setQuery('')}
                  />
                ) : null
              ) : detail.isError ? (
                <ErrorState
                  title="That block could not be read"
                  error={detail.error}
                  onRetry={() => void detail.refetch()}
                  size="compact"
                />
              ) : detail.isPending || !detail.data ? (
                selectedId === null ? (
                  <EmptyState
                    icon={Network}
                    size="compact"
                    title="Pick a block"
                    description="Its addresses, its free runs, the interfaces configured in it and what was collected about it appear here."
                  />
                ) : (
                  <div className={styles.loading} aria-busy="true">
                    <Skeleton shape="block" height={48} />
                    <Skeleton shape="block" height={160} />
                  </div>
                )
              ) : (
                <PrefixPanel
                  detail={detail.data}
                  labNames={labNames}
                  selectedAddress={selectedIp}
                  onSelectAddress={setSelectedIp}
                  onEdit={() =>
                    setPrefixDialog({ open: true, prefix: detail.data.prefix, cidr: '' })
                  }
                  onSplit={() => {
                    setSplitResult(null);
                    setSplitting(true);
                  }}
                  onFreeSpace={openFreeSpace}
                  onReserve={() =>
                    setAddressDialog({
                      open: true,
                      mode: 'reserve',
                      ip: '',
                      address: null,
                      prefixId: detail.data.prefix.id,
                    })
                  }
                  onAddRecord={() =>
                    setAddressDialog({
                      open: true,
                      mode: 'record',
                      ip: '',
                      address: null,
                      prefixId: detail.data.prefix.id,
                    })
                  }
                  onDelete={() => void removePrefix(detail.data.prefix)}
                />
              )}
            </div>
          </Splitter>
        )}

        {tab === 'vlans' && (
          <VlansTab
            site={site}
            labNames={labNames}
            onEdit={(row) => setVlanDialog({ open: true, row })}
            onDelete={(row) => void removeVlan(row)}
          />
        )}

        {tab === 'conflicts' && (
          <div className={styles.pane}>
            {conflicts.isError ? (
              <ErrorState
                title="The conflicts could not be read"
                error={conflicts.error}
                onRetry={() => void conflicts.refetch()}
                size="compact"
              />
            ) : conflicts.isPending ? (
              <div className={styles.loading} aria-busy="true">
                <Skeleton shape="block" height={96} />
                <Skeleton shape="block" height={96} />
              </div>
            ) : (
              <Conflicts
                conflicts={conflicts.data ?? []}
                onOpenSubject={openConflictSubject}
              />
            )}
          </div>
        )}

        {tab === 'reconcile' && (
          <div className={styles.pane}>
            {reconcile.isError ? (
              <ErrorState
                title="The plan and the network could not be compared"
                error={reconcile.error}
                onRetry={() => void reconcile.refetch()}
                size="compact"
              />
            ) : reconcile.isPending || !reconcile.data ? (
              <div className={styles.loading} aria-busy="true">
                <Skeleton shape="block" height={120} />
                <Skeleton shape="block" height={200} />
              </div>
            ) : (
              <Reconcile
                report={reconcile.data}
                labNames={labNames}
                deviceName={deviceName}
                refreshing={reconcile.isFetching}
                learning={learn.isPending}
                onRefresh={() => void reconcile.refetch()}
                onLearn={() => {
                  setLearning({ ...learning, open: true });
                  runLearn(false);
                }}
                onOpenPrefix={openPrefix}
                onWriteRecord={(ip, prefixId) =>
                  setAddressDialog({
                    open: true,
                    mode: 'record',
                    ip,
                    address: null,
                    prefixId,
                  })
                }
              />
            )}
          </div>
        )}
      </TabPanel>

      <AddressDetail
        open={selectedIp !== null && selectedAddress !== null}
        address={selectedAddress}
        cidr={selectedPrefix?.cidr ?? null}
        lab={scope !== ''}
        deviceName={deviceName}
        deleting={deleteAddress.isPending}
        onClose={() => setSelectedIp(null)}
        onEdit={(address) =>
          setAddressDialog({
            open: true,
            mode: 'record',
            ip: address.ip,
            address,
            prefixId: selectedPrefix?.id ?? null,
          })
        }
        onDelete={(address) => void removeAddress(address)}
      />

      <PrefixDialog
        open={prefixDialog.open}
        prefix={prefixDialog.prefix}
        suggestedCidr={prefixDialog.cidr}
        defaultSite={site.startsWith('lab:') ? '' : site}
        saving={savePrefix.isPending}
        error={savePrefix.error}
        saved={savePrefix.data ?? null}
        onClose={() => {
          setPrefixDialog({ open: false, prefix: null, cidr: '' });
          savePrefix.reset();
        }}
        onSave={(input) =>
          savePrefix.mutate(input, {
            onSuccess: (saved) => {
              setSelectedId(saved.prefix.id);
              if (saved.warnings.length === 0) {
                setPrefixDialog({ open: false, prefix: null, cidr: '' });
                savePrefix.reset();
              }
              toast.success(
                saved.adopted
                  ? `${saved.prefix.cidr} is documented — the discovered block was adopted`
                  : saved.created
                    ? `${saved.prefix.cidr} is in the plan`
                    : `${saved.prefix.cidr} was updated`,
              );
            },
          })
        }
      />

      <AddressDialog
        open={addressDialog.open}
        mode={addressDialog.mode}
        prefixId={addressDialog.prefixId}
        prefixCidr={selectedPrefix?.cidr ?? null}
        address={addressDialog.address}
        suggestedIp={addressDialog.ip}
        scope={scope}
        saving={saveAddress.isPending || reserveAddress.isPending}
        error={saveAddress.error ?? reserveAddress.error}
        saved={saveAddress.data ?? reserveAddress.data ?? null}
        onClose={() => {
          setAddressDialog({ ...addressDialog, open: false });
          saveAddress.reset();
          reserveAddress.reset();
        }}
        onSave={(input) =>
          saveAddress.mutate(input, {
            onSuccess: (saved) => {
              setAddressDialog({ ...addressDialog, open: false });
              saveAddress.reset();
              toast.success(
                saved.created
                  ? `${saved.address.ip} is documented`
                  : `The record for ${saved.address.ip} was updated`,
              );
            },
          })
        }
        onReserve={(request) =>
          reserveAddress.mutate(request, {
            onSuccess: (saved) => {
              setAddressDialog({ ...addressDialog, open: false });
              reserveAddress.reset();
              toast.success(`${saved.address.ip} is reserved`);
            },
          })
        }
      />

      <SplitDialog
        open={splitting}
        prefix={selectedPrefix}
        result={splitResult}
        pending={splitPrefix.isPending}
        error={splitPrefix.error}
        onClose={() => {
          setSplitting(false);
          setSplitResult(null);
          splitPrefix.reset();
        }}
        onRun={runSplit}
      />

      <FreeSpaceDialog
        open={freeSpace.open}
        prefix={selectedPrefix}
        length={freeSpace.length}
        count={freeSpace.count}
        onLength={(length) => setFreeSpace({ ...freeSpace, length })}
        onCount={(count) => setFreeSpace({ ...freeSpace, count })}
        nextPrefix={nextPrefix.data}
        nextPrefixError={nextPrefix.error}
        nextAddresses={nextAddresses.data}
        nextAddressError={nextAddresses.error}
        loading={nextPrefix.isFetching || nextAddresses.isFetching}
        onClose={() => setFreeSpace({ ...freeSpace, open: false })}
        onUsePrefix={(cidr) => {
          setFreeSpace({ ...freeSpace, open: false });
          setPrefixDialog({ open: true, prefix: null, cidr });
        }}
        onReserve={(ip) => {
          setFreeSpace({ ...freeSpace, open: false });
          setAddressDialog({
            open: true,
            mode: 'reserve',
            ip,
            address: null,
            prefixId: selectedId,
          });
        }}
      />

      <TransferDialog
        open={transfer.open}
        tab={transfer.tab}
        onTab={(next) => setTransfer({ ...transfer, tab: next })}
        kind={transfer.kind}
        onKind={(kind) => {
          setTransfer({ ...transfer, kind });
          setImportResult(null);
          importCsv.reset();
        }}
        text={transfer.text}
        onText={(text) => setTransfer({ ...transfer, text })}
        includeObserved={transfer.includeObserved}
        onIncludeObserved={(includeObserved) => setTransfer({ ...transfer, includeObserved })}
        preview={importResult}
        importing={importCsv.isPending}
        importError={importCsv.error}
        exported={exported.data}
        exportError={exported.error}
        onClose={() => {
          setTransfer({ ...transfer, open: false });
          setImportResult(null);
          importCsv.reset();
        }}
        onImport={runImport}
      />

      <LearnDialog
        open={learning.open}
        plan={learn.data ?? null}
        siteLabel={(site) => siteLabel(site, labNames) ?? 'no site'}
        includeArp={learning.includeArp}
        onIncludeArp={(includeArp) => {
          setLearning({ ...learning, includeArp });
          learn.mutate({ dry_run: true, include_arp: includeArp });
        }}
        pending={learn.isPending}
        error={learn.error}
        onClose={() => {
          setLearning({ ...learning, open: false });
          learn.reset();
        }}
        onRun={runLearn}
      />

      <VlanDialog
        open={vlanDialog.open}
        row={vlanDialog.row}
        defaultSite={site}
        prefixes={rows}
        saving={saveVlan.isPending}
        error={saveVlan.error}
        onClose={() => {
          setVlanDialog({ open: false, row: null });
          saveVlan.reset();
        }}
        onSave={(input) =>
          saveVlan.mutate(input, {
            onSuccess: (row) => {
              setVlanDialog({ open: false, row: null });
              saveVlan.reset();
              toast.success(`VLAN ${row.vid} is documented`);
            },
          })
        }
      />
    </Page>
  );
}

/** The VLANs tab is its own component so its read starts when it is opened. */
function VlansTab({
  site,
  labNames,
  onEdit,
  onDelete,
}: {
  site: string;
  labNames: ReadonlyMap<string, string>;
  onEdit: (row: VlanRow | null) => void;
  onDelete: (row: VlanRow) => void;
}) {
  const vlans = useVlans(site);
  if (vlans.isError) {
    return (
      <div className={styles.pane}>
        <ErrorState
          title="The VLANs could not be read"
          error={vlans.error}
          onRetry={() => void vlans.refetch()}
          size="compact"
        />
      </div>
    );
  }
  return (
    <div className={styles.pane}>
      <Vlans
        rows={vlans.data ?? []}
        labNames={labNames}
        loading={vlans.isPending}
        onEdit={onEdit}
        onDelete={onDelete}
      />
    </div>
  );
}

