/**
 * Settings — everything that is a preference rather than an operation.
 *
 * The route is `settings/:section?`. The list of sections comes from
 * `sections.ts` and is filtered by `app.methods`, so a build without a
 * feature module simply does not offer that section. Nothing here shows an
 * empty shell for a backend that is not present.
 */

import { SearchX } from 'lucide-react';
import { Navigate, NavLink, useParams } from 'react-router';
import { useRpc } from '@/api/hooks';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { Skeleton } from '@/design/Skeleton';
import { About } from './About';
import { Connections } from './Connections';
import { Credentials } from './Credentials';
import { DeviceTypes } from './DeviceTypes';
import { General } from './General';
import { Security } from './Security';
import { availableSections, type SettingsSection } from './sections';
import styles from './Settings.module.css';

/** The sections this slice implements. Others are hidden until someone does. */
const BODIES: Record<string, () => React.ReactElement> = {
  general: General,
  security: Security,
  credentials: Credentials,
  'device-types': DeviceTypes,
  connections: Connections,
  about: About,
};

export default function SettingsPage() {
  const { section: current } = useParams();
  const methods = useRpc<string[]>('app.methods');

  const sections = methods.data
    ? availableSections(new Set(methods.data)).filter((section) => section.id in BODIES)
    : [];
  const section = sections.find((entry) => entry.id === current);

  usePageHeader({
    title: section?.label ?? 'Settings',
    breadcrumbs: section ? [{ label: 'Settings', to: '/settings' }] : undefined,
  });

  if (methods.isError) {
    return (
      <Page>
        <ErrorState
          title="Settings could not be opened"
          error={methods.error}
          onRetry={() => void methods.refetch()}
        />
      </Page>
    );
  }

  if (methods.isPending) {
    return (
      <Page>
        <div className={styles.layout}>
          <div className={styles.nav} aria-busy="true">
            {[0, 1, 2, 3, 4, 5].map((row) => (
              <Skeleton key={row} height={28} />
            ))}
          </div>
          <Skeleton height={320} shape="block" />
        </div>
      </Page>
    );
  }

  // No section in the address: open the first one this build has.
  if (!current && sections[0]) return <Navigate to={`/settings/${sections[0].id}`} replace />;

  return (
    <Page>
      <div className={styles.layout}>
        <nav className={styles.nav} aria-label="Settings sections">
          {sections.map((entry) => (
            <NavLink
              key={entry.id}
              to={`/settings/${entry.id}`}
              className={styles.navItem}
              title={entry.description}
            >
              <Icon icon={entry.icon} size={14} className={styles.navIcon} />
              {entry.label}
            </NavLink>
          ))}
        </nav>

        <div className={styles.body} data-wide={section?.id === 'device-types' || undefined}>
          {section ? <SectionBody section={section} /> : <UnknownSection id={current} />}
        </div>
      </div>
    </Page>
  );
}

function SectionBody({ section }: { section: SettingsSection }) {
  const Body = BODIES[section.id];
  // The page header already says which section this is; here it is enough to
  // say what the section is for.
  return (
    <>
      <p className={styles.lead}>{section.description}</p>
      {Body ? <Body /> : null}
    </>
  );
}

function UnknownSection({ id }: { id: string | undefined }) {
  return (
    <EmptyState
      icon={SearchX}
      title="That settings section is not in this build"
      description={
        id
          ? `Nothing here is called “${id}”. The sections on the left are the ones this installation has a backend for.`
          : 'This installation has no settings sections, which should not happen.'
      }
    />
  );
}
