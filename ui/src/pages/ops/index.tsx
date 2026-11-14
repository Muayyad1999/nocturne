/**
 * Operations — the record of what happened, and the screen a NOC engineer
 * lives in during and after an incident.
 *
 * Six tabs on one route (`/ops/:tab?`), each a view over records that already
 * exist: **Incidents** (with the timeline that is the centre of this screen),
 * **Handover**, **Reports**, **Notes**, **Audit** and **Jobs**. Everything
 * comes from `nocturne-ops` (`docs/dev/features/ops.md`, types in
 * `api/types/ops.ts`) except the job history, which is the core's `jobs.*`.
 *
 * Nothing on this screen writes to a device; there is no method behind it that
 * could. What it does do is make claims about the past, so the rules it keeps
 * are about honesty rather than safety:
 *
 * - a section that could not be read says so, with the backend's own reason,
 *   and never as a zero (`ops.ts`, `Unavailable.tsx`);
 * - every line of a generated summary links to the record it came from;
 * - a tab whose feature is not in this build is not shown at all, and each
 *   tab degrades on its own — a build without alerting still has incidents.
 *
 * The incident page lives at `?incident=<id>` rather than a route of its own,
 * because the route table (`app/routes.tsx`) gives this screen one optional
 * segment and it is spent on the tab. It is still a link one can send.
 */

import {
  ClipboardList,
  FileText,
  ListTree,
  Notebook,
  ScrollText,
  Siren,
} from 'lucide-react';
import { useEffect } from 'react';
import { Navigate, useNavigate, useParams, useSearchParams } from 'react-router';
import { useHasMethod } from '@/api/hooks';
import { registerCommand } from '@/app/CommandPalette/commands';
import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { useNavBadge } from '@/app/navBadges';
import { Page } from '@/app/Page';
import { useStatusItem } from '@/app/StatusBar';
import { Badge } from '@/design/Badge';
import { EmptyState } from '@/design/EmptyState';
import { Skeleton } from '@/design/Skeleton';
import { TabPanel, Tabs, type TabItem } from '@/design/Tabs';
import { formatCount } from '@/lib/format';
import { AuditTab } from './Audit';
import { useIncident, useIncidents } from './data';
import { HandoverTab } from './Handover';
import { IncidentList } from './IncidentList';
import { IncidentPage } from './IncidentPage';
import { JobsTab } from './Jobs';
import { NotesTab } from './Notes';
import styles from './Ops.module.css';
import { ReportsTab } from './Reports';

const { label: areaLabel, path: areaPath, icon: areaIcon } = destination('ops');

/** A tab, and the one method without which it has nothing to show. */
interface OpsTab extends TabItem {
  needs: string;
  /** What it would have been, for a build that does not have it. */
  about: string;
}

const TABS: readonly OpsTab[] = [
  {
    id: 'incidents',
    label: 'Incidents',
    icon: Siren,
    needs: 'incidents.list',
    about: 'Incidents, with a timeline that gathers itself from the alerts, changes and configuration movement around them.',
  },
  {
    id: 'handover',
    label: 'Handover',
    icon: ClipboardList,
    needs: 'handover.generate',
    about: 'The summary of a shift, generated from what actually happened, with every item linked to its record.',
  },
  {
    id: 'reports',
    label: 'Reports',
    icon: FileText,
    needs: 'reports.kinds',
    about: 'Inventory, software, availability, changes, backups, compliance and alerts, over a period you choose.',
  },
  {
    id: 'notes',
    label: 'Notes',
    icon: Notebook,
    needs: 'notes.list',
    about: 'Runbooks and what the last engineer learned at 3am, attached to devices, sites and tags.',
  },
  {
    id: 'audit',
    label: 'Audit',
    icon: ScrollText,
    needs: 'audit.search',
    about: 'Every mutation anybody made through Nocturne, filterable, and linked to what it touched.',
  },
  {
    id: 'jobs',
    label: 'Jobs',
    icon: ListTree,
    needs: 'jobs.list',
    about: 'The full history of the work the engine has done: backups, collections, scans, applies.',
  },
];

const DEFAULT_TAB = 'incidents';

export default function OperationsPage() {
  // The whole feature is one crate: without it there is no incident, no
  // handover, no report and no note, and the audit trail has no view.
  const installed = useHasMethod('incidents.list');
  if (installed === undefined) return <Loading />;
  if (!installed) return <NotInstalled />;
  return <Operations />;
}

function Loading() {
  usePageHeader({ title: areaLabel });
  return (
    <Page>
      <Skeleton shape="block" height={32} width={520} />
      <Skeleton shape="block" height={56} />
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
        title="The operations record is not in this build"
        description="This copy of Nocturne was assembled without the operations feature, so it keeps no incidents, handovers, reports or notes. Everything else is unaffected."
      />
    </Page>
  );
}

