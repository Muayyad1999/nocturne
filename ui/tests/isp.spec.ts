/**
 * The ISP edition's screens, against a real engine with the feature built in.
 *
 * Nothing here is mocked. A `nocturne-server --features isp` runs on a fresh
 * data directory, the UI is the production bundle it serves, and every record
 * is created through the same `/api/rpc` the app uses (`support/isp.ts`). The
 * seed is built so that each of the feature's rules has something on screen
 * to be checked against:
 *
 * * a customer billed in dinars (no minor unit) and one billed in dollars
 *   (two minor digits), shown in their own currency and never added together;
 * * a billing run that is previewed and not issued, with nothing written;
 * * a customer with a serving interface nobody has ever collected, whose
 *   usage is a sentence and not a zero;
 * * an invoice that is overdue, one that is owed but not due yet, and a draft
 *   left a draft with a pro-rated line;
 * * a payment taken in dinars against a dollar invoice, at the rate it was
 *   quoted at;
 * * an issued invoice that offers no edit, and the void-and-reissue that
 *   replaces it;
 * * credit notes: one issued from an invoice, with the four figures adding
 *   up on screen and in the engine; an invoice a note cleared, which reads
 *   "Credited" and takes no payment; a note voided, with the debt back; and
 *   a note on the account that reduces nothing until it is set against an
 *   invoice;
 * * the business's calendar: set in one click from a browser in Baghdad,
 *   after which a date typed in a browser in Auckland is still a day of the
 *   business's calendar, a month billed from the dialog or from a customer's
 *   page is the business's month, and a due date of today is today;
 * * a link whose cost and capacity are emptied in the dialog, which then has
 *   neither — and a margin that stops subtracting the cost;
 * * a rate typed for a conversion, read back with the point where the
 *   currency puts it;
 * * a list filtered to a month, which shows that month however many invoices
 *   the month after it holds;
 * * a credit note, a payment and an edit saved while one of the lists behind
 *   them is slow to answer: each is confirmed, once, and its dialog closes.
 *
 * The tests run in order and each builds on what the ones before it left in
 * the engine, as a day's work would. Every test asserts the browser console
 * stayed clean.
 *
 * No date, month or document number below is typed in. "Overdue" and "not
 * due yet" are read against the real clock, so the seed counts its dates
 * from the moment it runs and hands them back as `seeded.dates`
 * (`src/pages/isp/seed-dates.ts`, which has unit tests of its own): the
 * month it billed, the months either side of it, and the numbers the engine
 * gives invoices issued on those dates — which carry a year, and in January
 * not the same year for both. What a test issues itself is numbered in the
 * year it runs in.
 */

import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Browser, type Page } from '@playwright/test';
import type {
  Arrears,
  CreditNote,
  CustomerDetail,
  CustomerPage,
  InvoiceView,
  IspSettings,
  Link as IspLink,
  Margin,
  RunResult,
  SubscriptionRow,
} from '../src/api/types/isp';
import {
  creditNoteNumber,
  dayIn,
  monthIn,
  monthsBefore,
  yearIn,
} from '../src/pages/isp/seed-dates';
import { goTo, openApp, pageLoaded, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { seedIsp, seedManyCustomers, type Seeded } from './support/isp';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let uiDir: string;
let seeded: Seeded;
/** The draft that replaced the invoice that was voided and reissued; issued further down. */
let replacementDraft: string;

async function rpc<T>(method: string, params?: unknown): Promise<T> {
  const response = await fetch(`${backend.url}/api/rpc`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${backend.token}` },
    body: JSON.stringify({ method, params: params ?? null }),
  });
  const envelope = (await response.json()) as
    | { ok: true; result: T }
    | { ok: false; error: { code: string; message: string } };
  if (!envelope.ok) throw new Error(`${method}: ${envelope.error.code} — ${envelope.error.message}`);
  return envelope.result;
}

test.beforeAll(async () => {
  test.setTimeout(900_000);
  // Built outside `ui/dist`: the server crate embeds that directory, so
  // writing to it would make cargo rebuild the whole binary first.
  uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
  await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
    cwd: UI_DIR,
    timeout: 300_000,
  });
  backend = await startBackend({ uiDir, features: ['isp'] });

  // A `NOCTURNE_SERVER_BIN` was built by somebody else, so the feature has to
  // be checked rather than assumed: without it every assertion below would
  // fail on "the ISP edition is not in this build", which is not the defect
  // the reader would go looking for.
  const methods = await rpc<string[]>('app.methods');
  if (!methods.includes('isp.customers')) {
    throw new Error(
      'this engine has no `isp.*` methods: build it with `cargo build -p nocturne-server ' +
        '--features isp` and point NOCTURNE_SERVER_BIN at that binary. It answered ' +
        `${methods.length} methods at ${backend.url}; NOCTURNE_SERVER_BIN is ` +
        `${process.env.NOCTURNE_SERVER_BIN ?? 'unset'}.\n${backend.log().slice(-2000)}`,
    );
  }
  seeded = await seedIsp(rpc);
  await seedManyCustomers(rpc, 60);
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

// ------------------------------------------------------------------ helpers

async function openIsp(page: Page, tab = 'customers'): Promise<void> {
  await openApp(page, `/isp/${tab}`, { backend });
  await expect(page.getByRole('tab', { name: 'Customers' })).toBeVisible({ timeout: 30_000 });
}

/** Everything the page says, for the assertions about what must not be in it. */
function bodyText(page: Page): Promise<string> {
  return page.evaluate(() => document.body.innerText);
}

/**
 * A page whose browser is in a particular time zone. The business's calendar
 * is a setting of the engine's; these exist to show that the zone of the
 * browser somebody happens to be sitting at changes nothing.
 */
async function pageIn(
  browser: Browser,
  timezoneId: string,
): Promise<{ page: Page; close: () => Promise<void> }> {
  const context = await browser.newContext({
    timezoneId,
    viewport: { width: 1280, height: 800 },
    reducedMotion: 'reduce',
  });
  const page = await context.newPage();
  return { page, close: () => context.close() };
}

/**
 * The year a credit note issued now is numbered in: the business's, in the
 * calendar it had when the seed ran. Every test that issues one runs before
 * the calendar is moved.
 */
function noteYear(): number {
  return yearIn(Date.now(), seeded.dates.offset);
}

/**
 * The number of the `sequence`th credit note these tests issue. The seed
 * issues none, and each year's notes count from one.
 */
function creditNote(sequence: number): string {
  return creditNoteNumber(noteYear(), sequence);
}

// -------------------------------------------------------------------- tests

test('the rail offers the ISP edition, and the list says who is billed in what', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/', { backend });

  const rail = page.getByRole('navigation', { name: 'Primary' });
  await expect(rail.getByRole('link', { name: 'ISP', exact: true })).toBeVisible();
  await rail.getByRole('link', { name: 'ISP', exact: true }).click();
  await pageLoaded(page);

  const table = page.getByRole('table', { name: 'Customers' });

  // The list is paged in the engine, not fetched whole: 63 customers, 50 to a
  // page, and the count says which of them is on screen.
  await expect(page.getByText(/1–50 of 63/)).toBeVisible();
  await expect(table.getByRole('row').filter({ hasText: 'Dohuk Dates' })).toContainText(
    'Suspended',
  );
  await page.getByRole('button', { name: 'Next' }).click();
  await expect(page.getByText(/51–63 of 63/)).toBeVisible();

  // Ordered by name in the engine, so the last page is where Z is.
  await expect(table.getByRole('row').filter({ hasText: 'Zagros Net' })).toContainText('IQD');
  await expect(table.getByRole('row').filter({ hasText: 'Tigris Trading' })).toContainText('USD');

  // Searching narrows it in the engine too, and resets to the first page.
  const search = page.getByRole('searchbox', { name: 'Search the customers' });
  await search.fill('Zagros');
  await expect(page.getByText(/1–1 of 1/)).toBeVisible();
  await expect(table.getByRole('row').filter({ hasText: 'Zagros Net' })).toContainText('C-001');
  // The searchable text is the engine's: a phone number finds them too.
  await search.fill('750 000 0001');
  await expect(page.getByText(/1–1 of 1/)).toBeVisible();

  expect(console.messages).toEqual([]);
});

