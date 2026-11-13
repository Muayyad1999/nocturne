/**
 * Monitoring — what is up, what is not, and what needs a human.
 *
 * The route is `monitoring/:tab?`; `?device=<id>` opens one device's page
 * instead of a tab. Everything on screen comes from `nocturne-monitor`
 * (`docs/dev/features/monitor.md`, types in `src/api/types/monitor.ts`) and
 * is kept up to date by its events, not by polling the backend.
 *
 * Monitoring is a feature crate, so this build may not have it: the screen
 * asks `app.methods` once and says so plainly rather than letting every call
 * fail.
 */

import { Activity, Bell, ChartLine, Network, SlidersHorizontal, Waves } from 'lucide-react';
import { useNavigate, useParams, useSearchParams } from 'react-router';
import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { useNavBadge } from '@/app/navBadges';
import { Page } from '@/app/Page';
import { useStatusItem } from '@/app/StatusBar';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { Skeleton } from '@/design/Skeleton';
import { TabPanel, Tabs, type TabItem } from '@/design/Tabs';
import { AREA as INSIGHTS, Insights } from '@/pages/insights';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { formatCount } from '@/lib/format';
import { AlertsTab } from './Alerts';
import { ChartsTab } from './Charts';
import { useAlertSummary, useDevices, useHasMethod, useMonitorStatus, usePollNow } from './data';
import { DeviceView } from './Device';
import { HealthTab } from './Health';
import { InterfacesTab } from './Interfaces';
import styles from './Monitoring.module.css';
import { RulesTab } from './Rules';
import { StatusStrip } from './StatusStrip';
import { deviceStatus, severityTone, worstOpenSeverity } from './vocab';

const { label: areaLabel, path: areaPath, icon: areaIcon } = destination('monitoring');

const TABS: readonly TabItem[] = [
  { id: 'alerts', label: 'Alerts', icon: Bell },
  { id: 'health', label: 'Health', icon: Activity },
  { id: 'interfaces', label: 'Interfaces', icon: Network },
  { id: 'charts', label: 'Charts', icon: ChartLine },
  { id: 'rules', label: 'Rules', icon: SlidersHorizontal },
  // `docs/dev/spec-ui-pages.md` heads this section "Monitoring — monitor.md,
  // logs.md, insights.md": the checks belong here rather than at a route of
  // their own, and the navigation rail has no Insights entry. The tab is
  // offered only when the crate is in the build; `Insights` itself also says
  // so if it is mounted without one, so neither place has to trust the other.
  { id: 'insights', label: INSIGHTS.label, icon: INSIGHTS.icon },
];

const DEFAULT_TAB = 'alerts';

export default function MonitoringPage() {
  const installed = useHasMethod('monitor.status');
  if (installed === undefined) return <Loading />;
  if (!installed) return <NotInstalled />;
  return <Monitoring />;
}

function Loading() {
  usePageHeader({ title: areaLabel });
  return (
    <Page>
      <Skeleton shape="block" height={96} />
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
        title="Monitoring is not in this build"
        description="This copy of Nocturne was assembled without the monitoring feature, so nothing is polled and no alerts are raised. The rest of the application is unaffected."
      />
    </Page>
  );
}

function Monitoring() {
  const { tab } = useParams();
  const [search, setSearch] = useSearchParams();
  const navigate = useNavigate();
  const current = TABS.some((entry) => entry.id === tab) ? (tab as string) : DEFAULT_TAB;
  const hasInsights = useHasMethod('insights.list');
  const deviceId = search.get('device');

  const status = useMonitorStatus();
  const devices = useDevices();
  const summary = useAlertSummary();
  const poll = usePollNow();

  const monitored = (devices.data ?? []).filter((device) => device.monitor.enabled);
  const down = monitored.filter((device) => deviceStatus(device) === 'down').length;
  const open = summary.data?.open ?? 0;
  const worst = worstOpenSeverity(summary.data?.by_severity);

  // While this screen is open the rail and the status bar carry its numbers.
  // They belong to the whole shell really, but only a mounted screen can set
  // them today; see NOTES.md.
  useNavBadge('monitoring', open > 0 ? { count: open, tone: worst ? severityTone(worst) : 'accent', label: 'open alerts' } : null);
  useStatusItem(
    status.data
      ? {
          id: 'monitor.polling',
          order: 20,
          label: status.data.enabled
            ? `${formatCount(status.data.devices_monitored)} monitored`
            : 'Monitoring off',
          tone: status.data.enabled ? (down > 0 ? 'crit' : 'ok') : 'idle',
          detail: status.data.enabled
            ? `${formatCount(status.data.polls_per_min)} polls a minute, ${formatCount(down)} down`
            : 'The scheduler is switched off in Settings → Monitoring.',
          onSelect: () => navigate(`${areaPath}/health`),
        }
      : null,
  );

  useStatusItem(
    summary.data
      ? {
          id: 'monitor.alerts',
          order: 30,
          label: open === 0 ? 'No open alerts' : `${formatCount(open)} open`,
          tone: open === 0 ? 'ok' : worst ? severityTone(worst) : 'warn',
          detail:
            open === 0
              ? 'Nothing is firing right now.'
              : `The worst of them is ${worst ?? 'open'}; the rail's badge counts the same alerts.`,
          onSelect: () => navigate(`${areaPath}/alerts`),
        }
      : null,
  );

  // One device's page is still this screen, so the header is set here rather
  // than in two places that would fight over it.
  const openDevice = deviceId ? devices.data?.find((device) => device.id === deviceId) : undefined;
  usePageHeader({
    title: deviceId ? (openDevice?.name ?? 'Device') : areaLabel,
    breadcrumbs: deviceId ? [{ label: areaLabel, to: `${areaPath}/${current}` }] : undefined,
  });

  if (deviceId) {
    return (
      <DeviceView
        key={deviceId}
        deviceId={deviceId}
        onBack={() => {
          const next = new URLSearchParams(search);
          next.delete('device');
          setSearch(next, { replace: true });
        }}
      />
    );
  }

  const tabs = TABS.filter((entry) => entry.id !== 'insights' || hasInsights !== false).map((entry) =>
    entry.id === 'alerts' && open > 0
      ? { ...entry, badge: <span className={styles.tabCount}>{formatCount(open)}</span> }
      : entry,
  );

  return (
    <Page>
      <StatusStrip status={status} devices={devices} summary={summary} />

      <Toolbar>
        <Tabs
          id="monitoring"
          aria-label="Monitoring"
          tabs={tabs}
          value={current}
          onChange={(next) => navigate(`${areaPath}/${next}`)}
        />
        <ToolbarSpacer />
        <Button
          variant="primary"
          icon={Waves}
          loading={poll.pending}
          disabled={monitored.length === 0}
          onClick={() =>
            void poll.run(
              monitored.map((device) => device.id),
              monitored.length === 1 ? (monitored[0]?.name ?? 'this device') : `${formatCount(monitored.length)} devices`,
            )
          }
        >
          Poll now…
        </Button>
      </Toolbar>

      <TabPanel tabs="monitoring" value={current} className={styles.panel}>
        {current === 'alerts' && <AlertsTab />}
        {current === 'health' && <HealthTab />}
        {current === 'interfaces' && <InterfacesTab />}
        {current === 'charts' && <ChartsTab />}
        {current === 'rules' && <RulesTab />}
        {current === 'insights' && <Insights />}
      </TabPanel>
    </Page>
  );
}
