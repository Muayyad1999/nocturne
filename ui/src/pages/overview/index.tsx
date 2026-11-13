/**
 * Overview — the screen a shift starts with, and the one left open on a wall.
 *
 * It answers six questions and nothing else: what is down, what is firing,
 * what changed, what is drifting, what is running, and what needs a person.
 * Everything comes from one call, `overview.get` (`nocturne-ops`), which
 * assembles each part from the feature that owns it and reports the ones it
 * could not fill instead of guessing — see `api/types/ops.ts`, whose first
 * paragraph is the contract this screen is built against.
 *
 * The three rules it enforces, everywhere:
 *
 * 1. **`null` is not `0`.** A part the backend could not fill shows the reason
 *    ("Not in this build", "Nothing recorded yet", "Could not be read") and no
 *    figure at all; a part that is present and zero shows the zero and says
 *    what it means ("Nothing firing"). The two are different shapes in
 *    `overview.ts`, so one cannot be rendered as the other by accident.
 * 2. **A capped count says so.** `truncated: true` puts "at least" in front of
 *    the number: a floor is never shown as a total.
 * 3. **The age of the reading is on screen.** A stale overview is the only way
 *    this screen can really mislead, so `generated_at` is in the toolbar as a
 *    live relative time, in the status bar, and — once it is older than
 *    `STALE_AFTER_MS` — as a banner saying the picture may have stopped moving.
 *
 * It updates from events rather than polling (`data.ts` lists the topics); the
 * one slow refetch is for the things that change with the clock and have no
 * event, and that file says which.
 */

import { RotateCw } from 'lucide-react';
import { useEffect, useEffectEvent, useState } from 'react';
import { useNavigate } from 'react-router';
import { useRpc } from '@/api/hooks';
import { registerCommand } from '@/app/CommandPalette/commands';
import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { useStatusItem } from '@/app/StatusBar';
import { Button } from '@/design/Button';
import { ErrorState } from '@/design/ErrorState';
import { TimeAgo } from '@/design/TimeAgo';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { formatRelative } from '@/lib/time';
import { useNow } from '@/lib/useNow';
import { Activity, ActivitySkeleton } from './Activity';
import { useAcknowledge, useCancelJob, useDemoLab, useDevices, useOverview } from './data';
import { FirstRun } from './FirstRun';
import { NeedsAttention, NeedsAttentionSkeleton } from './NeedsAttention';
import { freshness, needsAttention, readPart, sitesLine, tiles } from './overview';
import styles from './Overview.module.css';
import { Running } from './Running';
import { Tiles, TilesSkeleton } from './Tiles';

const { label } = destination('overview');

export default function OverviewPage() {
  const overview = useOverview();
  const devices = useDevices();
  const acknowledge = useAcknowledge();
  const cancel = useCancelJob();
  const demo = useDemoLab();
  const navigate = useNavigate();
  const [refreshedByHand, setRefreshedByHand] = useState(false);

  // `labs.*` is a feature crate: a build without it cannot offer the demo.
  const methods = useRpc<string[]>('app.methods');
  const canSimulate = methods.data?.includes('labs.create') ?? false;

  const refresh = useEffectEvent(() => {
    setRefreshedByHand(true);
    overview.refetch();
  });

  usePageHeader({
    title: label,
    actions: (
      <Button
        icon={RotateCw}
        variant="secondary"
        loading={overview.isFetching}
        onClick={() => refresh()}
      >
        Refresh
      </Button>
    ),
  });

  useEffect(
    () =>
      registerCommand({
        id: 'overview.refresh',
        title: 'Refresh the overview',
        group: 'Overview',
        keywords: ['reload', 'read again', 'now'],
        icon: RotateCw,
        run: () => {
          navigate('/');
          refresh();
        },
      }),
    [],
  );

  // The age of the reading, in the status bar as well as the toolbar: a
  // wallboard is read from across the room. `useNow` is only the heartbeat
  // that re-renders; the age itself is measured at render, because a tick
  // that is up to ten seconds old would make a reading taken a moment ago
  // read as "in 7s" — the future.
  useNow(10_000);
  const now = Date.now();
  const generatedAt = overview.data?.generated_at;
  const stale = generatedAt !== undefined && freshness(generatedAt, now) === 'stale';
  useStatusItem(
    generatedAt === undefined
      ? null
      : {
          id: 'overview.read',
          order: 10,
          label: `Overview read ${formatRelative(generatedAt, now)}`,
          tone: stale ? 'warn' : 'ok',
          detail: stale
            ? 'Nothing has refreshed this screen for a while. The engine, the event stream or this browser tab may have stopped.'
            : 'When the engine assembled this picture — not when it was drawn.',
          onSelect: () => refresh(),
        },
  );

  if (overview.error) {
    return (
      <Page>
        <ErrorState
          title="The overview could not be read"
          error={overview.error}
          onRetry={() => refresh()}
        />
      </Page>
    );
  }

  if (overview.isPending || !overview.data) {
    return (
      <Page>
        <div className={styles.screen}>
          <Toolbar>
            <span className={styles.read}>Reading the state of the network…</span>
          </Toolbar>
          <TilesSkeleton />
          <div className={styles.columns}>
            <NeedsAttentionSkeleton />
            <div className={styles.aside}>
              <ActivitySkeleton />
            </div>
          </div>
        </div>
      </Page>
    );
  }

  const data = overview.data;

  // An empty inventory is not a quiet network: there is nothing to be quiet.
  if (data.devices.total === 0) {
    return (
      <Page>
        <div className={styles.screen}>
          <FirstRun
            canSimulate={canSimulate}
            starting={demo.isPending}
            onExploreDemo={() => demo.mutate()}
          />
        </div>
      </Page>
    );
  }

  const rows = needsAttention({ overview: data, devices: devices.data });
  const sites = sitesLine(data.devices);
  // The inventory is needed only for the *names* of the dead devices; losing
  // it is not an error on this screen, but the list has to admit it.
  const namesMissing = devices.data === undefined && data.devices.down > 0;

  return (
    <Page>
      <div className={styles.screen}>
        <Toolbar>
          {sites !== null && <span className={styles.sites}>{sites}</span>}
          <ToolbarSpacer />
          <span className={styles.read} data-tone={stale ? 'warn' : undefined}>
            Read <TimeAgo at={data.generated_at} />
            {refreshedByHand && <span className={styles.by}>· asked by hand</span>}
          </span>
        </Toolbar>

        {stale && (
          <p className={styles.stale} role="status">
            This picture was assembled <TimeAgo at={data.generated_at} /> and has not been read
            again since. Nothing below is necessarily true now.
          </p>
        )}

        <Tiles tiles={tiles(data)} />

        <div className={styles.columns}>
          <NeedsAttention
            rows={rows}
            unavailable={data.unavailable}
            generatedAt={data.generated_at}
            namesMissing={namesMissing}
            acknowledging={acknowledge.isPending ? (acknowledge.variables?.id ?? null) : null}
            onAcknowledge={(id) => acknowledge.mutate({ id })}
          />
          <div className={styles.aside}>
            <Running
              jobs={readPart(data, 'jobs')}
              cancelling={cancel.isPending ? (cancel.variables?.id ?? null) : null}
              onCancel={(id) => cancel.mutate({ id })}
            />
            <Activity lines={data.activity} />
          </div>
        </div>
      </div>
    </Page>
  );
}