test('dinars and dollars are each shown in their own currency, and no figure adds them', async ({
  page,
}) => {
  const console = watchConsole(page);
  await openIsp(page, 'arrears');

  // One aged report per currency, side by side.
  const iqd = page.getByRole('region', { name: 'Owed in IQD' });
  const usd = page.getByRole('region', { name: 'Owed in USD' });
  await expect(iqd).toBeVisible();
  await expect(usd).toBeVisible();

  // 150 000 dinars less a 1 000 dinar payment, and 250.00 dollars less the
  // 25.00 a 33 000 dinar payment settled at 1 USD = 1 320 IQD.
  await expect(iqd).toContainText(/149\s*000\s*IQD/);
  await expect(usd).toContainText(/225\.00\s*USD/);

  // Owed, and not late, is kept apart from overdue — and the dinar invoice is
  // the overdue one. "Nothing due yet" is a sentence, not nought days.
  await expect(iqd).toContainText(/\d+ days overdue/);
  await expect(usd).toContainText('Nothing overdue');
  await expect(usd).toContainText('owed, and nobody is late');

  const text = await bodyText(page);
  // The two naive sums of those figures: 149 000 + 225, and 149 000 + 22 500
  // minor units. Neither is a number anybody is owed.
  expect(text).not.toMatch(/149\s*225/);
  expect(text).not.toMatch(/171\s*500/);
  // And no control anywhere on the screen offers to cut a customer off: this
  // is bookkeeping, and the standing note says so.
  for (const offer of [/disconnect/i, /cut off/i, /suspend the service/i, /block/i]) {
    await expect(page.getByRole('button', { name: offer })).toHaveCount(0);
  }
  await expect(page.getByText(/Nocturne does not suspend a service or change a device/)).toBeVisible();

  expect(console.messages).toEqual([]);
});

test('a billing run is previewed, and writes nothing until it is told to', async ({ page }) => {
  const console = watchConsole(page);
  const before = await rpc<InvoiceView[]>('isp.invoices', { limit: 100 });

  await openIsp(page, 'invoices');
  await page.getByRole('button', { name: "Bill a period…" }).click();
  const dialog = page.getByRole('dialog', { name: 'Bill a period' });
  await expect(dialog).toBeVisible();

  // Nothing has been asked for yet, and the dialog says so rather than
  // showing an empty result.
  await expect(dialog).toContainText('nothing has been asked for yet');
  await expect(dialog.getByRole('button', { name: 'Create drafts' })).toBeDisabled();
  await expect(dialog.getByRole('button', { name: 'Create and issue' })).toBeDisabled();

  // The month after the one the seed billed: that one is already invoiced
  // for both of them, and billing a period twice is the mistake the engine
  // refuses — which the preview says, per customer, rather than silently
  // drafting nothing.
  await dialog.getByLabel('Period').selectOption(seeded.dates.following.name);
  await dialog.getByRole('button', { name: 'Preview', exact: true }).click();

  // The first thing it says is that this is a preview.
  await expect(dialog.getByText('Preview — nothing has been written')).toBeVisible();
  await expect(dialog).toContainText('nothing was written: this is what would be billed');
  // What it comes to, per currency, with no total across them.
  await expect(dialog).toContainText(/150\s*000\s*IQD/);
  await expect(dialog).toContainText(/250\.00\s*USD/);
  const text = await dialog.innerText();
  expect(text).not.toMatch(/175\s*000/);

  // The two customers with a plan in force are billed; everybody else is
  // left out with a reason, and a suspended account is not in a run at all.
  await expect(dialog).toContainText('2 customers would be billed');
  await expect(dialog).toContainText('Zagros Net');
  await expect(dialog).toContainText('has nothing to bill for that period');

  // Nothing was written: the engine has exactly the invoices it had.
  const after = await rpc<InvoiceView[]>('isp.invoices', { limit: 100 });
  expect(after.map((invoice) => invoice.id).sort()).toEqual(
    before.map((invoice) => invoice.id).sort(),
  );

  // Only after a preview can anything be written, and issuing asks for the
  // word to be typed, because an issued invoice is frozen.
  await expect(dialog.getByRole('button', { name: 'Create drafts' })).toBeEnabled();
  await dialog.getByRole('button', { name: 'Create and issue' }).click();
  const confirm = page.getByRole('dialog', { name: /Issue \d+ invoices\?/ });
  await expect(confirm).toBeVisible();
  await expect(confirm.getByRole('button', { name: 'Issue them' })).toBeDisabled();
  await confirm.getByRole('button', { name: 'Cancel' }).click();

  // Backed out: still nothing written.
  const afterCancel = await rpc<InvoiceView[]>('isp.invoices', { limit: 100 });
  expect(afterCancel).toHaveLength(after.length);

  expect(console.messages).toEqual([]);
});

test('a customer nobody has collected says so in words, not as a usage of nothing', async ({
  page,
}) => {
  const console = watchConsole(page);
  await openIsp(page, 'reports');

  const usage = page.getByRole('region', { name: 'Used against sold' });
  await expect(usage).toBeVisible();

  // The report leads with its coverage: the reasons there is no figure,
  // counted, before any row is read.
  await expect(usage).toContainText('have a usage figure');
  await expect(usage).toContainText('which is not a usage of zero');
  await expect(usage).toContainText(/\d+ · Never collected/);
  await expect(usage).toContainText(/\d+ · No serving device/);

  const table = usage.getByRole('table', {
    name: 'Customers, what each was sold and what each is using',
  });
  // Tigris has a device and an interface, and monitoring has never recorded
  // that interface: there is no figure, and the cell is a sentence.
  const tigris = table.getByRole('row').filter({ hasText: 'Tigris Trading' });
  await expect(tigris).toContainText('Never collected');
  await expect(tigris).toContainText('50 Mb/s');
  await expect(tigris).toContainText('no figure');

  // Zagros has no serving device at all, which is a different nothing.
  const zagros = table.getByRole('row').filter({ hasText: 'Zagros Net' });
  await expect(zagros).toContainText('No serving device');

  // Neither row says 0, and neither says only a dash.
  for (const row of [tigris, zagros]) {
    const cells = await row.getByRole('cell').allInnerTexts();
    const using = cells.at(3) ?? '';
    expect(using).not.toMatch(/\d/);
    expect(using).not.toBe('—');
  }

  expect(console.messages).toEqual([]);
});

