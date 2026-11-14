/**
 * Tools — the drawer a network engineer opens twenty times a day.
 *
 * Three kinds of thing live here, and the screen's one job is to keep them
 * apart, because they have nothing in common but the drawer:
 *
 * 1. **Calculators and decoders.** Arithmetic on this machine: subnets, VLSM,
 *    masks and the ACL lines they make, MAC and OUI, OIDs, interface names,
 *    converters, MTU and MSS, regex, diff, digests, Base64, the reference
 *    tables. They answer as you type and nothing leaves the computer.
 * 2. **Probes from here.** Ping, traceroute, port check, sweep, DNS, WHOIS,
 *    TLS, HTTP, NTP, this machine's own network, Wake-on-LAN. These put
 *    packets on the wire from the machine the *engine* runs on — which in
 *    server mode is the NOC server, not the browser — and every one of them is
 *    recorded in the probe history.
 * 3. **Things that outlive the screen.** A probe run *from a device* through
 *    the read-only gate, and the two servers (TFTP, a staging syslog
 *    listener), whose state lives in the backend: this screen re-attaches to
 *    what it finds rather than starting it again.
 *
 * Everything on screen is the engine's own catalogue (`tools.list`) and the
 * engine's own answers. The two things the screen is most careful about are
 * the two the backend was written carefully for: a secret's `reversible` and
 * `decodable` are different questions and are shown as two statements
 * (`secrets.ts`), and a probe that could not run is a refusal with its code,
 * never an empty chart (`probes.ts`, `Unavailable.tsx`).
 *
 * The toolbox is a feature crate, so this build may not have it: the screen
 * asks `app.methods` once and says so in a sentence instead of showing dead
 * controls.
 */

import { Calculator, Radio, Wrench } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { useNavigate, useParams } from 'react-router';
import { useHasMethod } from '@/api/hooks';
import { registerCommand } from '@/app/CommandPalette';
import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { useStatusItem } from '@/app/StatusBar';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Skeleton } from '@/design/Skeleton';
import { groupPanels, panels, selectPanel, type Panel } from './catalogue';
import { useCatalogue, useDevices, useHistory } from './data';
import { SPECS } from './specs';
import { ToolList } from './ToolList';
import { ToolPanel } from './ToolPanel';
import styles from './Tools.module.css';

const { label, icon } = destination('tools');

export default function ToolsPage() {
  const installed = useHasMethod('tools.list');
  if (installed === undefined) return <Loading />;
  if (!installed) return <NotInstalled />;
  return <Tools />;
}

function Loading() {
  usePageHeader({ title: label });
  return (
    <Page>
      <div className={styles.frame}>
        <div className={styles.screen} aria-busy="true">
          <div className={styles.list}>
            <Skeleton shape="block" height={28} />
            <Skeleton shape="block" height={320} />
          </div>
          <div className={styles.panel}>
            <Skeleton shape="block" height={24} width={240} />
            <Skeleton shape="block" height={180} />
          </div>
        </div>
      </div>
    </Page>
  );
}

function NotInstalled() {
  usePageHeader({ title: label });
  return (
    <Page>
      <EmptyState
        icon={icon}
        title="The toolbox is not in this build"
        description="This copy of Nocturne was assembled without the toolbox feature, so the calculators, the probes and the two small servers are not here. Nothing else is affected."
      />
    </Page>
  );
}

function Tools() {
  const catalogue = useCatalogue();
  const { tool: route } = useParams();
  const navigate = useNavigate();
  const [query, setQuery] = useState('');

  const all = useMemo(() => panels(catalogue.data ?? []), [catalogue.data]);
  const groups = useMemo(() => groupPanels(all, query), [all, query]);
  const current = selectPanel(all, route);
  /** A link to a tool this build does not have: said, not quietly redirected. */
  const unknownRoute =
    route !== undefined && all.length > 0 && !all.some((panel) => panel.tool.id === route);

  // Only the tools with a device picker need the inventory, so only they ask.
  const needsDevices = (SPECS[current?.tool.id ?? '']?.fields ?? []).some(
    (field) => field.kind === 'device',
  );
  const devices = useDevices(needsDevices);

  // Probes that are still going, from this screen or from anywhere else.
  const running = useHistory({ limit: 50 });
  const inFlight = (running.data?.runs ?? []).filter((run) => run.ok === null);
  useStatusItem(
    inFlight.length === 0
      ? null
      : {
          id: 'tools.probes',
          order: 40,
          label: `${inFlight.length} ${inFlight.length === 1 ? 'probe' : 'probes'} running`,
          tone: 'info',
          detail: inFlight.map((run) => run.title).join(', '),
          onSelect: () => navigate('/tools/probe.history'),
        },
  );

  const open = (panel: Panel) => navigate(`/tools/${panel.tool.id}`);

  usePageHeader({ title: label });

  useEffect(() => {
    const stops = [
      registerCommand({
        id: 'tools.subnet',
        title: 'Subnet calculator',
        group: 'Tools',
        keywords: ['cidr', 'netmask', 'prefix', 'hosts'],
        icon: Calculator,
        run: () => navigate('/tools/tools.subnet'),
      }),
      registerCommand({
        id: 'tools.ping',
        title: 'Ping something',
        group: 'Tools',
        keywords: ['icmp', 'echo', 'probe', 'reachable'],
        icon: Radio,
        run: () => navigate('/tools/probe.ping'),
      }),
      registerCommand({
        id: 'tools.history',
        title: 'What was probed',
        group: 'Tools',
        keywords: ['probe history', 'runs', 'ping history'],
        icon: Wrench,
        run: () => navigate('/tools/probe.history'),
      }),
    ];
    return () => {
      for (const stop of stops) stop();
    };
  }, []);

  if (catalogue.error) {
    return (
      <Page>
        <ErrorState
          title="The tool catalogue could not be read"
          error={catalogue.error}
          onRetry={() => void catalogue.refetch()}
        />
      </Page>
    );
  }

  return (
    <Page flush>
      <div className={styles.frame}>
        <div className={styles.screen}>
          <ToolList
            groups={groups}
            selected={current?.tool.id}
            query={query}
            total={all.length}
            loading={catalogue.isPending}
            onQuery={setQuery}
            onSelect={open}
          />
          <div className={styles.panelScroll}>
            {unknownRoute && (
              <p className={styles.unknown} role="status">
                This build has no tool called <code>{route}</code>. The one below is the first in
                the drawer.
              </p>
            )}
            {current ? (
              <ToolPanel key={current.tool.id} panel={current} devices={devices.data} />
            ) : (
              !catalogue.isPending && (
                <EmptyState
                  icon={icon}
                  size="compact"
                  title="This build has an empty toolbox"
                  description="The engine answers tools.list but offers no tools in it."
                />
              )
            )}
          </div>
        </div>
      </div>
    </Page>
  );
}
