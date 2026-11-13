import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { EmptyState } from '@/design/EmptyState';

const { label, icon } = destination('settings');

/** Route stub: the Settings slice replaces this body with the real screen. */
export default function SettingsPage() {
  usePageHeader({ title: label });
  return (
    <Page>
      <EmptyState
        icon={icon}
        title="This screen is not built yet"
        description="Settings will hold credentials, connection defaults, notification rules and appearance."
      />
    </Page>
  );
}