test('an issued invoice offers no edit, and a wrong one is voided and reissued', async ({ page }) => {
  const console = watchConsole(page);
  const { overdue, open, partPeriod, earlier } = seeded.dates;
  await openIsp(page, 'invoices');

  const table = page.getByRole('table', { name: 'Invoices' });
  await expect(table.getByRole('row').filter({ hasText: overdue.number })).toContainText(
    'Overdue',
  );

  // The dollar invoice: issued, part paid by a payment taken in dinars.
  await table.getByRole('button', { name: open.number }).click();
  const drawer = page.getByRole('dialog', { name: open.number });
  await expect(drawer).toBeVisible();
  await expect(drawer).toContainText('Part paid');
  await expect(drawer).toContainText(/225\.00\s*USD/);
  // Issued means frozen: no line may be added, and the drawer does not offer.
  await expect(drawer.getByRole('button', { name: 'Add a line…' })).toHaveCount(0);
  // It has a payment against it, so voiding it is refused — and said so here.
  await expect(drawer).toContainText('hide money that was actually received');
  await expect(drawer.getByRole('button', { name: 'Void…' })).toHaveCount(0);
  // The rate the dinar payment was taken at is on the payment, as quoted.
  await expect(drawer).toContainText(/1 USD = 1\s*320\s*IQD/);
  await page.getByRole('button', { name: 'Close' }).click();

  // The overdue dinar invoice has one payment too, so take the draft instead:
  // a draft can be edited, and is not issued to anybody.
  await page.getByLabel('Invoice state').selectOption('draft');
  const draft = table.getByRole('row').filter({ hasText: 'Dohuk Dates' });
  await expect(draft).toContainText('not issued');
  await draft.getByRole('button', { name: 'not issued' }).click();
  const draftDrawer = page.getByRole('dialog', { name: 'Draft invoice' });
  await expect(draftDrawer.getByRole('button', { name: 'Add a line…' })).toBeVisible();
  // A part period carries its arithmetic, so the figure can be rechecked:
  // the plan began on the 16th, and the month has as many days as it has.
  await expect(draftDrawer).toContainText(partPeriod.words);
  await page.getByRole('button', { name: 'Close' }).click();

  // Now reissue the invoice that has no payment: the original is voided, keeps
  // its number, and a draft replacement is made. It is for the month before
  // the one the seed billed, which their plan was already in force for.
  const clean = await rpc<{ id: string }>('isp.invoice.issue', {
    id: (
      await rpc<{ invoice: { id: string } }>('isp.invoice.draft', {
        customer_id: seeded.zagros,
        period_start: earlier.start,
        period_end: earlier.end,
      })
    ).invoice.id,
  });
  await page.reload();
  await pageLoaded(page);
  await page.getByLabel('Invoice state').selectOption('issued');
  const issued = await rpc<InvoiceView>('isp.invoice.get', { id: clean.id });
  await table.getByRole('button', { name: issued.number ?? '' }).click();
  const reissuing = page.getByRole('dialog', { name: issued.number ?? '' });
  await reissuing.getByRole('button', { name: 'Void and reissue…' }).click();
  const ask = page.getByRole('dialog', { name: /Void this invoice and draft its replacement/ });
  await ask.getByLabel('Reason').fill('The megabits were wrong');
  await ask.getByRole('button', { name: 'Void and reissue' }).click();
  await expect(page.getByText('Voided, and a draft replacement is ready')).toBeVisible();

  const voided = await rpc<InvoiceView>('isp.invoice.get', { id: clean.id });
  expect(voided.state).toBe('void');
  expect(voided.number).toBe(issued.number);
  expect(voided.replaced_by).not.toBeNull();
  replacementDraft = voided.replaced_by ?? '';

  expect(console.messages).toEqual([]);
});

test('a payment taken in another currency keeps the rate it was taken at', async ({ page }) => {
  const console = watchConsole(page);
  await openIsp(page, 'payments');

  const table = page.getByRole('table', { name: 'Payments' });
  const cross = table.getByRole('row').filter({ hasText: 'R-1002' });
  await expect(cross).toContainText(/33\s*000\s*IQD/);
  await expect(cross).toContainText(/25\.00\s*USD/);
  await expect(cross).toContainText(/1 USD = 1\s*320\s*IQD/);

  // The dinar payment on the dinar invoice needed no rate, and says so rather
  // than showing an empty cell.
  const same = table.getByRole('row').filter({ hasText: 'R-1001' });
  await expect(same).toContainText('same currency');

  expect(console.messages).toEqual([]);
});

test('an amount is entered as whole minor units, and a dinar has no halves', async ({ page }) => {
  const console = watchConsole(page);
  await openIsp(page, 'payments');

  await page.getByRole('button', { name: 'Record a payment…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Record a payment' });
  await dialog.getByRole('combobox', { name: 'Customer' }).click();
  // The combobox's list is a portal at the end of the document, not a child
  // of the dialog, so it is asked for from the page.
  await page.getByRole('option', { name: /Zagros Net/ }).click();

  const amount = dialog.getByLabel('Amount received');
  await amount.fill('1.5');
  // IQD has no minor unit, so a half dinar is refused before the round trip.
  await expect(dialog).toContainText('whole units');
  await expect(dialog.getByRole('button', { name: 'Record it' })).toBeDisabled();

  await amount.fill('5000');
  // What will be sent, in that currency's own terms.
  await expect(dialog).toContainText('Sent as 5 000 minor units of IQD');
  await dialog.getByLabel('How it came in').fill('cash');
  await dialog.getByRole('button', { name: 'Record it' }).click();
  await expect(page.getByText(/5\s*000\s*IQD received, on account/)).toBeVisible();

  // It is on account: real money, in the currency it arrived in, not against
  // an invoice.
  const payments = await rpc<{ amount: { minor: number }; invoice_id: string | null }[]>(
    'isp.payments',
    { customer_id: seeded.zagros, limit: 10 },
  );
  const onAccount = payments.find((payment) => payment.invoice_id === null);
  expect(onAccount?.amount.minor).toBe(5000);

  expect(console.messages).toEqual([]);
});

test('a customer page shows what they were sold, what they owe and what they are using', async ({
  page,
}) => {
  const console = watchConsole(page);
  await openApp(page, `/isp/customer/${seeded.tigris}`, { backend });
  // The page's own heading, in the header, with the breadcrumb back to the
  // list; the name is not repeated in the body.
  await expect(page.getByRole('heading', { level: 1, name: 'Tigris Trading' })).toBeVisible({
    timeout: 30_000,
  });
  await expect(page.getByRole('main').getByRole('heading', { name: 'Tigris Trading' })).toHaveCount(
    0,
  );

  await expect(page.getByText('billed in USD')).toBeVisible();
  const sold = page.getByRole('region', { name: 'What they were sold' });
  await expect(sold).toContainText('Business 50');
  await expect(sold).toContainText('50 Mbit/s, flat, a month');
  await expect(sold).toContainText(/250\.00\s*USD/);

  const owed = page.getByRole('region', { name: 'What they owe' });
  await expect(owed).toContainText(/225\.00\s*USD/);
  await expect(owed).toContainText('1 issued invoice still short');

  const using = page.getByRole('region', {
    name: 'What they are using against what they pay for',
  });
  await expect(using).toContainText('50 Mb/s');
  await expect(using).toContainText('Never collected');
  await expect(using).toContainText('nobody has looked, which is not the same as nothing passing');
  await expect(using).toContainText('no figure');

  // Their invoice and the payment against it are both here.
  await expect(page.getByRole('region', { name: 'Invoices' })).toContainText(
    seeded.dates.open.number,
  );
  await expect(page.getByRole('region', { name: 'Payments' })).toContainText(/33\s*000\s*IQD/);

  // Marking an account says what it does, which is to record a decision.
  await page.getByRole('button', { name: 'Mark suspended' }).click();
  const confirm = page.getByRole('dialog', { name: /Mark Tigris Trading suspended\?/ });
  await expect(confirm).toContainText('records a decision about the account');
  await expect(confirm).toContainText('somebody changes the router');
  await confirm.getByRole('button', { name: 'Cancel' }).click();

  expect(console.messages).toEqual([]);
});

