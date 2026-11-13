import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { EmptyState } from '@/design/EmptyState';

const { label, icon } = destination('ipam');

/** Route stub: the IPAM slice replaces this body with the real screen. */
export default function IpamPage() {
  usePageHeader({ title: label });
  return (
    <Page>
      <EmptyState
        icon={icon}
        title="This screen is not built yet"
        description="IPAM will track prefixes, VLANs and address assignments, and show where the plan and the network disagree."
      />
    </Page>
  );
}
