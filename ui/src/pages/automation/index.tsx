import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { EmptyState } from '@/design/EmptyState';

const { label, icon } = destination('automation');

/** Route stub: the Automation slice replaces this body with the real screen. */
export default function AutomationPage() {
  usePageHeader({ title: label });
  return (
    <Page>
      <EmptyState
        icon={icon}
        title="This screen is not built yet"
        description="Automation will run jobs and playbooks across devices, on demand or on a schedule, and keep the result of every run."
      />
    </Page>
  );
}