test('a credit note issued from an invoice is a figure of its own, and the four add up', async ({
  page,
}) => {
  const console = watchConsole(page);
  const open = seeded.dates.open.number;
  const note = creditNote(1);
  await openIsp(page, 'invoices');
  const table = page.getByRole('table', { name: 'Invoices' });

  // The dollar invoice: 250.00, of which a dinar payment settled 25.00.
  await table.getByRole('button', { name: open }).click();
  const drawer = page.getByRole('dialog', { name: open });
  const figures = drawer.getByRole('group', { name: 'Settlement' });
  await expect(figures).toContainText(/Total\s*250\.00\s*USD/);
  await expect(figures).toContainText(/Paid\s*25\.00\s*USD/);
  await expect(figures).toContainText(/Credited\s*0\.00\s*USD/);
  await expect(figures).toContainText(/Outstanding\s*225\.00\s*USD/);
  await expect(drawer.getByRole('group', { name: 'Credit notes' })).toContainText(
    'no credit note stands against this invoice',
  );

  await drawer.getByRole('button', { name: 'Issue a credit note…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Issue a credit note' });
  // What a credit note is, before anything else: not money, and not a refund.
  await expect(dialog).toContainText('it is not money');
  // The cap is what was charged, not what is left: 250.00, though 225.00 is owed.
  await expect(dialog).toContainText(/Up to 250\.00\s*USD more can be credited/);
  const issue = dialog.getByRole('button', { name: 'Issue the credit note' });

  // More than was ever charged is refused here, before the round trip.
  await dialog.getByLabel('Amount to credit').fill('300');
  await dialog.getByLabel('Reason').fill('Four days without service after the storm');
  await expect(dialog).toContainText(/at most 250\.00\s*USD more can be/);
  await expect(issue).toBeDisabled();

  // And a note with no reason is not a note.
  await dialog.getByLabel('Amount to credit').fill('100.00');
  await dialog.getByLabel('Reason').fill('');
  await expect(issue).toBeDisabled();
  await dialog.getByLabel('Reason').fill('Four days without service after the storm');
  // What it will leave is said before it is done.
  await expect(dialog).toContainText(/125\.00\s*USD would still be outstanding/);
  await issue.click();
  await expect(
    page.getByText(new RegExp(`${note}: 100\\.00\\s*USD credited on invoice ${open}`)),
  ).toBeVisible();

  // The drawer follows `isp.credits.changed`: all four figures, adding up,
  // with what was credited kept apart from what was paid.
  await expect(figures).toContainText(/Total\s*250\.00\s*USD/);
  await expect(figures).toContainText(/Paid\s*25\.00\s*USD/);
  await expect(figures).toContainText(/Credited\s*100\.00\s*USD/);
  await expect(figures).toContainText(/Outstanding\s*125\.00\s*USD/);
  await expect(figures).toContainText('written off by 1 credit note, not received');
  // Money is still owed and some was paid: it is part paid, not credited.
  await expect(drawer).toContainText('Part paid');
  const notes = drawer.getByRole('group', { name: 'Credit notes' });
  await expect(notes).toContainText(note);
  await expect(notes).toContainText('Four days without service after the storm');
  // A note stands against it now, so voiding the invoice is refused by the
  // engine — and is not offered, with the reason said here.
  await expect(drawer).toContainText(`Credit note ${note} stands against this invoice`);
  await expect(drawer.getByRole('button', { name: 'Void and reissue…' })).toHaveCount(0);

  // The engine's own figures, which are what the screen showed.
  const invoice = await rpc<InvoiceView>('isp.invoice.get', { id: seeded.openInvoice });
  expect(invoice.credited.minor).toBe(10_000);
  expect(invoice.total.minor - invoice.paid.minor - invoice.credited.minor).toBe(
    invoice.outstanding.minor,
  );
  expect(invoice.outstanding.minor).toBe(12_500);
  expect(invoice.status).toBe('part_paid');
  await page.getByRole('button', { name: 'Close' }).click();

  // In the list what was credited is a figure of its own, in its own words,
  // under what was paid — so the row adds up too.
  const row = table.getByRole('row').filter({ hasText: open });
  await expect(row).toContainText(/25\.00\s*USD\s*100\.00\s*USD credited/);
  await expect(row).toContainText(/125\.00\s*USD/);

  // A small dinar draft, so that the list holds an amount in dinars that is
  // a smaller number than one in dollars: 3 000 IQD against 12 500 cents. It
  // is for the month two before the one the seed billed, which nothing else
  // here bills; the month filter's test looks for it there.
  const small = await rpc<CustomerPage>('isp.customers', { query: 'Subscriber 0002' });
  const empty = await rpc<{ invoice: { id: string } }>('isp.invoice.draft', {
    customer_id: small.customers[0]?.id,
    month: seeded.dates.earliest.name,
    allow_empty: true,
  });
  await rpc('isp.invoice.line.add', {
    invoice_id: empty.invoice.id,
    description: 'Installation',
    amount: { currency: 'IQD', minor: 3_000 },
  });
  await expect(table.getByRole('row').filter({ hasText: 'Subscriber 0002' })).toBeVisible();

  // Sorted by what is outstanding, the dinars stay together and the dollars
  // stay together: 3 000 dinars is not ranked against 125.00 dollars.
  await table.getByRole('columnheader', { name: 'Outstanding' }).getByRole('button').click();
  await table.getByRole('columnheader', { name: 'Outstanding' }).getByRole('button').click();
  await expect(table.getByRole('columnheader', { name: 'Outstanding' })).toHaveAttribute(
    'aria-sort',
    'descending',
  );
  const outstanding = await table.locator('tbody tr').evaluateAll((rows) =>
    rows.map((entry) => (entry as HTMLTableRowElement).cells[5]?.innerText ?? ''),
  );
  const currencies = outstanding.map((cell) => cell.trim().slice(-3));
  expect(currencies).toContain('USD');
  expect(currencies.filter((code) => code === 'IQD').length).toBeGreaterThan(1);
  const firstDinar = currencies.indexOf('IQD');
  const lastDinar = currencies.lastIndexOf('IQD');
  expect(currencies.slice(firstDinar, lastDinar + 1).every((code) => code === 'IQD')).toBe(true);
  // And within the dinars it is still an order: the 3 000 comes last.
  expect(outstanding[lastDinar]).toMatch(/^3\s*000\s*IQD/);

  expect(console.messages).toEqual([]);
});

test('an invoice a credit note cleared reads Credited and takes no payment, until the note is voided', async ({
  page,
}) => {
  const console = watchConsole(page);
  const open = seeded.dates.open.number;
  const first = creditNote(1);
  const second = creditNote(2);
  await openApp(page, `/isp/customer/${seeded.tigris}`, { backend });
  await expect(page.getByRole('heading', { level: 1, name: 'Tigris Trading' })).toBeVisible({
    timeout: 30_000,
  });

  // Their notes are on their page, with where each one stands.
  const theirNotes = page.getByRole('region', { name: 'Credit notes' });
  await expect(theirNotes).toContainText(first);
  await expect(theirNotes).toContainText(`against invoice ${open}`);

  // Write off the rest: 125.00 is what is still owed.
  await page.getByRole('button', { name: 'Issue a credit note…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Issue a credit note' });
  await dialog.getByLabel('Against').selectOption(seeded.openInvoice);
  await dialog.getByLabel('Amount to credit').fill('125.00');
  await dialog.getByLabel('Reason').fill('Goodwill, agreed by the owner');
  await expect(dialog).toContainText('Nothing would be left outstanding');
  await dialog.getByRole('button', { name: 'Issue the credit note' }).click();
  await expect(
    page.getByText(new RegExp(`${second}: 125\\.00\\s*USD credited on invoice`)),
  ).toBeVisible();

  // Nothing more is owed — and the word is "Credited", for a person, because
  // 25.00 of payments did not cover 250.00: the rest was written off.
  const invoices = page.getByRole('region', { name: 'Invoices' });
  const row = invoices.getByRole('row').filter({ hasText: open });
  await expect(row).toContainText('Credited');
  await expect(row).not.toContainText('credited');
  // Its due date is weeks away and it is settled: it does not say "not due yet".
  await expect(row).toContainText('Settled');
  await expect(row).not.toContainText('Not due yet');
  const owed = page.getByRole('region', { name: 'What they owe' });
  await expect(owed).toContainText('nothing is owed');

  await row.getByRole('button', { name: open }).click();
  const drawer = page.getByRole('dialog', { name: open });
  await expect(drawer).toContainText('it took a credit note to get there');
  const figures = drawer.getByRole('group', { name: 'Settlement' });
  await expect(figures).toContainText(/Paid\s*25\.00\s*USD/);
  await expect(figures).toContainText(/Credited\s*225\.00\s*USD/);
  await expect(figures).toContainText(/Outstanding\s*0\.00\s*USD/);
  // Nothing is left to pay, so a payment is not offered.
  await expect(drawer.getByRole('button', { name: 'Record a payment…' })).toHaveCount(0);

  // The second note was a mistake. It is voided, with a reason, and kept.
  await drawer.getByRole('button', { name: `Void ${second}` }).click();
  const voiding = page.getByRole('dialog', { name: `Void credit note ${second}` });
  await expect(voiding).toContainText(/puts back the 125\.00\s*USD it wrote off/);
  await expect(voiding.getByRole('button', { name: 'Void it' })).toBeDisabled();
  await voiding.getByLabel('Reason').fill('Agreed for a different customer');
  await voiding.getByRole('button', { name: 'Void it' }).click();
  await expect(
    page.getByText(`Credit note ${second} is void and the record is kept`),
  ).toBeVisible();

  // The debt is back, the note is still there with why it was withdrawn,
  // and the invoice can be paid again.
  await expect(figures).toContainText(/Credited\s*100\.00\s*USD/);
  await expect(figures).toContainText(/Outstanding\s*125\.00\s*USD/);
  await expect(drawer).toContainText('Part paid');
  const notes = drawer.getByRole('group', { name: 'Credit notes' });
  await expect(notes).toContainText(second);
  await expect(notes).toContainText('Agreed for a different customer');
  await expect(drawer.getByRole('button', { name: 'Record a payment…' })).toBeVisible();
  await expect(drawer.getByRole('button', { name: `Void ${second}` })).toHaveCount(0);

  const invoice = await rpc<InvoiceView>('isp.invoice.get', { id: seeded.openInvoice });
  expect(invoice.status).toBe('part_paid');
  expect(invoice.credited.minor).toBe(10_000);
  expect(invoice.credits.map((credit) => credit.state).sort()).toEqual(['issued', 'void']);

  expect(console.messages).toEqual([]);
});

