import { Navigate, useNavigate, useParams } from 'react-router';
import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { EmptyState } from '@/design/EmptyState';
import { TabPanel, Tabs } from '@/design/Tabs';

const { label, icon, path } = destination('ops');

/** The tab is part of the route (`/ops/:tab?`), so a tab can be linked to and survives a reload. */
const TABS = [
  {
    id: 'incidents',
    label: 'Incidents',
    description: 'Incidents will record what went wrong, who is working on it and how it was resolved.',
  },
  {
    id: 'handover',
    label: 'Shift handover',
    description: 'Shift handover will collect open issues and notes for the next shift.',
  },
  {
    id: 'reports',
    label: 'Reports',
    description: 'Reports will summarise availability, changes and incidents over a period you choose.',
  },
  {
    id: 'audit',
    label: 'Audit log',
    description: 'The audit log will list every action taken through Nocturne, by whom and on which device.',
  },
] as const;

/** Route stub: the Operations slice replaces the body of each tab with the real screen. */
export default function OperationsPage() {
  const { tab = TABS[0].id } = useParams();
  const navigate = useNavigate();
  usePageHeader({ title: label });

  const current = TABS.find((entry) => entry.id === tab);
  if (!current) return <Navigate to={path} replace />;

  return (
    <Page>
      <Tabs
        id="ops"
        aria-label={label}
        tabs={TABS}
        value={current.id}
        onChange={(id) => void navigate(id === TABS[0].id ? path : `${path}/${id}`)}
      />
      <TabPanel tabs="ops" value={current.id}>
        <EmptyState icon={icon} title="This screen is not built yet" description={current.description} />
      </TabPanel>
    </Page>
  );
}
