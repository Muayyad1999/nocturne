import { Skeleton } from '@/design/Skeleton';
import { Page } from './Page';

/** Shown while a screen's code is loading: the outline of a toolbar and content. */
export function PageFallback() {
  return (
    <Page aria-busy="true" aria-label="Loading screen">
      <Skeleton width={220} height={28} />
      <Skeleton shape="block" height={160} />
      <Skeleton width="60%" />
      <Skeleton width="45%" />
    </Page>
  );
}