test('what stands in a customer’s favour is shown beside what is owed, and taken off nothing', async ({
  page,
}) => {
  const console = watchConsole(page);
  // 130.00 arrives against the 125.00 that is left on the dollar invoice:
  // 5.00 more than it asks for.
  await rpc('isp.payment.record', {
    customer_id: seeded.tigris,
    invoice_id: seeded.openInvoice,
    amount: { currency: 'USD', text: '130.00' },
    method: 'bank transfer',
    reference: 'R-1003',
  });

  // The report somebody chases debts from says so, in its own card — and
  // the dinars that are owed are still owed in full.
  await openIsp(page, 'arrears');
  const favour = page.getByRole('region', { name: /In customers.s? favour, in USD/ });
  await expect(favour).toContainText(/5\.00\s*USD/);
  await expect(favour).toContainText('Tigris Trading');
  await expect(favour).toContainText('not taken off anything owed');
  await expect(page.getByRole('region', { name: 'Owed in IQD' })).toContainText(/149\s*000\s*IQD/);
  await expect(page.getByRole('region', { name: 'Owed in USD' })).toHaveCount(0);

  // On the invoice there is no negative "outstanding": the four figures
  // still add up, and the last one says whose favour it is in.
  await openApp(page, `/isp/invoices?invoice=${seeded.openInvoice}`, { backend });
  const drawer = page.getByRole('dialog', { name: seeded.dates.open.number });
  const figures = drawer.getByRole('group', { name: 'Settlement' });
  await expect(figures).toContainText(/Total\s*250\.00\s*USD/);
  await expect(figures).toContainText(/Paid\s*155\.00\s*USD/);
  await expect(figures).toContainText(/Credited\s*100\.00\s*USD/);
  await expect(figures).toContainText(/In their favour\s*5\.00\s*USD/);
  await expect(figures).not.toContainText('Outstanding');
  await expect(figures).not.toContainText('−');
  // Payments alone did not cover it — 155.00 of 250.00 — so it is credited.
  await expect(drawer).toContainText('Credited');
  await expect(drawer.getByRole('button', { name: 'Record a payment…' })).toHaveCount(0);

  const invoice = await rpc<InvoiceView>('isp.invoice.get', { id: seeded.openInvoice });
  expect(invoice.total.minor - invoice.paid.minor - invoice.credited.minor).toBe(
    invoice.outstanding.minor,
  );
  expect(invoice.outstanding.minor).toBe(-500);
  await page.getByRole('button', { name: 'Close' }).click();

  // And on their own page it is a figure beside what they owe.
  await openApp(page, `/isp/customer/${seeded.tigris}`, { backend });
  const owed = page.getByRole('region', { name: 'What they owe' });
  await expect(owed).toContainText(/In their favour\s*5\.00\s*USD/);
  await expect(owed).toContainText('1 invoice settled beyond its total');
  await expect(owed).toContainText('nothing is owed');

  expect(console.messages).toEqual([]);
});

test('a credit note on the account reduces nothing until it is set against an invoice', async ({
  page,
}) => {
  const console = watchConsole(page);
  const { overdue, open } = seeded.dates;
  const third = creditNote(3);
  await openIsp(page, 'credits');
  const table = page.getByRole('table', { name: 'Credit notes' });

  // Every note is listed, the void one included, with where it stands.
  await expect(table.getByRole('row').filter({ hasText: creditNote(1) })).toContainText(
    `against invoice ${open.number}`,
  );
  await expect(table.getByRole('row').filter({ hasText: creditNote(2) })).toContainText(
    'Void: Agreed for a different customer',
  );

  await page.getByRole('button', { name: 'Issue a credit note…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Issue a credit note' });
  await dialog.getByRole('combobox', { name: 'Customer' }).click();
  await page.getByRole('option', { name: /Zagros Net/ }).click();
  // No currency to choose: a note is in the currency the customer is billed in.
  await expect(dialog).toContainText('In IQD, the currency this customer is billed in');
  await dialog.getByLabel('Amount to credit').fill('9000');
  await dialog.getByLabel('Reason').fill('Router swap took a week');
  await dialog.getByRole('button', { name: 'Issue the credit note' }).click();
  await expect(
    page.getByText(new RegExp(`${third}: 9\\s*000\\s*IQD credited on the account`)),
  ).toBeVisible();

  const onAccount = table.getByRole('row').filter({ hasText: third });
  await expect(onAccount).toContainText('on the account: it reduces nothing until it is set against');
  await expect(onAccount).toContainText('Zagros Net');

  // It is on their balance as its own figure, and what they owe has not moved.
  const before = await rpc<CustomerDetail>('isp.customer.get', { id: seeded.zagros });
  expect(before.balance.credit_on_account.map((money) => money.minor)).toEqual([9_000]);
  expect(before.balance.outstanding.map((money) => money.minor)).toEqual([149_000]);

  await openApp(page, `/isp/customer/${seeded.zagros}`, { backend });
  const owed = page.getByRole('region', { name: 'What they owe' });
  await expect(owed).toContainText(/Outstanding\s*149\s*000\s*IQD/);
  await expect(owed).toContainText(/Credit on account\s*9\s*000\s*IQD/);
  await expect(owed).toContainText('credit notes, not money');
  // The 5 000 they paid on account is a different thing, and a different figure.
  await expect(owed).toContainText(/On account\s*5\s*000\s*IQD/);
  const text = await owed.innerText();
  expect(text).not.toMatch(/14\s*000\s*IQD/);
  expect(text).not.toMatch(/140\s*000\s*IQD/);

  // Set it against the overdue invoice: the same note, now reducing a debt.
  const theirNotes = page.getByRole('region', { name: 'Credit notes' });
  await theirNotes.getByRole('button', { name: `Set ${third} against an invoice` }).click();
  const allocate = page.getByRole('dialog', { name: `Set ${third} against an invoice` });
  await expect(allocate).toContainText('It happens once');
  await allocate.getByLabel('Invoice').selectOption(seeded.overdueInvoice);
  await expect(allocate).toContainText(/140\s*000\s*IQD would still be outstanding/);
  await allocate.getByRole('button', { name: 'Set it against the invoice' }).click();
  await expect(
    page.getByText(`${third} now stands against invoice ${overdue.number}`),
  ).toBeVisible();

  await expect(owed).toContainText(/Outstanding\s*140\s*000\s*IQD/);
  await expect(owed).toContainText('no credit note on the account');
  await expect(theirNotes).toContainText(`against invoice ${overdue.number}`);
  // Once: the note no longer offers to be set against anything.
  await expect(
    theirNotes.getByRole('button', { name: `Set ${third} against an invoice` }),
  ).toHaveCount(0);

  const after = await rpc<CustomerDetail>('isp.customer.get', { id: seeded.zagros });
  expect(after.balance.credit_on_account).toEqual([]);
  expect(after.balance.outstanding.map((money) => money.minor)).toEqual([140_000]);
  const note = (await rpc<CreditNote[]>('isp.credits', { customer_id: seeded.zagros }))[0];
  expect(note?.number).toBe(third);
  expect(note?.invoice_id).toBe(seeded.overdueInvoice);
  expect(note?.allocated_at).not.toBeNull();

  // And the arrears somebody chases from have moved with it.
  const arrears = await rpc<Arrears>('isp.arrears', {});
  expect(arrears.by_currency.find((entry) => entry.currency === 'IQD')?.total.minor).toBe(140_000);

  expect(console.messages).toEqual([]);
});

