/**
 * Configuration — what every device actually says, what changed, whether it
 * is still what we meant, and who else has a given line.
 *
 * The route is `config/:tab?`; everything that makes a view worth sharing is
 * in the query string (`?device=`, `?kind=`, `?q=`, `?regex=1`), so a search
 * result or one device's history can be pasted into a ticket. The backend
 * contract is `docs/dev/features/config.md` and `src/api/types/config.ts`.
 *
 * Two rules from CLAUDE.md shape this screen more than anything in the spec:
 *
 * - A configuration is redacted before a human sees it, by default. The
 *   backend does that; this screen never undoes it, says where a secret was
 *   taken out, and asks once per session before showing one — which the
 *   backend records in the audit trail.
 * - A configuration that was never captured is not an empty configuration.
 *   "Never captured", "captured and empty", "could not be read" and "not
 *   backed up on purpose" are four states with four different screens.
 */

import { Camera, FileClock, HardDriveDownload, Search, ShieldCheck, Target } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate, useParams, useSearchParams } from 'react-router';
import type { ConfigKind } from '@/api/types/config';
import { registerCommand } from '@/app/CommandPalette/commands';
import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { Skeleton } from '@/design/Skeleton';
import { TabPanel, Tabs, type TabItem } from '@/design/Tabs';
import { Toolbar } from '@/design/Toolbar';
import { plural } from '@/lib/format';
import { BackupsTab } from './Backups';
import { ComplianceTab } from './Compliance';
import styles from './Config.module.css';
import { useBackup, useCoverage, useUnsaved } from './data';
import { HistoryTab } from './History';
import { IntendedTab } from './Intended';
import { useMethods } from './methods';
import { NetworkSearchTab } from './NetworkSearch';

const { label: areaLabel, path: areaPath, icon: areaIcon } = destination('config');

const TABS: readonly TabItem[] = [
  { id: 'backups', label: 'Backups', icon: HardDriveDownload },
  { id: 'history', label: 'History', icon: FileClock },
  { id: 'search', label: 'Search', icon: Search },
  { id: 'compliance', label: 'Compliance', icon: ShieldCheck },
  { id: 'intended', label: 'Intended', icon: Target },
];

const DEFAULT_TAB = 'backups';

export default function ConfigPage() {
  const methods = useMethods();
  if (methods === undefined) return <Loading />;
  if (!methods.includes('config.devices')) return <NotInstalled />;
  return <Config canHandOver={methods.includes('changes.create')} />;
}

function Loading() {
  usePageHeader({ title: areaLabel });
  return (
    <Page>
      <Skeleton shape="block" height={32} width={420} />
      <Skeleton shape="block" height={320} />
    </Page>
  );
}

function NotInstalled() {
  usePageHeader({ title: areaLabel });
  return (
    <Page>
      <EmptyState
        icon={areaIcon}
        title="Configuration management is not in this build"
        description="This copy of Nocturne was assembled without the configuration feature, so nothing is backed up and no compliance rules are checked. The rest of the application is unaffected."
      />
    </Page>
  );
}

