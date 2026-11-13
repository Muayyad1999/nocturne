import { Compass } from 'lucide-react';
import { useLocation, useNavigate } from 'react-router';
import { usePalette } from '@/app/CommandPalette';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';

/** Shown for any address that is not a screen. */
export default function NotFoundPage() {
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const openPalette = usePalette((state) => state.open);
  usePageHeader({ title: 'Not found' });

  return (
    <Page>
      <EmptyState
        icon={Compass}
        title="There is no screen at this address"
        description={
          <>
            Nothing lives at <code>{pathname}</code>. The link may be out of date.
          </>
        }
        action={
          <>
            <Button variant="primary" onClick={() => void navigate('/')}>
              Go to Overview
            </Button>
            <Button variant="ghost" onClick={openPalette}>
              Search screens
            </Button>
          </>
        }
      />
    </Page>
  );
}
