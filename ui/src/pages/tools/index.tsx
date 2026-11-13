import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { EmptyState } from '@/design/EmptyState';

const { label, icon } = destination('tools');

/** Route stub: the Tools slice replaces this body with the real screen. */
export default function ToolsPage() {
  usePageHeader({ title: label });
  return (
    <Page>
      <EmptyState
        icon={icon}
        title="This screen is not built yet"
        description="Tools will gather the everyday utilities in one place: ping, traceroute, DNS lookup, port check and a subnet calculator."
      />
    </Page>
  );
}
