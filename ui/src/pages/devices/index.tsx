import { useParams } from 'react-router';
import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { EmptyState } from '@/design/EmptyState';

const { label, icon, path } = destination('devices');

/** Route stub for the list and the `:id` detail route: the Devices slice replaces this body with the real screens. */
export default function DevicesPage() {
  const { id } = useParams();
  usePageHeader(id ? { title: id, breadcrumbs: [{ label, to: path }] } : { title: label });
  return (
    <Page>
      <EmptyState
        icon={icon}
        title="This screen is not built yet"
        description={
          id
            ? 'A device page will show one device: its interfaces, neighbours, config history and health.'
            : 'Devices will list every router, switch and firewall Nocturne manages, with live reachability and one-step access to a terminal, the config and metrics.'
        }
      />
    </Page>
  );
}