test('a credit note, a payment and an edit are each confirmed once, however slowly the lists behind them are read again', async ({
  page,
}) => {
  const console = watchConsole(page);
  // A save waits for the lists behind it to be read again before it reports
  // back. One of those reads is held here, so that the invoice — or the
  // customer — the dialog was opened on has answered, and changed, well
  // before the save says anything. That is any slow link, and it used to
  // empty the form and lose the answer with it: the note had been issued,
  // and the dialog stood open and blank, asking to be filled in again.
  const slow = new Set<string>();
  await page.route(`${backend.url}/api/rpc`, async (route) => {
    const body = route.request().postDataJSON() as { method?: string };
    if (body?.method && slow.has(body.method)) {
      await new Promise((done) => setTimeout(done, 1_500));
    }
    return route.fallback();
  });
  const issuedNotes = async () =>
    (await rpc<CreditNote[]>('isp.credits', { invoice_id: seeded.overdueInvoice, state: 'issued' }))
      .length;
  const recordedPayments = async () =>
    (
      await rpc<{ id: string }[]>('isp.payments', {
        invoice_id: seeded.overdueInvoice,
        state: 'recorded',
      })
    ).length;
  const notesBefore = await issuedNotes();
  const paymentsBefore = await recordedPayments();

  const overdue = seeded.dates.overdue.number;
  await openApp(page, `/isp/invoices?invoice=${seeded.overdueInvoice}`, { backend });
  const drawer = page.getByRole('dialog', { name: overdue });
  const figures = drawer.getByRole('group', { name: 'Settlement' });
  await expect(figures).toContainText(/Outstanding\s*140\s*000\s*IQD/);

  // A credit note, from the invoice.
  await drawer.getByRole('button', { name: 'Issue a credit note…' }).click();
  const crediting = page.getByRole('dialog', { name: 'Issue a credit note' });
  await crediting.getByLabel('Amount to credit').fill('1000');
  await crediting.getByLabel('Reason').fill('Two days of outage in the storm');
  slow.add('isp.summary');
  await crediting.getByRole('button', { name: 'Issue the credit note' }).click();
  // It says so, and closes: there is nothing left on screen to send again.
  await expect(
    page.getByText(
      new RegExp(`CN-${noteYear()}-\\d{4}: 1\\s*000\\s*IQD credited on invoice ${overdue}`),
    ),
  ).toBeVisible({ timeout: 15_000 });
  await expect(crediting).toHaveCount(0);
  slow.clear();
  await expect(figures).toContainText(/Outstanding\s*139\s*000\s*IQD/);
  expect(await issuedNotes()).toBe(notesBefore + 1);

  // A payment, from the same invoice.
  await drawer.getByRole('button', { name: 'Record a payment…' }).click();
  const paying = page.getByRole('dialog', { name: 'Record a payment' });
  await paying.getByLabel('Amount received').fill('1000');
  await paying.getByLabel('How it came in').fill('cash');
  slow.add('isp.summary');
  await paying.getByRole('button', { name: 'Record it' }).click();
  await expect(page.getByText(/1\s*000\s*IQD received, settling 1\s*000\s*IQD/)).toBeVisible({
    timeout: 15_000,
  });
  await expect(paying).toHaveCount(0);
  slow.clear();
  await expect(figures).toContainText(/Outstanding\s*138\s*000\s*IQD/);
  expect(await recordedPayments()).toBe(paymentsBefore + 1);
  await page.getByRole('button', { name: 'Close' }).click();

  // And an edit to the customer the page itself is showing.
  await goTo(page, `/isp/customer/${seeded.zagros}`);
  await page.getByRole('button', { name: 'Edit details…' }).click();
  const editing = page.getByRole('dialog', { name: 'Edit Zagros Net' });
  await editing.getByLabel('Notes').fill('Prefers to be called before a visit');
  slow.add('isp.payments');
  await editing.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Zagros Net was updated')).toBeVisible({ timeout: 15_000 });
  await expect(editing).toHaveCount(0);
  slow.clear();
  await expect(page.getByText('Prefers to be called before a visit')).toBeVisible();

  expect(console.messages).toEqual([]);
});

test('a cost and a capacity emptied in the link dialog are taken off the link, and the margin stops using the cost', async ({
  page,
}) => {
  const console = watchConsole(page);
  await openIsp(page, 'plans');
  const links = page.getByRole('table', { name: 'Links and tower feeds' });
  const row = links.getByRole('row').filter({ hasText: 'Uplink A' });
  await expect(row).toContainText(/3\.00\s*USD/);
  await expect(row).toContainText('100 Mb/s');
  const before = (await rpc<Margin[]>('isp.margin', {})).find((entry) => entry.id === seeded.linkId);
  // 100 Mbit/s at 3.00 a megabit: the margin is worked out against 300.00.
  expect(before?.cost?.minor).toBe(30_000);
  expect(before?.margin).not.toBeNull();

  // Somebody wrote both figures down in error and takes them off again.
  await row.dblclick();
  const dialog = page.getByRole('dialog', { name: 'Edit Uplink A' });
  const capacity = dialog.getByLabel('Capacity in Mbit/s');
  const cost = dialog.getByLabel('Wholesale cost of one megabit');
  await expect(capacity).toHaveValue('100');
  await expect(cost).toHaveValue('3.00');
  await capacity.fill('');
  await cost.fill('');
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Uplink A was updated')).toBeVisible();

  // "Updated" has to mean it: the link has neither figure any more — in the
  // list, in the engine, and when the dialog is opened again.
  await expect(row).toContainText('no cost recorded');
  await expect(row).toContainText('not recorded');
  const stored = (await rpc<IspLink[]>('isp.links')).find((link) => link.id === seeded.linkId);
  expect(stored?.cost_per_mbit).toBeNull();
  expect(stored?.capacity_mbit).toBe(0);
  // And the margin has stopped subtracting a cost nobody records: no cost
  // is no margin, not a margin against a cost of nothing.
  const after = (await rpc<Margin[]>('isp.margin', {})).find((entry) => entry.id === seeded.linkId);
  expect(after?.cost).toBeNull();
  expect(after?.margin).toBeNull();

  await row.dblclick();
  await expect(capacity).toHaveValue('');
  await expect(cost).toHaveValue('');
  // Written down again, they are back: the same save sets what it can clear.
  await capacity.fill('100');
  await cost.fill('3.00');
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(row).toContainText(/3\.00\s*USD/);
  const again = (await rpc<IspLink[]>('isp.links')).find((link) => link.id === seeded.linkId);
  expect(again?.cost_per_mbit?.minor).toBe(300);
  expect(again?.capacity_mbit).toBe(100);

  expect(console.messages).toEqual([]);
});

test('a rate typed for a conversion is read back with the point where the currency puts it', async ({
  page,
}) => {
  const console = watchConsole(page);
  await openIsp(page, 'reports');
  const revenue = page.getByRole('region', { name: 'Revenue' });
  // Dinars and dollars both came in over the last thirty days, so one figure
  // needs a rate — which is typed in minor units, because that is what fits
  // in a whole number.
  await revenue.getByLabel('One figure, at rates you supply').selectOption('IQD');
  const quoted = revenue.getByLabel('1 IQD = ? minor units of USD');
  await expect(revenue).toContainText('a whole number, as it was quoted to you');
  // The figure is only there for where its point goes: 108 minor units of a
  // currency with two decimals is 1.08, and echoing it as "108 USD" would
  // state the rate a hundredfold.
  await quoted.fill('108');
  await expect(revenue).toContainText(/as quoted: 1 IQD = 1\.08\s*USD/);
  await expect(revenue).not.toContainText(/= 108\s*USD/);

  // The way a dinar rate is really quoted is the other way round, and a
  // dinar has no minor unit: 1 320 is 1 320.
  await revenue.getByRole('button', { name: 'Swap the direction of the USD rate' }).click();
  const swapped = revenue.getByLabel('1 USD = ? minor units of IQD');
  await swapped.fill('1320');
  await expect(revenue).toContainText(/as quoted: 1 USD = 1\s*320\s*IQD/);
  await revenue.getByRole('button', { name: 'Convert' }).click();
  // What comes back is labelled a conversion and names the rate it used.
  await expect(revenue.getByText('Invoiced, converted')).toBeVisible();
  await expect(revenue).toContainText(/1 USD = 1\s*320\s*IQD/);

  expect(console.messages).toEqual([]);
});

