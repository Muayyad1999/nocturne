/**
 * The ISP edition: subscribers, bandwidth and money.
 *
 * Eight tabs over one feature crate (`crates/nocturne-isp`, specification
 * `docs/dev/features/isp.md`): who the customers are, what each was sold,
 * what each owes, what was written off and why, and whether what they are
 * using matches what they pay for.
 *
 * Five things about this screen are not stylistic, and the modules beside it
 * exist to keep them that way.
 *
 * * **Money is an integer of minor units with its currency** (`money.ts`).
 *   `IQD` has no minor unit and `USD` has two, so `1000` is a thousand dinars
 *   or ten dollars depending on the code beside it. Nothing here divides by a
 *   hundred and nothing calls `toFixed`.
 * * **A total across currencies is not a number** — the API returns one
 *   amount per currency and no scalar, and these screens render every one of
 *   them. A single figure only ever appears where the person asked for a
 *   conversion and supplied the rates, and it is then labelled as a
 *   conversion with those rates and their date (Reports).
 * * **No figure is not a figure of zero** (`coverage.ts`). A customer whose
 *   interface nobody has ever collected has no usage figure, and the cell
 *   says so in words; the same for a plan nobody is on, a link with no
 *   capacity written down and a customer with nothing overdue.
 * * **A day is the business's day** (`calendar.ts`). Where midnight is, is a
 *   setting of the ISP's, and every date typed or shown here is read in it —
 *   never in the zone of whichever browser is open, which once billed three
 *   hours of September to a plan that started on 1 October. The Settings
 *   tab is where it is set; until it is known, or while it holds something
 *   that is not a calendar, the screen says so rather than guess.
 * * **This is bookkeeping, not control.** Nothing in the feature can touch a
 *   device: suspending an account records a decision and says that nothing
 *   was sent. There is no control on this screen that offers to disconnect
 *   anybody, and `crates/nocturne-isp/tests/boundaries.rs` holds the backend
 *   to the same thing.
 *
 * The feature is only in the paid edition, so this build may not answer any
 * `isp.*` method at all. The screen asks `app.methods` once and says so in a
 * sentence; the navigation leaves the item out entirely
 * (`app/destinations.ts`), so the free edition shows no ISP anywhere.
 */

import {
  Banknote,
  CreditCard,
  HandCoins,
  LineChart,
  Package,
  ReceiptText,
  SlidersHorizontal,
  Users,
} from 'lucide-react';
import { useEffect } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router';
import { registerCommand } from '@/app/CommandPalette/commands';
import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { EmptyState } from '@/design/EmptyState';
import { Skeleton } from '@/design/Skeleton';
import { TabPanel, Tabs, type TabItem } from '@/design/Tabs';
import { Arrears } from './Arrears';
import { isOffset } from './calendar';
import { Credits } from './Credits';
import { Customers } from './Customers';
import { useIspInstalled, useIspSettings, useSettingsWatch } from './data';
import { Invoices } from './Invoices';
import { Payments } from './Payments';
import { Plans } from './Plans';
import { Reports } from './Reports';
import { SettingsTab } from './SettingsTab';
import styles from './isp.module.css';

const { label: areaLabel, icon: areaIcon, path: areaPath } = destination('isp');

const TABS: readonly TabItem[] = [
  { id: 'customers', label: 'Customers', icon: Users },
  { id: 'invoices', label: 'Invoices', icon: CreditCard },
  { id: 'payments', label: 'Payments', icon: HandCoins },
  { id: 'credits', label: 'Credit notes', icon: ReceiptText },
  { id: 'arrears', label: 'Arrears', icon: Banknote },
  { id: 'reports', label: 'Reports', icon: LineChart },
  { id: 'plans', label: 'Tariffs and links', icon: Package },
  { id: 'settings', label: 'Settings', icon: SlidersHorizontal },
];

const DEFAULT_TAB = 'customers';

export default function IspPage() {
  const installed = useIspInstalled();
  // `=== true` on purpose: until `app.methods` has answered, a build that has
  // the feature and a build that does not look the same, and saying "not in
  // this build" about a feature that is there is the worse mistake.
  if (installed === undefined) return <Loading />;
  if (!installed) return <NotInstalled />;
  return <Isp />;
}

function Loading() {
  usePageHeader({ title: areaLabel });
  return (
    <Page>
      <Skeleton shape="block" height={32} width={520} />
      <Skeleton shape="block" height={56} />
      <Skeleton shape="block" height={360} />
    </Page>
  );
}

