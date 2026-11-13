import { useParams } from 'react-router';
import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { EmptyState } from '@/design/EmptyState';

const { label, icon, path } = destination('changes');

/** Route stub for the list and the `:id` detail route: the Changes slice replaces this body with the real screens. */
export default function ChangesPage() {
  const { id } = useParams();
  usePageHeader(id ? { title: id, breadcrumbs: [{ label, to: path }] } : { title: label });
  return (
    <Page>
      <EmptyState
        icon={icon}
        title="This screen is not built yet"
        description={
          id
            ? 'A change page will show one change: what it does and to which devices, who approved it, and how it ran.'
            : 'Changes will take a planned change from draft through review and approval to execution, with verification and rollback.'
        }
      />
    </Page>
  );
}
