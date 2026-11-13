/**
 * The component showcase: every component of the design system in every
 * state, at `/#/dev/components`. Development and test builds only.
 *
 * It is grouped into tabs kept in the route (`/dev/components/:group`), so a
 * group can be linked to and screenshotted on its own.
 */

import type { ComponentType } from 'react';
import { Navigate, useNavigate, useParams } from 'react-router';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { TabPanel, Tabs } from '@/design/Tabs';
import { Actions } from './sections/Actions';
import { Charts } from './sections/Charts';
import { Code } from './sections/Code';
import { DataDisplay } from './sections/DataDisplay';
import { Feedback } from './sections/Feedback';
import { Forms } from './sections/Forms';
import { Foundations } from './sections/Foundations';
import { Layout } from './sections/Layout';
import { Overlays } from './sections/Overlays';
import { Patterns } from './sections/Patterns';
import { Status } from './sections/Status';
import styles from './Showcase.module.css';

const ROOT = '/dev/components';

const GROUPS: readonly { id: string; label: string; Content: ComponentType }[] = [
  { id: 'foundations', label: 'Foundations', Content: Foundations },
  { id: 'actions', label: 'Actions', Content: Actions },
  { id: 'forms', label: 'Forms', Content: Forms },
  { id: 'status', label: 'Status', Content: Status },
  { id: 'data', label: 'Data', Content: DataDisplay },
  { id: 'overlays', label: 'Overlays', Content: Overlays },
  { id: 'feedback', label: 'Feedback', Content: Feedback },
  { id: 'code', label: 'Code', Content: Code },
  { id: 'charts', label: 'Charts', Content: Charts },
  { id: 'layout', label: 'Layout', Content: Layout },
  { id: 'patterns', label: 'Patterns', Content: Patterns },
];

export default function ShowcasePage() {
  const { group = GROUPS[0]?.id } = useParams();
  const navigate = useNavigate();
  usePageHeader({ title: 'Components' });

  const current = GROUPS.find((entry) => entry.id === group);
  if (!current) return <Navigate to={ROOT} replace />;
  const { Content } = current;

  return (
    <Page>
      <div className={styles.tabs}>
        <Tabs
          id="showcase"
          aria-label="Component groups"
          tabs={GROUPS}
          value={current.id}
          onChange={(id) => void navigate(`${ROOT}/${id}`)}
        />
      </div>
      <TabPanel tabs="showcase" value={current.id} className={styles.content}>
        <Content />
      </TabPanel>
    </Page>
  );
}
