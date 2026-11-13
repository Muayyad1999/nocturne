import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { EmptyState } from '@/design/EmptyState';

const { label, icon } = destination('topology');

/** Route stub: the Topology slice replaces this body with the real screen. */
export default function TopologyPage() {
  usePageHeader({ title: label });
  return (
    <Page>
      <EmptyState
        icon={icon}
        title="This screen is not built yet"
        description="Topology will draw the network as discovered from LLDP, CDP and routing neighbours, with link state and utilisation on the map."
      />
    </Page>
  );
}