test('an invoice may fall due today, and is not late on its due day — in whatever zone it is issued from', async ({
  browser,
}) => {
  // The operator's browser is in Auckland, where it is already tomorrow for
  // half of the business's day. The invoice was drafted in the business's
  // calendar (UTC, as a fresh installation is) and its dates are days of that.
  const { page, close } = await pageIn(browser, 'Pacific/Auckland');
  const console = watchConsole(page);
  const settings = await rpc<IspSettings>('settings.get', { section: 'isp' });
  const today = dayIn(Date.now(), settings.utc_offset_minutes);

  await openApp(page, `/isp/invoices?invoice=${replacementDraft}`, { backend });
  const draft = page.getByRole('dialog', { name: 'Draft invoice' });
  await draft.getByRole('button', { name: 'Issue…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Issue this invoice' });
  // The hint says which day it is in the invoice's calendar, not the browser's.
  await expect(dialog).toContainText(`today is ${today}`);
  await dialog.getByLabel('Due date').fill(today);
  // Numbered in the year it is issued in, which is this one.
  const year = yearIn(Date.now(), settings.utc_offset_minutes);
  await dialog.getByRole('button', { name: 'Issue it' }).click();
  await expect(
    page.getByText(new RegExp(`Invoice ${year}-\\d{4} is issued to Zagros Net`)),
  ).toBeVisible();

  const issued = await rpc<InvoiceView>('isp.invoice.get', { id: replacementDraft });
  // Due on the day that was typed, in the invoice's own calendar…
  expect(dayIn(issued.due_at ?? 0, issued.utc_offset_minutes)).toBe(today);
  // …and not late on it: the engine says so, and the screen repeats it.
  expect(issued.status).toBe('issued');
  expect(issued.days_overdue).toBeNull();
  const drawer = page.getByRole('dialog', { name: issued.number ?? '' });
  await expect(drawer).toContainText(`${today} · Due today`);
  await expect(drawer).not.toContainText('Overdue');
  const arrears = await rpc<Arrears>('isp.arrears', { customer_id: seeded.zagros });
  const dinars = arrears.by_currency.find((entry) => entry.currency === 'IQD');
  // Owed, and not overdue: the 150 000 is in "not yet due".
  expect(dinars?.not_yet_due.minor).toBe(150_000);

  expect(console.messages).toEqual([]);
  await close();
});

test('the business’s calendar is set in one click, and a typed date is a day of it in any browser', async ({
  browser,
}) => {
  // An operator in Baghdad opens the settings. A fresh installation is on UTC.
  const baghdad = await pageIn(browser, 'Asia/Baghdad');
  const console = watchConsole(baghdad.page);
  await openIsp(baghdad.page, 'settings');
  const panel = baghdad.page.getByRole('region', { name: /The business.s calendar/ });
  await expect(panel).toContainText('UTC+00:00');
  await expect(panel).toContainText('not the business’s calendar');
  await panel.getByRole('button', { name: 'Use this computer’s offset (UTC+03:00)' }).click();
  await expect(baghdad.page.getByText('The business’s calendar is now UTC+03:00')).toBeVisible();
  await expect(panel).toContainText('the same as the business’s calendar');
  await expect(panel.getByRole('button', { name: /Use this computer.s offset/ })).toBeDisabled();
  expect((await rpc<IspSettings>('settings.get', { section: 'isp' })).utc_offset_minutes).toBe(180);
  expect(console.messages).toEqual([]);
  await baghdad.close();

  // Somebody else sells a plan from a browser in Auckland, thirteen hours
  // ahead of UTC, to start on the first of next month.
  const offset = 180;
  const now = Date.now();
  const thisMonth = dayIn(now, offset).slice(0, 7);
  const [year, month] = thisMonth.split('-').map(Number) as [number, number];
  const next = month === 12 ? { year: year + 1, month: 1 } : { year, month: month + 1 };
  const firstOfNext = `${next.year}-${String(next.month).padStart(2, '0')}-01`;
  const subscriber = (await rpc<CustomerPage>('isp.customers', { query: 'Subscriber 0005' }))
    .customers[0];
  expect(subscriber?.currency).toBe('IQD');

  const auckland = await pageIn(browser, 'Pacific/Auckland');
  const console2 = watchConsole(auckland.page);
  await openApp(auckland.page, `/isp/customer/${subscriber?.id}`, { backend });
  await auckland.page.getByRole('button', { name: 'Sell a plan…' }).click();
  const selling = auckland.page.getByRole('dialog', { name: 'Sell a plan' });
  await selling.getByLabel('Tariff').selectOption(seeded.homeTariff);
  // The field says whose day it is: the business's, in Baghdad.
  await expect(selling).toContainText(`(UTC+03:00), from its first moment; today is ${dayIn(now, offset)}`);
  await selling.getByLabel('Starts', { exact: true }).fill(firstOfNext);
  await selling.getByRole('button', { name: 'Sell it' }).click();
  await expect(auckland.page.getByText(/Home 10: 150\s*000\s*IQD a month/)).toBeVisible();

  // The plan starts at the business's midnight — 21:00 UTC the evening
  // before — not at Auckland's, which is ten hours earlier and inside this
  // month, and not at UTC's.
  const plans = await rpc<SubscriptionRow[]>('isp.subscriptions', { customer_id: subscriber?.id });
  expect(plans).toHaveLength(1);
  expect(plans[0]?.started_at).toBe(Date.UTC(next.year, next.month - 1, 1) - offset * 60_000);
  // And the page shows the day that was typed, to the person in Auckland too.
  await expect(auckland.page.getByRole('region', { name: 'What they were sold' })).toContainText(
    `since ${firstOfNext}`,
  );

  // So this month's run does not bill them for a month their service is not
  // in: the plan is in force for none of it.
  const preview = await rpc<RunResult>('isp.invoice.run', {
    month: thisMonth,
    customer_ids: [subscriber?.id],
    dry_run: true,
  });
  expect(preview.utc_offset_minutes).toBe(180);
  expect(preview.invoices).toEqual([]);
  expect(preview.skipped[0]?.reason).toContain('has nothing to bill for that period');
  // And next month's bills the whole of it, with no part period.
  const nextRun = await rpc<RunResult>('isp.invoice.run', {
    month: firstOfNext.slice(0, 7),
    customer_ids: [subscriber?.id],
    dry_run: true,
  });
  expect(nextRun.invoices[0]?.total.minor).toBe(150_000);

  expect(console2.messages).toEqual([]);
  await auckland.close();
});

