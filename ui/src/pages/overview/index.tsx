import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { EmptyState } from '@/design/EmptyState';

const { label, icon } = destination('overview');

/** Route stub: the Overview slice replaces this body with the real screen. */
export default function OverviewPage() {
  usePageHeader({ title: label });
  return (
    <Page>
      <EmptyState
        icon={icon}
        title="This screen is not built yet"
        description="Overview will open with the state of the network at a glance: what is down, what changed and what needs attention."
      />
    </Page>
  );
}
