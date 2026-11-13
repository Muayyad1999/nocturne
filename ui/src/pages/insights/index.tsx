/**
 * Insights — what is wrong with the network, with the evidence, and what was
 * not looked at.
 *
 * Everything here comes from `nocturne-insights` (`docs/dev/features/insights.md`,
 * types in `src/api/types/insights.ts`): `insights.list` for the findings,
 * `insights.coverage` for what the last scan could and could not decide,
 * `insights.checks` for the catalogue, and `insights.scan` for a new scan,
 * which runs as a job. The list is kept up to date by `insights.changed`.
 *
 * Insights is the Monitoring screen's **Insights** tab (`spec-ui-pages.md`
 * heads that section `monitor.md`, `logs.md`, `insights.md`), so the component
 * that matters here is {@link Insights}: a plain component with no props, no
 * router of its own, no `Page` and no page header, which renders inside
 * another screen's tab panel. It keeps its own sub-view in the `view` search
 * parameter, which is free of anything Monitoring uses.
 *
 * The default export mounts the same component as a standalone screen (its own
 * `Page` and page title). That is what the Playwright specs drive, and what a
 * route would render if Insights ever gets one.
 *
 * It is built around one rule from CLAUDE.md: "unknown", "clean" and "not
 * evaluated" are three different things. Coverage is therefore not a tab you
 * can forget to open — it is a band above the findings, on every view, and the
 * age of the scan is in the status bar as well.
 *
 * Insights is a feature crate, so a build may not have it: the component asks
 * `app.methods` once and says so plainly rather than letting every call fail.
 */

import { ListChecks, Radar, ScanSearch, TriangleAlert } from 'lucide-react';
import { useEffect, useEffectEvent } from 'react';
import { useSearchParams } from 'react-router';
import type { InsightsListResult } from '@/api/types/insights';
import { registerCommand } from '@/app/CommandPalette/commands';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { useStatusItem } from '@/app/StatusBar';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { Skeleton } from '@/design/Skeleton';
import { TabPanel, Tabs, type TabItem } from '@/design/Tabs';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { formatCount, plural } from '@/lib/format';
import { formatRelative } from '@/lib/time';
import { useHasMethod, useRpc } from '@/api/hooks';
import { ChecksTab } from './Checks';
import { CoverageBand } from './CoverageBand';
import { CoverageTab } from './Coverage';
import { useCoverage } from './data';
import { FindingsTab } from './Findings';
import { freshness, freshnessTone, severityTone, worstSeverity } from './insight';
import styles from './Insights.module.css';
import { ScanBanner } from './ScanBanner';
import { useScan } from './useScan';

/**
 * What this part of the application is called, for the standalone page title
 * and the empty states. It is not a nav destination (`destinations.ts` has no
 * `insights` entry and should not need one), so the label lives here.
 */
export const AREA = { label: 'Insights', icon: ScanSearch } as const;

const TABS: readonly TabItem[] = [
  { id: 'findings', label: 'Findings', icon: TriangleAlert },
  { id: 'coverage', label: 'Coverage', icon: ScanSearch },
  { id: 'checks', label: 'Checks', icon: ListChecks },
];

const DEFAULT_TAB = 'findings';

/**
 * The screen's body, for mounting inside another screen's tab panel. No props:
 * everything it needs is RPC, and the view it is on is in the URL's `view`
 * search parameter.
 */
export function Insights() {
  const installed = useHasMethod('insights.list');
  if (installed === undefined) return <Loading />;
  if (!installed) return <NotInstalled />;
  return <InsightsBody />;
}

/** Insights as a screen of its own: the same component, with a page around it. */
export default function InsightsPage() {
  usePageHeader({ title: AREA.label });
  return (
    <Page>
      <Insights />
    </Page>
  );
}

function Loading() {
  return (
    <div className={styles.loading}>
      <Skeleton shape="block" height={32} width={420} />
      <Skeleton shape="block" height={96} />
      <Skeleton shape="block" height={320} />
    </div>
  );
}