test('a month billed from the screens is the business’s month, from a browser in any zone', async ({
  browser,
}) => {
  // The test before this one put the business on UTC+03:00. The operator is
  // still in Auckland, ten hours further east.
  const offset = (await rpc<IspSettings>('settings.get', { section: 'isp' })).utc_offset_minutes;
  expect(offset).toBe(180);
  const thisMonth = dayIn(Date.now(), offset).slice(0, 7);
  const current = monthIn(thisMonth, offset);
  const lastMonth = monthsBefore(thisMonth, 1);
  const last = monthIn(lastMonth, offset);

  // A subscriber who has been on a plan since long before last month.
  const subscriber = (await rpc<CustomerPage>('isp.customers', { query: 'Subscriber 0006' }))
    .customers[0];
  expect(subscriber?.currency).toBe('IQD');
  await rpc('isp.subscription.create', {
    customer_id: subscriber?.id,
    tariff_id: seeded.homeTariff,
    started_at: monthIn(monthsBefore(thisMonth, 6), offset).start,
  });

  const { page, close } = await pageIn(browser, 'Pacific/Auckland');
  const console = watchConsole(page);

  // From their own page: "last month" is named, and the engine works out
  // where it begins. The draft covers the business's month to the instant —
  // not UTC's, which would begin three hours into it, and not Auckland's.
  await openApp(page, `/isp/customer/${subscriber?.id}`, { backend });
  await page.getByRole('button', { name: "Draft last month's invoice" }).click();
  const drawer = page.getByRole('dialog', { name: 'Draft invoice' });
  await expect(drawer).toContainText(`${last.first} → ${last.last}`);
  const drafted = await rpc<InvoiceView[]>('isp.invoices', { customer_id: subscriber?.id });
  expect(drafted).toHaveLength(1);
  expect(drafted[0]?.period_start).toBe(last.start);
  expect(drafted[0]?.period_end).toBe(last.end);
  expect(drafted[0]?.utc_offset_minutes).toBe(180);
  // One whole period at the agreed price: nothing pro-rated at either end.
  expect(drafted[0]?.total.minor).toBe(150_000);
  expect(drafted[0]?.lines.map((line) => line.prorated)).toEqual([null]);
  await page.getByRole('button', { name: 'Close' }).click();

  // A payment dated today is dated the business's today: its first moment
  // there, which in Auckland was hours ago on another day of the week.
  const today = dayIn(Date.now(), offset);
  await page.getByRole('button', { name: 'Record a payment…' }).click();
  const paying = page.getByRole('dialog', { name: 'Record a payment' });
  await paying.getByLabel('Amount received').fill('1000');
  await paying.getByLabel('How it came in').fill('cash');
  await expect(paying).toContainText(`(UTC+03:00); today is ${today}`);
  await paying.getByLabel('When the money changed hands').fill(today);
  await paying.getByRole('button', { name: 'Record it' }).click();
  await expect(page.getByText(/1\s*000\s*IQD received, on account/)).toBeVisible();
  const paid = await rpc<{ at: number }[]>('isp.payments', { customer_id: subscriber?.id });
  expect(paid).toHaveLength(1);
  expect(paid[0]?.at).toBe(Date.parse(`${today}T00:00:00Z`) - offset * 60_000);
  await expect(page.getByRole('region', { name: 'Payments' })).toContainText(today);

  // From the run: the month on offer is the business's last month, and what
  // the engine billed is that month's days in the business's calendar.
  await openApp(page, '/isp/invoices?run=1', { backend });
  const run = page.getByRole('dialog', { name: 'Bill a period' });
  await expect(run.getByLabel('Period')).toHaveValue(lastMonth);
  await run.getByLabel('Period').selectOption(thisMonth);
  await expect(run).toContainText(
    `${current.first} → ${current.last}, days of the business’s calendar (UTC+03:00)`,
  );
  await run.getByRole('button', { name: 'Preview', exact: true }).click();
  await expect(run.getByText('Preview — nothing has been written')).toBeVisible();
  await expect(run).toContainText(`${current.first} → ${current.last} in UTC+03:00`);
  // The plan sold to start on the first of next month is in force for none
  // of this one. Sent as UTC's month, the last three hours of it would be,
  // and this subscriber would be billed a part period before their service
  // began — the defect this calendar exists to end.
  await expect(run.getByRole('row').filter({ hasText: 'Subscriber 0005' })).toContainText(
    'has nothing to bill for that period',
  );
  // And the one on a plan all month is billed the whole of it.
  await expect(run.getByRole('row').filter({ hasText: 'Subscriber 0006' })).toContainText(
    /150\s*000\s*IQD/,
  );

  expect(console.messages).toEqual([]);
  await close();
});

test('a list filtered to a month shows that month, however many invoices the month after it holds', async ({
  page,
}) => {
  const console = watchConsole(page);
  // More invoices in one month than fit on a page, and two in the month
  // before it. Months well clear of the ones the tests above billed.
  const offset = (await rpc<IspSettings>('settings.get', { section: 'isp' })).utc_offset_minutes;
  const thisMonth = dayIn(Date.now(), offset).slice(0, 7);
  const busy = monthsBefore(thisMonth, 7);
  const quiet = monthsBefore(thisMonth, 8);
  const subscribers = (await rpc<CustomerPage>('isp.customers', { query: 'Subscriber', limit: 100 }))
    .customers;
  expect(subscribers).toHaveLength(60);
  for (const customer of subscribers.slice(0, 52)) {
    await rpc('isp.invoice.draft', { customer_id: customer.id, month: busy, allow_empty: true });
  }
  for (const customer of subscribers.slice(52, 54)) {
    await rpc('isp.invoice.draft', { customer_id: customer.id, month: quiet, allow_empty: true });
  }

  await openIsp(page, 'invoices');
  const table = page.getByRole('table', { name: 'Invoices' });
  const period = page.getByLabel('Period', { exact: true });

  // The quiet month is its two invoices. The list is newest period first and
  // fifty to a page, so a query that also took in the month after it would
  // fill this page with that month's invoices and show none of these.
  await period.selectOption(quiet);
  await expect(table.getByRole('row').filter({ hasText: subscribers[52]?.name })).toBeVisible();
  await expect(table.getByRole('row').filter({ hasText: subscribers[53]?.name })).toBeVisible();
  await expect(table.locator('tbody tr')).toHaveCount(2);
  await expect(page.getByText('1–2', { exact: true })).toBeVisible();

  // The busy month is all of its own, over two pages.
  await period.selectOption(busy);
  await expect(page.getByText('1–50', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Next' }).click();
  await expect(page.getByText('51–52', { exact: true })).toBeVisible();

  // An invoice keeps the month it was written for when the calendar moves.
  // The hand-made draft an earlier test left is for the month two before
  // the one the seed billed, and was written while the business was on UTC,
  // so it ends three hours into the next month of UTC+03:00: the engine,
  // which compares instants, sends it with that month's — and it is not
  // listed there, and the count says one was left out and why.
  await period.selectOption(seeded.dates.earlier.name);
  await expect(table.getByRole('row').filter({ hasText: 'Zagros Net' }).first()).toBeVisible();
  await expect(table.getByRole('row').filter({ hasText: 'Subscriber 0002' })).toHaveCount(0);
  await expect(page.getByText(/1 not shown: its period is another month/)).toBeVisible();
  await period.selectOption(seeded.dates.earliest.name);
  await expect(table.getByRole('row').filter({ hasText: 'Subscriber 0002' })).toBeVisible();

  expect(console.messages).toEqual([]);
});

test('a build that answers no isp method shows no ISP navigation at all', async ({ page }) => {
  const console = watchConsole(page);
  // The free edition is not assembled with the feature crate, so its engine
  // registers no `isp.*` method — which is what the interface keys off. Here
  // that answer is taken out at the transport, so the real engine stands in
  // for a free-edition one; that the crate is absent from the binary is the
  // backend's own guarantee (`docs/dev/spec-editions.md`).
  await page.route(`${backend.url}/api/rpc`, async (route) => {
    const body = route.request().postDataJSON() as { method?: string };
    if (body?.method !== 'app.methods') return route.fallback();
    const response = await route.fetch();
    const envelope = (await response.json()) as { ok: boolean; result: string[] };
    return route.fulfill({
      json: {
        ...envelope,
        result: envelope.result.filter((method) => !method.startsWith('isp.')),
      },
    });
  });

  await openApp(page, '/', { backend });
  const rail = page.getByRole('navigation', { name: 'Primary' });
  await expect(rail.getByRole('link', { name: 'Devices' })).toBeVisible();
  await expect(rail.getByRole('link', { name: 'ISP', exact: true })).toHaveCount(0);

  // Nor in the command palette.
  await page.keyboard.press('ControlOrMeta+k');
  const palette = page.getByRole('dialog', { name: 'Command palette' });
  await palette.getByRole('combobox').fill('ISP');
  await expect(palette.getByRole('option', { name: 'Go to ISP' })).toHaveCount(0);
  await page.keyboard.press('Escape');

  // And the screen itself, if its address is typed in, says so in a sentence
  // instead of failing.
  await openApp(page, '/isp', { backend });
  await expect(page.getByText('The ISP edition is not in this build')).toBeVisible();

  expect(console.messages).toEqual([]);
});
