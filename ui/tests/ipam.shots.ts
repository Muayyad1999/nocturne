/**
 * `pnpm shots` — the IPAM screen, photographed in both themes.
 *
 * The state is the one `ipam.spec.ts` builds, and for the same reason: a
 * screenshot of an empty plan proves nothing. The demo campus is collected
 * from, so the lab's blocks are real discoveries; the production plan is
 * documented by hand; and the seed deliberately contains an address that is
 * documented and seen, one documented and never seen in a block nobody has
 * collected from, a record that contradicts the network, a gateway nothing
 * has, a record in no block at all, a VLAN the switches call something else
 * and an IPv6 /64 whose utilisation is not a percentage.
 *
 * Both sizes are taken (1440×900 and 1280×720) and one with the assistant
 * dock open, because the tree and the grid have to survive 900px of content.
 */

import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import type { PrefixRow } from '../src/api/types/ipam';
import { openApp, settle, type Theme } from './support/app';
import { startBackend, type Backend } from './support/backend';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));
const SHOTS = resolve(fileURLToPath(new URL('./shots/ipam', import.meta.url)));
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };
const THEMES: readonly Theme[] = ['dark', 'light'];

test.describe.configure({ mode: 'serial', timeout: 900_000 });

let backend: Backend;
let uiDir: string;
let labStaffCidr = '10.10.10.0/24';

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

async function waitForJob(id: string): Promise<void> {
  for (let attempt = 0; attempt < 120; attempt += 1) {
    const job = await rpc<{ state: string }>('jobs.get', { id });
    if (job.state !== 'running') return;
    await new Promise((done) => setTimeout(done, 1_000));
  }
  throw new Error(`job ${id} did not finish`);
}

/**
 * Wait for the feature's own debounced sync, rather than asking for one.
 *
 * `ipam` re-reads the collected data 1.5 s after `collect.updated` settles.
 * Calling `ipam.sync` as well races that watcher: both runs read "no block
 * covers this address" and both insert one, which leaves two `discovered`
 * blocks for the same CIDR (see `ui/src/pages/ipam/NOTES.md`). So the seed
 * waits for the automatic one and only asks if it never comes.
 */
async function waitForSync(): Promise<void> {
  for (let attempt = 0; attempt < 30; attempt += 1) {
    const summary = await rpc<{ prefixes: number }>('ipam.summary');
    if (summary.prefixes > 0) return;
    await new Promise((done) => setTimeout(done, 1_000));
  }
  await rpc('ipam.sync', {});
}

async function seed(): Promise<void> {
  const lab = await rpc<{ id: string }>('labs.create', { name: 'Campus', example: 'demo' });
  await rpc('labs.start', { id: lab.id });
  const devices = await rpc<{ id: string; name: string }[]>('devices.list');
  const started = await rpc<{ job_id: string }>('collect.run', {
    device_ids: devices.map((device) => device.id),
  });
  await waitForJob(started.job_id);
  await waitForSync();

  await rpc('ipam.prefix.save', {
    cidr: '10.20.0.0/16',
    name: 'HQ campus',
    site: 'HQ',
    role: 'lan',
    description: 'Everything at headquarters',
  });
  const staff = await rpc<{ prefix: PrefixRow }>('ipam.prefix.save', {
    cidr: '10.20.1.0/24',
    name: 'Staff wired',
    site: 'HQ',
    role: 'lan',
    vlan_id: 10,
    gateway: '10.20.1.1',
    dhcp: { start: '10.20.1.100', end: '10.20.1.200' },
    tags: ['users'],
  });
  await rpc('ipam.prefix.save', {
    cidr: '10.20.2.0/24',
    name: 'Voice',
    site: 'HQ',
    role: 'voice',
    vlan_id: 20,
  });
  await rpc('ipam.prefix.save', {
    cidr: '2001:db8:10::/64',
    name: 'HQ staff IPv6',
    site: 'HQ',
    role: 'lan',
  });

  await rpc('ipam.address.save', {
    ip: '10.20.1.50',
    prefix_id: staff.prefix.id,
    status: 'active',
    hostname: 'printer-1',
    description: 'Third floor printer',
  });
  await rpc('ipam.address.reserve', {
    prefix_id: staff.prefix.id,
    hostname: 'spare-gw',
    description: 'kept for the second router',
  });
  await rpc('ipam.address.save', {
    ip: '192.0.2.77',
    status: 'active',
    hostname: 'orphan-host',
    description: 'documented, inside no block on record',
  });

  const rows = await rpc<PrefixRow[]>('ipam.prefixes', {});
  const labStaff = rows.find((row) => row.cidr === labStaffCidr);
  if (!labStaff) throw new Error('the demo lab reported no 10.10.10.0/24');
  await rpc('ipam.prefix.save', {
    id: labStaff.id,
    name: 'Staff (lab)',
    vlan_id: 10,
    role: 'lan',
    gateway: '10.10.10.254',
  });
  const as1 = devices.find((device) => device.name.endsWith('/AS1'));
  if (as1) {
    await rpc('ipam.address.save', {
      ip: '10.10.10.2',
      prefix_id: labStaff.id,
      status: 'active',
      hostname: 'gw-staff',
      device_id: as1.id,
      interface: 'Vlan99',
      mac: 'aa:bb:cc:dd:ee:ff',
    });
  }
  await rpc('ipam.vlan.save', {
    vid: 10,
    name: 'Staff wired',
    site: `lab:${lab.id}`,
    description: 'documented name',
    prefix_ids: [labStaff.id],
  });
}

