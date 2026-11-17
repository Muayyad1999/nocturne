/**
 * `pnpm shots` — the ISP edition's screens, photographed in both themes.
 *
 * The state is the one `isp.spec.ts` builds (`support/isp.ts`), and for the
 * same reason: a screenshot of an empty ledger proves nothing. In the picture
 * there is a customer billed in dinars and one billed in dollars, an overdue
 * invoice and one not yet due, a draft with a pro-rated line, a payment taken
 * in dinars against a dollar invoice, a suspended account, a customer whose
 * interface nobody has ever collected, and a link whose sold megabits are set
 * against what it can carry.
 *
 * These are written to `tests/shots/isp/<night|day>/` and are not compared
 * against anything: they exist to be looked at before a change ships.
 */

import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import { openApp, pageLoaded, settle, type Theme } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { seedIsp, seedManyCustomers, type Seeded } from './support/isp';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));
const SHOTS = resolve(fileURLToPath(new URL('./shots/isp', import.meta.url)));
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };
const THEMES: readonly Theme[] = ['dark', 'light'];

test.describe.configure({ mode: 'serial', timeout: 900_000 });

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
  uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
  await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
    cwd: UI_DIR,
    timeout: 300_000,
  });
  backend = await startBackend({ uiDir, features: ['isp'] });
  const methods = await rpc<string[]>('app.methods');
  if (!methods.includes('isp.customers')) {
    throw new Error(
      'this engine has no `isp.*` methods: build it with `cargo build -p nocturne-server ' +
        '--features isp` and point NOCTURNE_SERVER_BIN at that binary',
    );
  }
  seeded = await seedIsp(rpc);
  // Twenty more, so the list looks like a list and the seeded three are
  // still in the picture: fifty to a page, ordered by name in the engine.
  await seedManyCustomers(rpc, 20);
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

async function shoot(page: Page, theme: Theme, name: string): Promise<void> {
  const directory = join(SHOTS, THEME_FOLDER[theme]);
  await mkdir(directory, { recursive: true });
  await settle(page);
  await page.screenshot({ path: join(directory, `${name}.png`), animations: 'disabled' });
}

async function openIsp(page: Page, theme: Theme, tab: string, dock = false): Promise<void> {
  await openApp(page, `/isp/${tab}`, { theme, backend, dock });
  await expect(page.getByRole('tab', { name: 'Customers' })).toBeVisible({ timeout: 60_000 });
}

for (const theme of THEMES) {
  test(`isp screens — ${theme}`, async ({ page }) => {
    test.setTimeout(600_000);

    // ---------------------------------------------------------- the customers
    await openIsp(page, theme, 'customers');
    await expect(page.getByRole('table', { name: 'Customers' })).toContainText('Zagros Net');
    await shoot(page, theme, 'customers');

    // One customer: their plans, their balance, their invoices, their
    // payments, and what they are using against what they pay for.
    await openApp(page, `/isp/customer/${seeded.tigris}`, { theme, backend });
    await expect(page.getByRole('heading', { level: 1, name: 'Tigris Trading' })).toBeVisible({
      timeout: 60_000,
    });
    await shoot(page, theme, 'customer');

    // ----------------------------------------------------------- the invoices
    await openIsp(page, theme, 'invoices');
    await expect(page.getByRole('table', { name: 'Invoices' })).toContainText('2026-0001');
    await shoot(page, theme, 'invoices');

    // An issued invoice: frozen, part paid, with the rate the dinar payment
    // was taken at.
    await page.getByRole('table', { name: 'Invoices' }).getByRole('button', { name: '2026-0002' }).click();
    await expect(page.getByRole('dialog', { name: '2026-0002' })).toBeVisible();
    await shoot(page, theme, 'invoice-issued');
    await page.getByRole('button', { name: 'Close' }).click();

    // A draft: editable, with the arithmetic of its part period on the line.
    await page.getByLabel('Invoice state').selectOption('draft');
    await page
      .getByRole('table', { name: 'Invoices' })
      .getByRole('row')
      .filter({ hasText: 'Dohuk Dates' })
      .getByRole('button', { name: 'not issued' })
      .click();
    await expect(page.getByRole('dialog', { name: 'Draft invoice' })).toBeVisible();
    await shoot(page, theme, 'invoice-draft');
    await page.getByRole('button', { name: 'Close' }).click();

    // The billing run, previewed: the thing that must be unmistakably a
    // preview before anything is issued.
    await page.getByLabel('Invoice state').selectOption('');
    await page.getByRole('button', { name: 'Bill a period…' }).click();
    const run = page.getByRole('dialog', { name: 'Bill a period' });
    // November: October is already invoiced for both of them, so a preview
    // of it would be a screen full of "already invoiced" reasons.
    await run.getByLabel('Period').selectOption('2026-11');
    await run.getByRole('button', { name: 'Preview', exact: true }).click();
    await expect(run.getByText('Preview — nothing has been written')).toBeVisible();
    await shoot(page, theme, 'run-preview');
    await page.keyboard.press('Escape');

    // Recording a payment, in a currency the invoice is not in.
    await page.getByRole('button', { name: 'Record a payment…' }).click();
    const payment = page.getByRole('dialog', { name: 'Record a payment' });
    await payment.getByRole('combobox', { name: 'Customer' }).click();
    await page.getByRole('option', { name: /Tigris Trading/ }).click();
    await payment.getByLabel('Against').selectOption({ index: 1 });
    await payment.getByLabel('Currency').selectOption('IQD');
    await payment.getByLabel('Amount received').fill('33000');
    await payment.getByLabel('How it came in').fill('cash');
    await expect(payment.getByText(/Rate it was taken at/)).toBeVisible();
    await shoot(page, theme, 'payment-conversion');
    await page.keyboard.press('Escape');

    // ----------------------------------------------------------- the payments
    await openIsp(page, theme, 'payments');
    await expect(page.getByRole('table', { name: 'Payments' })).toContainText('R-1002');
    await shoot(page, theme, 'payments');

    // ------------------------------------------------------------ the arrears
    await openIsp(page, theme, 'arrears');
    await expect(page.getByRole('region', { name: 'Owed in IQD' })).toBeVisible();
    await shoot(page, theme, 'arrears');

    // ------------------------------------------------------------ the reports
    await openIsp(page, theme, 'reports');
    await expect(page.getByRole('region', { name: 'Revenue' })).toBeVisible();
    await shoot(page, theme, 'reports');
    // The usage report's coverage, and the oversubscription table, scrolled to.
    await page.getByRole('region', { name: 'Used against sold' }).scrollIntoViewIfNeeded();
    await shoot(page, theme, 'reports-usage');
    await page.getByRole('region', { name: 'Margin per megabit' }).scrollIntoViewIfNeeded();
    await shoot(page, theme, 'reports-margin');

    // ------------------------------------------------- the tariffs and links
    await openIsp(page, theme, 'plans');
    await expect(page.getByRole('table', { name: 'Tariffs' })).toContainText('Home 10');
    await shoot(page, theme, 'plans');

    // ----------------------------------------- narrow, with the dock open
    await page.setViewportSize({ width: 1280, height: 720 });
    await openIsp(page, theme, 'customers', true);
    await pageLoaded(page);
    await shoot(page, theme, 'customers-narrow-dock');
    await openApp(page, `/isp/customer/${seeded.zagros}`, { theme, backend, dock: true });
    await expect(page.getByRole('heading', { level: 1, name: 'Zagros Net' })).toBeVisible({
      timeout: 60_000,
    });
    await shoot(page, theme, 'customer-narrow-dock');
    await page.setViewportSize({ width: 1440, height: 900 });
  });
}
