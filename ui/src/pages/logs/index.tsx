/**
 * Logs — what the network says about itself.
 *
 * Six panes over five engine pieces (`docs/dev/features/logs.md` and
 * `docs/dev/features/detect.md`; types in `src/api/types/logs.ts` and
 * `detect.ts`):
 *
 * ```
 * Lines       the store: search, histogram, list and live tail, every stream
 * Receivers   syslog, SNMP traps and flows: what is listening and what arrived
 * Rules       patterns that turn lines into signals, with thresholds and clearing
 * Forwarding  handing everything that is stored on to a log platform
 * Flows       the top talkers the flow collector has in memory
 * Detections  the catalogue of things two sources show and one does not
 * ```
 *
 * **Every pane, card and control exists only when its method is in
 * `app.methods`** (`panes.ts`). A build without `flows.*` has no Flows pane,
 * one without `detections.*` no Detections pane, one without
 * `logs.forward.*` no Forwarding — and inside a pane a button whose method
 * is missing is not rendered. Nothing here can answer `unknown_method`.
 *
 * Logs is the Monitoring screen's **Logs** tab, so the component that
 * matters is {@link Logs}: no props, no router of its own, no `Page` and no
 * page header. The pane is the `pane` search parameter, and the two panes
 * whose view is worth sharing keep it in parameters of their own (`q`, `sev`,
 * `stream`, `dev`, `span`, `from`/`to`, `tail` for the lines; `by`, `period`,
 * `scope` for the flows) — none of which Monitoring or Insights use, so a
 * shared link opens the same pane on the same search.
 *
 * The default export mounts the same component as a standalone screen with
 * its own `Page` and title, for a route if Logs ever gets one.
 */

import { ScrollText } from 'lucide-react';
import { useSearchParams } from 'react-router';
import { EmptyState } from '@/design/EmptyState';
import { Skeleton } from '@/design/Skeleton';
import { TabPanel, Tabs } from '@/design/Tabs';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { useStatusItem } from '@/app/StatusBar';
import { formatCount, plural } from '@/lib/format';
import { receiverVerdict } from './coverage';
import { useCapabilities, useLogsStatus } from './data';
import { DetectionsPane } from './DetectionsPane';
import { FlowsPane } from './FlowsPane';
import { ForwardingPane } from './ForwardingPane';
import { LinesPane } from './LinesPane';
import styles from './Logs.module.css';
import { paneFor, panesOf, type Capabilities } from './panes';
import { ReceiversPane } from './ReceiversPane';
import { RulesPane } from './RulesPane';

/**
 * What this part of the application is called, for the standalone page title
 * and the tab that mounts it. Logs is not a nav destination of its own.
 */
export const AREA = { label: 'Logs', icon: ScrollText } as const;

/**
 * The screen's body, for mounting inside another screen's tab panel. No props:
 * everything it needs is RPC, and what it is showing is in the URL.
 */
export function Logs() {
  const capabilities = useCapabilities();
  // `undefined` is "the method list has not answered yet". Waiting for proof
  // is right here: a screen that flashes "not in this build" and then draws
  // itself is worse than a skeleton for one round trip.
  if (capabilities === undefined) return <Loading />;
  if (panesOf(capabilities).length === 0) return <NotInstalled />;
  return <Panes capabilities={capabilities} />;
}

/** Logs as a screen of its own: the same component, with a page around it. */
export default function LogsPage() {
  usePageHeader({ title: AREA.label });
  return (
    <Page>
      <Logs />
    </Page>
  );
}

function Loading() {
  return (
    <div className={styles.screen}>
      <Skeleton shape="block" height={32} width={520} />
      <Skeleton shape="block" height={120} />
      <Skeleton shape="block" height={320} />
    </div>
  );
}

function NotInstalled() {
  return (
    <EmptyState
      icon={AREA.icon}
      title="Log collection is not in this build"
      description="This copy of Nocturne was assembled without the logs feature, so no syslog, trap or flow is received, stored or searched. The rest of the application is unaffected."
    />
  );
}

function Panes({ capabilities }: { capabilities: Capabilities }) {
  const [search, setSearch] = useSearchParams();
  const panes = panesOf(capabilities);
  const current = paneFor(search.get('pane'), capabilities);

  // One status for the whole tab: the receiver cards, the gaps in the
  // histogram, the forwarders' counters and the line in the status bar all
  // read the same answer and follow the same event.
  const status = useLogsStatus(capabilities.receivers);

  // The one line about logging that is on screen whatever pane is open.
  const verdict = status.data ? receiverVerdict(status.data) : null;
  useStatusItem(
    status.data && verdict
      ? {
          id: 'logs.receiver',
          order: 40,
          label:
            verdict.kind === 'receiving'
              ? `${formatCount(status.data.syslog.rate_per_minute)} lines/min`
              : verdict.kind === 'off'
                ? 'Syslog off'
                : verdict.kind === 'failed'
                  ? 'Syslog not listening'
                  : 'No syslog yet',
          tone:
            verdict.kind === 'receiving'
              ? 'ok'
              : verdict.kind === 'failed'
                ? 'crit'
                : verdict.kind === 'off'
                  ? 'idle'
                  : 'warn',
          detail: `${verdict.title}. ${plural(status.data.store.rows, 'line')} stored${
            status.data.syslog.dropped > 0
              ? `, ${formatCount(status.data.syslog.dropped)} dropped under load since the start`
              : ''
          }.`,
        }
      : null,
  );

  if (current === null) return <NotInstalled />;

  const open = (pane: string) => {
    const next = new URLSearchParams(search);
    // The first pane is the default and needs no parameter, so a plain link
    // to the tab stays a plain link.
    if (pane === panes[0]?.id) next.delete('pane');
    else next.set('pane', pane);
    setSearch(next, { replace: true });
  };

  const body = (
    <>
      {current === 'lines' && <LinesPane capabilities={capabilities} status={status} area={AREA} />}
      {current === 'receivers' && <ReceiversPane capabilities={capabilities} status={status} />}
      {current === 'rules' && <RulesPane capabilities={capabilities} status={status.data} />}
      {current === 'forwarding' && <ForwardingPane capabilities={capabilities} status={status.data} />}
      {current === 'flows' && <FlowsPane capabilities={capabilities} status={status.data} />}
      {current === 'detections' && <DetectionsPane capabilities={capabilities} />}
    </>
  );

  // A build with one pane has nothing to choose between, and a tab panel
  // with no tab to be labelled by would be a broken reference.
  if (panes.length === 1) return <div className={styles.screen}>{body}</div>;

  return (
    <div className={styles.screen}>
      <Tabs id="logs" aria-label="Logs" tabs={panes} value={current} onChange={open} />
      <TabPanel tabs="logs" value={current} className={styles.pane}>
        {body}
      </TabPanel>
    </div>
  );
}
