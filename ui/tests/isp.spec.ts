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
 *   replaces it.
 *
 * Every test asserts the browser console stayed clean.
 */

import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import type { InvoiceView } from '../src/api/types/isp';
import { openApp, pageLoaded, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { seedIsp, seedManyCustomers, type Seeded } from './support/isp';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let uiDir: string;
let seeded: Seeded;

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
  await expect(usd).toContainText('Nothing due yet');
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

  // November: October is already invoiced for both of them, and billing a
  // period twice is the mistake the engine refuses — which the preview says,
  // per customer, rather than silently drafting nothing.
  await dialog.getByLabel('Period').selectOption('2026-11');
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
  await openIsp(page, 'invoices');

  const table = page.getByRole('table', { name: 'Invoices' });
  await expect(table.getByRole('row').filter({ hasText: '2026-0001' })).toContainText('Overdue');

  // The dollar invoice: issued, part paid by a payment taken in dinars.
  await table.getByRole('button', { name: '2026-0002' }).click();
  const drawer = page.getByRole('dialog', { name: '2026-0002' });
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
  // A part period carries its arithmetic, so the figure can be rechecked.
  await expect(draftDrawer).toContainText(/16 of the period’s 31 days/);
  await page.getByRole('button', { name: 'Close' }).click();

  // Now reissue the invoice that has no payment: the original is voided, keeps
  // its number, and a draft replacement is made.
  const clean = await rpc<{ id: string }>('isp.invoice.issue', {
    id: (
      await rpc<{ invoice: { id: string } }>('isp.invoice.draft', {
        customer_id: seeded.zagros,
        period_start: Date.UTC(2026, 8, 1),
        period_end: Date.UTC(2026, 9, 1),
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
  await expect(page.getByRole('region', { name: 'Invoices' })).toContainText('2026-0002');
  await expect(page.getByRole('region', { name: 'Payments' })).toContainText(/33\s*000\s*IQD/);

  // Marking an account says what it does, which is to record a decision.
  await page.getByRole('button', { name: 'Mark suspended' }).click();
  const confirm = page.getByRole('dialog', { name: /Mark Tigris Trading suspended\?/ });
  await expect(confirm).toContainText('records a decision about the account');
  await expect(confirm).toContainText('somebody changes the router');
  await confirm.getByRole('button', { name: 'Cancel' }).click();

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
