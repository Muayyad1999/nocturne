/**
 * What the header shows for the current page. A page calls `usePageHeader`
 * once, at the top of its route component.
 */

import { useEffect, type ReactNode } from 'react';
import { create } from 'zustand';

export interface Breadcrumb {
  label: string;
  /** Route to go to. The last crumb, the page itself, has none. */
  to?: string;
}

export interface PageHeader {
  /** The page's name: the header's heading and the window title. */
  title: string;
  /** Ancestors of the page, outermost first: `[{ label: 'Devices', to: '/devices' }]`. */
  breadcrumbs?: Breadcrumb[];
  /** Page-level actions shown at the right of the header. */
  actions?: ReactNode;
}

interface PageHeaderState {
  header: PageHeader | null;
}

export const usePageHeaderStore = create<PageHeaderState>(() => ({ header: null }));

/** Sets the header's title, breadcrumbs and actions while the page is mounted. */
export function usePageHeader({ title, breadcrumbs, actions }: PageHeader): void {
  // Breadcrumbs are usually written inline; compare them by content so a new
  // array on every render does not count as a change.
  const crumbs = JSON.stringify(breadcrumbs ?? []);

  useEffect(() => {
    usePageHeaderStore.setState({
      header: { title, breadcrumbs: JSON.parse(crumbs) as Breadcrumb[], actions },
    });
  }, [title, crumbs, actions]);

  useEffect(() => {
    document.title = `${title} · Nocturne`;
  }, [title]);

  useEffect(
    () => () => {
      usePageHeaderStore.setState({ header: null });
      document.title = 'Nocturne';
    },
    [],
  );
}
