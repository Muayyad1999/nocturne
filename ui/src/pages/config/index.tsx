import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { EmptyState } from '@/design/EmptyState';

const { label, icon } = destination('config');

/** Route stub: the Config slice replaces this body with the real screen. */
export default function ConfigPage() {
  usePageHeader({ title: label });
  return (
    <Page>
      <EmptyState
        icon={icon}
        title="This screen is not built yet"
        description="Config will keep versioned backups of every device, show what changed between versions and check them against your standards."
      />
    </Page>
  );
}
