import { useParams } from 'react-router';
import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { EmptyState } from '@/design/EmptyState';

const { label, icon, path } = destination('lab');

/** Route stub for the list and the `:id` detail route: the Lab slice replaces this body with the real screens. */
export default function LabPage() {
  const { id } = useParams();
  usePageHeader(id ? { title: id, breadcrumbs: [{ label, to: path }] } : { title: label });
  return (
    <Page>
      <EmptyState
        icon={icon}
        title="This screen is not built yet"
        description={
          id
            ? 'A lab page will show one simulated network: its topology, its devices and a console to each.'
            : 'Lab will let you build simulated networks, or a twin of the real one, and try changes there before they touch production.'
        }
      />
    </Page>
  );
}
