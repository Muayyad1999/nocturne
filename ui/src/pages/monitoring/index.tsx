import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { EmptyState } from '@/design/EmptyState';

const { label, icon } = destination('monitoring');

/** Route stub: the Monitoring slice replaces this body with the real screen. */
export default function MonitoringPage() {
  usePageHeader({ title: label });
  return (
    <Page>
      <EmptyState
        icon={icon}
        title="This screen is not built yet"
        description="Monitoring will collect SNMP metrics, syslog and reachability, graph them, and raise alerts when thresholds are crossed."
      />
    </Page>
  );
}
