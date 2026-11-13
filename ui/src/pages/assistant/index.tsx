import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { EmptyState } from '@/design/EmptyState';

const { label, icon } = destination('assistant');

/** Route stub: the Assistant slice replaces this body with the conversation history. */
export default function AssistantPage() {
  usePageHeader({ title: label });
  return (
    <Page>
      <EmptyState
        icon={icon}
        title="This screen is not built yet"
        description="Assistant history will keep past conversations, including what the assistant proposed and what was approved."
      />
    </Page>
  );
}