function Operations() {
  const { tab } = useParams();
  const [search, setSearch] = useSearchParams();
  const navigate = useNavigate();

  // Each tab is asked for separately: a build can have incidents without a
  // configuration feature, or jobs without reports.
  const present: Record<string, boolean | undefined> = {
    incidents: useHasMethod('incidents.list'),
    handover: useHasMethod('handover.generate'),
    reports: useHasMethod('reports.kinds'),
    notes: useHasMethod('notes.list'),
    audit: useHasMethod('audit.search'),
    jobs: useHasMethod('jobs.list'),
  };

  const known = TABS.find((entry) => entry.id === tab);
  const current = known?.id ?? DEFAULT_TAB;
  const incidentId = search.get('incident');

  // The unresolved incidents, for the rail badge, the status bar and the tab
  // count. One read, shared with the list below it by its query key.
  const unresolved = useIncidents({ state: 'unresolved', limit: 200 });
  const openCount = unresolved.data?.counts.by_state.open ?? 0;
  const unresolvedCount =
    (unresolved.data?.counts.by_state.open ?? 0) + (unresolved.data?.counts.by_state.mitigated ?? 0);
  const worst = unresolved.data?.incidents.reduce<string | null>(
    (found, incident) => found ?? (incident.severity === 'sev1' || incident.severity === 'sev2' ? incident.severity : null),
    null,
  );

  useNavBadge(
    'ops',
    unresolvedCount > 0
      ? { count: unresolvedCount, tone: worst ? 'crit' : 'warn', label: 'unresolved incidents' }
      : null,
  );

  useStatusItem(
    unresolved.data
      ? {
          id: 'ops.incidents',
          order: 40,
          label: unresolvedCount === 0 ? 'No open incident' : `${formatCount(unresolvedCount)} unresolved`,
          tone: unresolvedCount === 0 ? 'ok' : worst ? 'crit' : 'warn',
          detail:
            unresolvedCount === 0
              ? 'Nothing is being worked on right now.'
              : `${formatCount(openCount)} open, ${formatCount(unresolvedCount - openCount)} mitigated but not resolved.`,
          onSelect: () => navigate(`${areaPath}/incidents`),
        }
      : null,
  );

  useEffect(() => {
    const stops = [
      registerCommand({
        id: 'ops.incidents',
        title: 'Open incidents',
        group: areaLabel,
        keywords: ['incident', 'outage', 'timeline', 'postmortem'],
        icon: Siren,
        run: () => navigate(`${areaPath}/incidents`),
      }),
      registerCommand({
        id: 'ops.handover',
        title: 'Write the shift handover',
        group: areaLabel,
        keywords: ['handover', 'shift', 'summary', 'hand over'],
        icon: ClipboardList,
        run: () => navigate(`${areaPath}/handover`),
      }),
      registerCommand({
        id: 'ops.reports',
        title: 'Generate a report',
        group: areaLabel,
        keywords: ['report', 'availability', 'inventory', 'compliance'],
        icon: FileText,
        run: () => navigate(`${areaPath}/reports`),
      }),
      registerCommand({
        id: 'ops.audit',
        title: 'Search the audit trail',
        group: areaLabel,
        keywords: ['audit', 'who did', 'history', 'trail'],
        icon: ScrollText,
        run: () => navigate(`${areaPath}/audit`),
      }),
    ];
    return () => {
      for (const stop of stops) stop();
    };
  }, []);

  const openIncident = (id: string | null) => {
    const next = new URLSearchParams(search);
    if (id) next.set('incident', id);
    else next.delete('incident');
    setSearch(next, { replace: id === null });
  };

  // The header belongs to the screen, not to each tab, so one place decides
  // what it says — including on the incident page, which is still this screen.
  // The incident is read under the same query key the page itself uses, so
  // naming it in the header costs nothing.
  const open = useIncident(current === 'incidents' ? incidentId : null);
  usePageHeader({
    title: incidentId && current === 'incidents' ? (open.data?.title ?? 'Incident') : areaLabel,
    breadcrumbs:
      incidentId && current === 'incidents'
        ? [{ label: areaLabel, to: `${areaPath}/incidents` }]
        : undefined,
  });

  if (tab !== undefined && !known) return <Navigate to={areaPath} replace />;

  // A tab whose feature is absent is not offered. While `app.methods` is in
  // flight nothing is hidden, so the bar does not shuffle as it arrives.
  const tabs: TabItem[] = TABS.filter((entry) => present[entry.id] !== false).map((entry) =>
    entry.id === 'incidents' && unresolvedCount > 0
      ? {
          ...entry,
          badge: (
            <Badge tone={worst ? 'crit' : 'warn'} variant="outline">
              {formatCount(unresolvedCount)}
            </Badge>
          ),
        }
      : entry,
  );

  return (
    <Page>
      <Tabs
        id="ops"
        aria-label={areaLabel}
        tabs={tabs}
        value={current}
        className={styles.tabs}
        onChange={(id) => void navigate(id === DEFAULT_TAB ? areaPath : `${areaPath}/${id}`)}
      />
      <TabPanel tabs="ops" value={current} className={styles.panel}>
        <Body tab={current} incidentId={incidentId} onOpenIncident={openIncident} present={present} />
      </TabPanel>
    </Page>
  );
}

interface BodyProps {
  tab: string;
  incidentId: string | null;
  onOpenIncident: (id: string | null) => void;
  present: Record<string, boolean | undefined>;
}

function Body({ tab, incidentId, onOpenIncident, present }: BodyProps) {
  // A tab reached by its link in a build that does not have it: the tab bar
  // left it out, but the address could still name it.
  if (present[tab] === false) {
    const entry = TABS.find((candidate) => candidate.id === tab);
    return (
      <EmptyState
        size="compact"
        icon={entry?.icon ?? areaIcon}
        title={`${entry?.label ?? 'That tab'} is not in this build`}
        description={entry?.about}
      />
    );
  }

  switch (tab) {
    case 'incidents':
      return incidentId ? (
        <IncidentPage key={incidentId} id={incidentId} onClose={() => onOpenIncident(null)} />
      ) : (
        <IncidentList onOpen={(id) => onOpenIncident(id)} />
      );
    case 'handover':
      return <HandoverTab />;
    case 'reports':
      return <ReportsTab />;
    case 'notes':
      return <NotesTab />;
    case 'audit':
      return <AuditTab />;
    case 'jobs':
      return <JobsTab />;
    default:
      return null;
  }
}