function NotInstalled() {
  return (
    <EmptyState
      icon={AREA.icon}
      title="Issue identification is not in this build"
      description="This copy of Nocturne was assembled without the insights feature, so nothing is examined and no findings are kept. The rest of the application is unaffected."
    />
  );
}

function InsightsBody() {
  const [search, setSearch] = useSearchParams();
  const requested = search.get('view');
  const tab = TABS.some((entry) => entry.id === requested) ? (requested as string) : DEFAULT_TAB;

  const scan = useScan();
  const coverage = useCoverage();

  const select = useEffectEvent((next: string) => {
    const params = new URLSearchParams(search);
    if (next === DEFAULT_TAB) params.delete('view');
    else params.set('view', next);
    setSearch(params, { replace: true });
  });
  const startScan = useEffectEvent(() => scan.start());

  // The counts of everything that is open, whatever the list below is
  // filtered to: the badge and the tab count must not move when a filter does.
  const open = useRpc<InsightsListResult>(
    'insights.list',
    { state: 'open', limit: 1 },
    { invalidateOn: ['insights.changed'] },
  );
  const counts = open.data?.counts.by_severity;
  const openTotal = counts
    ? Object.values(counts).reduce((sum, value) => sum + value, 0)
    : undefined;
  const worst = worstSeverity(counts);

  // The age of the last scan, in the one place that is on screen whatever
  // the user is looking at. A finding is only as true as its scan.
  const scannedAt = coverage.data?.scanned_at;
  useStatusItem(
    coverage.isPending
      ? null
      : {
          id: 'insights.scan',
          order: 30,
          label:
            scannedAt === undefined
              ? 'Never scanned'
              : `Scanned ${formatRelative(scannedAt)}`,
          tone: scannedAt === undefined ? 'idle' : freshnessTone(freshness(scannedAt)),
          detail:
            scannedAt === undefined
              ? 'No scan has run yet, so there are no findings and no coverage.'
              : `The last scan covered ${plural(coverage.data?.devices ?? 0, 'device')} with ${formatCount(coverage.data?.checks.length)} checks. Findings are only as current as this.`,
          onSelect: () => select('coverage'),
        },
  );

  useEffect(
    () =>
      registerCommand({
        id: 'insights.scan',
        title: 'Scan the network for issues',
        group: 'Insights',
        keywords: ['insight', 'findings', 'issues', 'check', 'duplex', 'scan'],
        icon: Radar,
        run: () => startScan(),
      }),
    [],
  );
  useEffect(
    () =>
      registerCommand({
        id: 'insights.coverage',
        title: 'Show what the last scan could not examine',
        group: 'Insights',
        keywords: ['coverage', 'skipped', 'not evaluated', 'blind spots'],
        icon: ScanSearch,
        run: () => select('coverage'),
      }),
    [],
  );

  const tabs = TABS.map((entry) =>
    entry.id === 'findings' && openTotal !== undefined && openTotal > 0
      ? {
          ...entry,
          badge: (
            <Badge tone={worst ? severityTone(worst) : 'neutral'} variant="outline">
              {formatCount(openTotal)}
            </Badge>
          ),
        }
      : entry,
  );

  return (
    <div className={styles.screen}>
      <Toolbar>
        <Tabs id="insights" tabs={tabs} value={tab} onChange={select} aria-label="Insights views" />
        <ToolbarSpacer />
        {/* One primary action, with a stable name: what it is doing is said by
            the banner below, which also carries the Cancel. */}
        <Button
          variant="primary"
          icon={Radar}
          loading={scan.running}
          onClick={() => scan.start()}
          title="Reads what has already been collected from every device. It does not connect to anything."
        >
          Scan now
        </Button>
      </Toolbar>

      <ScanBanner scan={scan} />

      {/* Coverage sits above every view, so the findings cannot be read
          without what was not examined. */}
      <CoverageBand onOpen={() => select('coverage')} scan={scan} />

      <TabPanel tabs="insights" value={tab} className={styles.panel}>
        {tab === 'findings' && <FindingsTab scan={scan} />}
        {tab === 'coverage' && <CoverageTab scan={scan} />}
        {tab === 'checks' && <ChecksTab scan={scan} />}
      </TabPanel>
    </div>
  );
}