test.beforeAll(async () => {
  test.setTimeout(900_000);
  uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
  await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
    cwd: UI_DIR,
    timeout: 300_000,
  });
  backend = await startBackend({ uiDir });
  await seed();
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

async function openIpam(page: Page, theme: Theme, tab?: string, dock = false): Promise<void> {
  await openApp(page, tab ? `/ipam?tab=${tab}` : '/ipam', { theme, backend, dock });
  await expect(page.getByRole('tab', { name: 'Blocks' })).toBeVisible({ timeout: 60_000 });
}

function block(page: Page, cidr: string) {
  return page.getByRole('treeitem').filter({ hasText: cidr }).first();
}

for (const theme of THEMES) {
  test(`ipam screens — ${theme}`, async ({ page }) => {
    test.setTimeout(600_000);

    // --------------------------------------------------------- the whole tree
    await openIpam(page, theme);
    await expect(page.getByRole('heading', { name: 'Lab: Campus' })).toBeVisible();
    await shoot(page, theme, 'tree');

    // A documented block with a DHCP range, a gateway and records in it.
    await block(page, '10.20.1.0/24').click();
    await expect(page.getByRole('heading', { name: '10.20.1.0/24' })).toBeVisible();
    await shoot(page, theme, 'documented-block');

    // A discovered block: its provenance, and the grid with all four states.
    await block(page, labStaffCidr).click();
    await expect(page.getByRole('heading', { name: labStaffCidr })).toBeVisible();
    await shoot(page, theme, 'grid');

    // One address, with the record and the evidence side by side.
    await page.locator('[role="gridcell"][data-ip="10.10.10.2"]').click();
    await expect(page.getByRole('dialog', { name: '10.10.10.2' })).toBeVisible();
    await shoot(page, theme, 'address-disagreement');
    await page.keyboard.press('Escape');

    // A block nobody has documented at all.
    await block(page, '10.10.30.0/24').click();
    await expect(page.getByRole('heading', { name: '10.10.30.0/24' })).toBeVisible();
    await shoot(page, theme, 'discovered-block');

    // A block whose fullness is not a percentage.
    await block(page, '2001:db8:10::/64').click();
    await expect(page.getByRole('heading', { name: '2001:db8:10::/64' })).toBeVisible();
    await shoot(page, theme, 'ipv6-block');

    // ------------------------------------------------------------- free space
    await block(page, labStaffCidr).click();
    await page.getByRole('button', { name: 'Free space' }).first().click();
    await expect(
      page.getByRole('dialog', { name: `Free space in ${labStaffCidr}` }),
    ).toBeVisible();
    await expect(page.getByText('is not known to be free', { exact: false })).toBeVisible({
      timeout: 30_000,
    });
    await shoot(page, theme, 'free-space');
    await page.keyboard.press('Escape');

    // ----------------------------------------------------------------- splits
    await block(page, '10.20.0.0/16').click();
    await page.getByRole('button', { name: 'Split…' }).click();
    const split = page.getByRole('dialog', { name: 'Split 10.20.0.0/16' });
    // The design system draws its own radio dot over a transparent input, so
    // Playwright's `check()` can never deliver the click: press the label, as
    // a person does. "Host counts" is also part of the radio's own name, so
    // the field is asked for by role.
    await split.getByText('VLSM: a list of host counts').click({ timeout: 15_000 });
    await split
      .getByRole('textbox', { name: 'Host counts' })
      .fill('staff: 500, voice: 100, 50', { timeout: 15_000 });
    await split.getByRole('button', { name: 'Preview' }).click({ timeout: 15_000 });
    await expect(split.getByText('This is a preview', { exact: false })).toBeVisible({
      timeout: 20_000,
    });
    await shoot(page, theme, 'split-preview');
    await page.keyboard.press('Escape');

    // --------------------------------------------------------------- search
    await page.getByRole('searchbox', { name: /Search blocks/ }).fill('10.10.10.2');
    const found = page.getByRole('region', { name: 'Addresses that match' });
    await expect(found.getByText('Documented and seen')).toBeVisible({ timeout: 20_000 });
    await shoot(page, theme, 'search');
    await page.getByRole('searchbox', { name: /Search blocks/ }).fill('');

    // ---------------------------------------------------------------- VLANs
    await openIpam(page, theme, 'vlans');
    await expect(page.getByRole('table', { name: 'VLANs' })).toBeVisible({ timeout: 30_000 });
    await shoot(page, theme, 'vlans');

    // ------------------------------------------------------------ conflicts
    await openIpam(page, theme, 'conflicts');
    await expect(page.getByText('reported, never resolved', { exact: false })).toBeVisible({
      timeout: 30_000,
    });
    await shoot(page, theme, 'conflicts');

    // ---------------------------------------------------------- reconcile
    await openIpam(page, theme, 'reconcile');
    await expect(page.getByText('What was looked at')).toBeVisible({ timeout: 30_000 });
    await shoot(page, theme, 'reconcile');

    // The sections that found nothing start closed; opening one shows what
    // finding nothing means.
    await page.getByRole('button', { name: /An interface mask that is not the block/ }).click();
    await shoot(page, theme, 'reconcile-empty-section');

    await page.getByRole('button', { name: 'Per device' }).click();
    await shoot(page, theme, 'reconcile-coverage');

    // The one write that turns evidence into records, previewed.
    await page.getByRole('button', { name: /Document what the network shows/ }).click();
    const learn = page.getByRole('dialog', { name: 'Document what the network shows' });
    await expect(learn.getByText('Would create', { exact: false })).toBeVisible({
      timeout: 30_000,
    });
    await shoot(page, theme, 'learn-preview');
    await page.keyboard.press('Escape');

    // --------------------------------------------------- import and export
    await openIpam(page, theme);
    await page.getByRole('button', { name: 'Import / export' }).click();
    const transfer = page.getByRole('dialog', { name: 'Import and export' });
    await transfer
      .getByLabel('CSV')
      .fill('cidr,name,site,role\n10.60.0.0/24,Voice,HQ,voice\nnonsense,Bad,HQ,');
    await transfer.getByRole('button', { name: 'Preview' }).click();
    await expect(transfer.getByText('1 invalid')).toBeVisible({ timeout: 20_000 });
    await shoot(page, theme, 'import-preview');
    await transfer.getByRole('tab', { name: 'Export' }).click();
    await expect(transfer.getByText('cidr,name,description', { exact: false })).toBeVisible({
      timeout: 20_000,
    });
    await shoot(page, theme, 'export');
    await page.keyboard.press('Escape');

    // --------------------------------------------------------- the small window
    await page.setViewportSize({ width: 1280, height: 720 });
    await block(page, labStaffCidr).click();
    await shoot(page, theme, 'grid-1280');
    await page.setViewportSize({ width: 1440, height: 900 });

    // ------------------------------------------------- with the assistant dock
    await openIpam(page, theme, undefined, true);
    await block(page, labStaffCidr).click();
    await expect(page.getByRole('heading', { name: labStaffCidr })).toBeVisible();
    await shoot(page, theme, 'grid-dock');

    await page.setViewportSize({ width: 1280, height: 720 });
    await shoot(page, theme, 'grid-dock-1280');
    await page.setViewportSize({ width: 1440, height: 900 });
  });
}