function NotInstalled() {
  usePageHeader({ title: areaLabel });
  return (
    <Page>
      <EmptyState
        icon={areaIcon}
        title="The ISP edition is not in this build"
        description="This copy of Nocturne was assembled without the subscriber and billing feature, so it keeps no customers, tariffs, invoices or payments. Everything else is unaffected."
      />
    </Page>
  );
}

function Isp() {
  const { tab } = useParams();
  const [search, setSearch] = useSearchParams();
  const navigate = useNavigate();

  const known = TABS.find((entry) => entry.id === tab);
  const current = known?.id ?? DEFAULT_TAB;
  const settings = useIspSettings();
  useSettingsWatch();
  // Stored, and not a calendar: the engine refuses to bill, age or read an
  // invoice until it is put right, so every tab would otherwise open on an
  // error that only names a setting. Say where to fix it, once, above them.
  const calendarBroken =
    settings.data !== undefined && !isOffset(settings.data.utc_offset_minutes);

  usePageHeader({ title: areaLabel });

  useEffect(() => {
    const stops = [
      registerCommand({
        id: 'isp.customers',
        title: 'Find a customer',
        group: areaLabel,
        keywords: ['subscriber', 'account', 'customer', 'isp'],
        icon: Users,
        run: () => navigate(`${areaPath}/customers`),
      }),
      registerCommand({
        id: 'isp.invoices',
        title: 'Open the invoices',
        group: areaLabel,
        keywords: ['invoice', 'bill', 'billing', 'draft', 'issue'],
        icon: CreditCard,
        run: () => navigate(`${areaPath}/invoices`),
      }),
      registerCommand({
        id: 'isp.run',
        title: 'Bill a period',
        group: areaLabel,
        keywords: ['billing run', 'month', 'invoice run', 'preview'],
        icon: CreditCard,
        run: () => navigate(`${areaPath}/invoices?run=1`),
      }),
      registerCommand({
        id: 'isp.credits',
        title: 'Open the credit notes',
        group: areaLabel,
        keywords: ['credit note', 'write off', 'goodwill', 'credited'],
        icon: ReceiptText,
        run: () => navigate(`${areaPath}/credits`),
      }),
      registerCommand({
        id: 'isp.settings',
        title: 'Set the business’s calendar',
        group: areaLabel,
        keywords: ['time zone', 'timezone', 'utc offset', 'calendar', 'payment terms', 'isp settings'],
        icon: SlidersHorizontal,
        run: () => navigate(`${areaPath}/settings`),
      }),
      registerCommand({
        id: 'isp.arrears',
        title: 'Who is in arrears',
        group: areaLabel,
        keywords: ['arrears', 'overdue', 'ageing', 'aging', 'debt', 'owed'],
        icon: Banknote,
        run: () => navigate(`${areaPath}/arrears`),
      }),
      registerCommand({
        id: 'isp.reports',
        title: 'Revenue, usage, oversubscription and margin',
        group: areaLabel,
        keywords: ['revenue', 'usage', 'capacity', 'oversubscription', 'margin', 'report'],
        icon: LineChart,
        run: () => navigate(`${areaPath}/reports`),
      }),
    ];
    return () => {
      for (const stop of stops) stop();
    };
  }, [navigate]);

  return (
    <Page flush>
      <Tabs
        id="isp"
        aria-label={areaLabel}
        tabs={TABS}
        value={current}
        className={styles.tabs}
        onChange={(id) => void navigate(id === DEFAULT_TAB ? areaPath : `${areaPath}/${id}`)}
      />
      <TabPanel tabs="isp" value={current} className={styles.panel}>
        {calendarBroken && current !== 'settings' && (
          <p className={styles.error} role="alert">
            The business’s calendar is not set to a calendar, so nothing can be billed, aged or
            called overdue, and no date here can be shown or read.{' '}
            <Link to={`${areaPath}/settings`}>Put it right in Settings.</Link>
          </p>
        )}
        <div className={styles.body}>
          {current === 'customers' && <Customers />}
          {current === 'invoices' && <Invoices search={search} setSearch={setSearch} />}
          {current === 'payments' && <Payments />}
          {current === 'credits' && <Credits />}
          {current === 'arrears' && <Arrears />}
          {current === 'reports' && <Reports />}
          {current === 'plans' && <Plans />}
          {current === 'settings' && <SettingsTab />}
        </div>
      </TabPanel>
    </Page>
  );
}
