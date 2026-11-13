import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { EmptyState } from '@/design/EmptyState';

const { label, icon } = destination('terminal');

/** Route stub: the Terminal slice replaces this body with the real screen. */
export default function TerminalPage() {
  usePageHeader({ title: label });
  return (
    <Page>
      <EmptyState
        icon={icon}
        title="This screen is not built yet"
        description="Terminal will hold SSH, Telnet and serial sessions in tabs, with logging, search and commands sent to many devices at once."
      />
    </Page>
  );
}