function Config({ canHandOver }: { canHandOver: boolean }) {
  const { tab } = useParams();
  const [search, setSearch] = useSearchParams();
  const navigate = useNavigate();
  const current = TABS.some((entry) => entry.id === tab) ? (tab as string) : DEFAULT_TAB;

  const coverage = useCoverage();
  const unsaved = useUnsaved();
  const backup = useBackup();
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const searchFieldRef = useRef<HTMLInputElement>(null);

  const deviceId = search.get('device');
  const kind = (search.get('kind') === 'startup' ? 'startup' : 'running') as ConfigKind;
  const query = search.get('q') ?? '';
  const regex = search.get('regex') === '1';
  const matchCase = search.get('case') === '1';

  /** One place that writes the query string, so a change never drops the rest. */
  const put = useCallback(
    (changes: Record<string, string | null>) => {
      setSearch(
        (previous) => {
          const next = new URLSearchParams(previous);
          for (const [key, value] of Object.entries(changes)) {
            if (value === null || value === '') next.delete(key);
            else next.set(key, value);
          }
          return next;
        },
        { replace: true },
      );
    },
    [setSearch],
  );

  const goTab = (next: string) => navigate(`${areaPath}/${next}?${search.toString()}`);

  const openDevice = useCallback(
    (id: string) => {
      put({ device: id });
      navigate(`${areaPath}/history?device=${encodeURIComponent(id)}&kind=${kind}`);
    },
    [put, navigate, kind],
  );

  const rows = coverage.data ?? [];
  const backable = useMemo(() => rows.filter((row) => !row.skipped), [rows]);

  /** Backs up a set of devices, saying first exactly which. */
  const backUp = useCallback(
    async (ids: readonly string[], kinds?: readonly ConfigKind[]) => {
      if (ids.length === 0) return;
      const names = ids
        .map((id) => rows.find((row) => row.device_id === id)?.name ?? id)
        .slice(0, 5);
      const ok = await confirm({
        title: ids.length === 1 ? `Back up ${names[0]}?` : `Back up ${plural(ids.length, 'device')}?`,
        description: `Nocturne opens a read-only session to ${
          ids.length === 1 ? names[0] : `${plural(ids.length, 'device')} (${names.join(', ')}${ids.length > names.length ? ', …' : ''})`
        } and reads ${
          kinds?.length === 1 ? `the ${kinds[0]} configuration` : 'its configuration'
        }. Nothing is changed on any device. A capture identical to the last one is not stored twice.`,
        confirmLabel: ids.length === 1 ? 'Back it up' : `Back up ${ids.length}`,
      });
      if (ok) backup.start(ids, kinds);
    },
    [rows, backup],
  );

  usePageHeader({ title: areaLabel });

  // The palette reaches the three things people come here to do.
  useEffect(() => {
    const registered = [
      registerCommand({
        id: 'config.search',
        title: 'Search every configuration',
        group: 'Configuration',
        keywords: ['grep', 'find', 'line', 'who has'],
        icon: Search,
        run: () => navigate(`${areaPath}/search`),
      }),
      registerCommand({
        id: 'config.backup.all',
        title: 'Back up every device',
        group: 'Configuration',
        keywords: ['capture', 'snapshot'],
        icon: Camera,
        run: () => void backUp(backable.map((row) => row.device_id)),
      }),
      registerCommand({
        id: 'config.compliance',
        title: 'Check the configurations against the policies',
        group: 'Configuration',
        keywords: ['compliance', 'hardening', 'audit'],
        icon: ShieldCheck,
        run: () => navigate(`${areaPath}/compliance`),
      }),
    ];
    return () => {
      for (const remove of registered) remove();
    };
  }, [navigate, backUp, backable]);

  // `/` focuses the screen's search, wherever the screen has one.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== '/' || event.defaultPrevented) return;
      const target = event.target as HTMLElement | null;
      if (target && /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName)) return;
      if (target?.isContentEditable) return;
      if (current !== 'search') {
        navigate(`${areaPath}/search`);
        event.preventDefault();
        return;
      }
      searchFieldRef.current?.focus();
      event.preventDefault();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [current, navigate]);

  const chosen = [...selected].filter((id) => rows.some((row) => row.device_id === id));

  return (
    <Page className={styles.page}>
      <Toolbar>
        <Tabs
          id="config"
          aria-label="Configuration"
          tabs={TABS}
          value={current}
          onChange={goTab}
        />
        <Button
          className={styles.primary}
          variant="primary"
          icon={Camera}
          loading={backup.starting || backup.running}
          disabled={backable.length === 0}
          onClick={() =>
            void backUp(chosen.length > 0 ? chosen : backable.map((row) => row.device_id))
          }
        >
          {chosen.length > 0
            ? `Back up ${plural(chosen.length, 'device')}…`
            : backable.length === 0
              ? 'Back up every device…'
              : `Back up all ${plural(backable.length, 'device')}…`}
        </Button>
      </Toolbar>

      <TabPanel tabs="config" value={current} className={styles.panel}>
        {current === 'backups' && (
          <BackupsTab
            coverage={coverage.data}
            loading={coverage.isPending}
            error={coverage.isError ? coverage.error : null}
            onRetry={() => void coverage.refetch()}
            unsaved={unsaved.data}
            selected={selected}
            onSelected={setSelected}
            onOpen={openDevice}
            onBackUp={(ids, kinds) => void backUp(ids, kinds)}
            job={backup.job}
            running={backup.running}
            onCancel={backup.cancel}
            onDismiss={backup.dismiss}
          />
        )}
        {current === 'history' && (
          <HistoryTab
            coverage={coverage.data}
            loading={coverage.isPending}
            deviceId={deviceId}
            onDevice={(id) => put({ device: id })}
            kind={kind}
            onKind={(next) => put({ kind: next })}
            onBackUp={(id) => void backUp([id])}
            backingUp={backup.starting || backup.running}
          />
        )}
        {current === 'search' && (
          <NetworkSearchTab
            query={query}
            onQuery={(next) => put({ q: next })}
            regex={regex}
            onRegex={(next) => put({ regex: next ? '1' : null })}
            caseSensitive={matchCase}
            onCaseSensitive={(next) => put({ case: next ? '1' : null })}
            kind={kind}
            onKind={(next) => put({ kind: next })}
            onOpen={openDevice}
            focusRef={searchFieldRef}
          />
        )}
        {current === 'compliance' && (
          <ComplianceTab onOpenDevice={openDevice} canRemediate={canHandOver} />
        )}
        {current === 'intended' && (
          <IntendedTab
            coverage={coverage.data}
            deviceId={deviceId}
            onDevice={(id) => put({ device: id })}
            canFix={canHandOver}
          />
        )}
      </TabPanel>
    </Page>
  );
}
