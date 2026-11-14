/**
 * The IPAM screen, against a real engine with a real network behind it.
 *
 * Nothing here is mocked. A `nocturne-server` runs on a fresh data directory,
 * the UI is the production bundle it serves, and the addresses come from the
 * demo campus lab: sixteen simulated devices whose interface addresses, ARP
 * tables and VLANs the real collectors read. On top of that the seed builds
 * the states this screen exists to tell apart:
 *
 * * blocks nobody documented (`source: discovered`), in a lab namespace of
 *   their own, beside a documented production plan;
 * * an address documented **and** seen; one documented and never seen, in a
 *   block no device could have reported (so "nobody looked"); one seen and
 *   never documented;
 * * a record that contradicts the network (wrong device, wrong MAC);
 * * a documented gateway no interface has, and a record outside every block —
 *   the two conflicts the engine reports and refuses to resolve;
 * * a VLAN whose documented name is not what the switches call it;
 * * an IPv6 /64, whose utilisation is deliberately not a percentage.
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
import type {
  Address,
  Conflict,
  PrefixDetail,
  PrefixRow,
  Reconciliation,
  Summary,
  VlanRow,
} from '../src/api/types/ipam';
import { openApp, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let uiDir: string;
/** The demo lab's id, which is also the namespace its blocks live in. */
let labId = '';
/** The ids the seed created, for the assertions that compare with RPC. */
const seeded: Record<string, string> = {};

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

/** The campus, collected from, plus a plan and the disagreements with it. */
async function seedCampus(): Promise<void> {
  const lab = await rpc<{ id: string }>('labs.create', { name: 'Campus', example: 'demo' });
  labId = lab.id;
  await rpc('labs.start', { id: lab.id });

  const devices = await rpc<{ id: string; name: string }[]>('devices.list');
  const started = await rpc<{ job_id: string }>('collect.run', {
    device_ids: devices.map((device) => device.id),
  });
  await waitForJob(started.job_id);
  await waitForSync();

  // The plan: a documented estate with a block nothing has ever been seen in,
  // and an IPv6 /64 whose fullness cannot be a percentage.
  const hq = await rpc<{ prefix: PrefixRow }>('ipam.prefix.save', {
    cidr: '10.20.0.0/16',
    name: 'HQ campus',
    site: 'HQ',
    role: 'lan',
    description: 'Everything at headquarters',
  });
  seeded.hq = hq.prefix.id;
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
  seeded.staff = staff.prefix.id;
  const six = await rpc<{ prefix: PrefixRow }>('ipam.prefix.save', {
    cidr: '2001:db8:10::/64',
    name: 'HQ staff IPv6',
    site: 'HQ',
    role: 'lan',
  });
  seeded.six = six.prefix.id;

  // Records nothing has shown, in a block no device could have reported.
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
  // A record in no block at all: `outside_any_prefix`.
  await rpc('ipam.address.save', {
    ip: '192.0.2.77',
    status: 'active',
    hostname: 'orphan-host',
    description: 'documented, inside no block on record',
  });

  // Adopt the lab's staff /24 and document a gateway nothing has:
  // `gateway_mismatch`, which the engine reports and will not resolve.
  const labPrefixes = await rpc<PrefixRow[]>('ipam.prefixes', {});
  const labStaff = labPrefixes.find((row) => row.cidr === '10.10.10.0/24');
  if (!labStaff) throw new Error('the demo lab reported no 10.10.10.0/24');
  seeded.labStaff = labStaff.id;
  await rpc('ipam.prefix.save', {
    id: labStaff.id,
    name: 'Staff (lab)',
    vlan_id: 10,
    role: 'lan',
    gateway: '10.10.10.254',
  });

  // A record that contradicts the network: the wrong device and the wrong MAC
  // for an address a distribution switch really has.
  const as1 = (await rpc<{ id: string; name: string }[]>('devices.list')).find((device) =>
    device.name.endsWith('/AS1'),
  );
  if (!as1) throw new Error('the demo lab has no AS1');
  seeded.as1 = as1.id;
  await rpc('ipam.address.save', {
    ip: '10.10.10.2',
    prefix_id: labStaff.id,
    status: 'active',
    hostname: 'gw-staff',
    device_id: as1.id,
    interface: 'Vlan99',
    mac: 'aa:bb:cc:dd:ee:ff',
  });

  // A VLAN documented under a name the switches do not use.
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
  // Built outside `ui/dist`: the server crate embeds that directory, so
  // writing to it would make cargo rebuild the whole binary first.
  uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
  await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
    cwd: UI_DIR,
    timeout: 300_000,
  });
  backend = await startBackend({ uiDir });
  await seedCampus();
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

// ------------------------------------------------------------------ helpers

async function openIpam(page: Page, tab?: string): Promise<void> {
  await openApp(page, tab ? `/ipam?tab=${tab}` : '/ipam', { backend });
  await expect(page.getByRole('tab', { name: 'Blocks' })).toBeVisible({ timeout: 30_000 });
}

function block(page: Page, cidr: string) {
  return page.getByRole('treeitem').filter({ hasText: cidr }).first();
}

async function selectBlock(page: Page, cidr: string) {
  await block(page, cidr).click();
  await expect(page.getByRole('heading', { name: cidr })).toBeVisible({ timeout: 15_000 });
}

/** The state the grid paints each address with, by address. */
async function cellStates(page: Page): Promise<Record<string, string>> {
  return page.evaluate(() =>
    Object.fromEntries(
      [...document.querySelectorAll('[role="gridcell"][data-ip]')].map((element) => [
        element.getAttribute('data-ip') ?? '',
        element.getAttribute('data-state') ?? '',
      ]),
    ),
  );
}

// -------------------------------------------------------------------- tests

test.describe('the blocks', () => {
  test('a lab’s blocks are a root of their own, and a discovered block says nobody documented it', async ({
    page,
  }) => {
    const console_ = watchConsole(page);
    await openIpam(page);

    // Production first, the lab in a labelled root of its own.
    await expect(page.getByRole('heading', { name: 'Documented network' })).toBeVisible();
    const labRoot = page.getByRole('heading', { name: 'Lab: Campus' });
    await expect(labRoot).toBeVisible();
    await expect(
      page.getByRole('tree', { name: 'Prefixes in Lab: Campus' }).getByText('LAB').first(),
    ).toBeVisible();

    // Every block the engine answers with is drawn, and the two kinds of
    // standing are different words, not only different colours.
    const rows = await rpc<PrefixRow[]>('ipam.prefixes', {});
    for (const row of rows) await expect(block(page, row.cidr)).toBeVisible();
    const discovered = rows.filter((row) => row.source === 'discovered');
    expect(discovered.length).toBeGreaterThan(0);
    const firstDiscovered = discovered[0];
    if (!firstDiscovered) throw new Error('no discovered block in the seed');
    await expect(block(page, firstDiscovered.cidr).getByText('Discovered')).toBeVisible();
    await expect(block(page, '10.20.1.0/24').getByText('Documented')).toBeVisible();

    // And the provenance of a discovered block is on screen, not just a badge.
    await selectBlock(page, firstDiscovered.cidr);
    await expect(
      page.getByText('This block is not in anybody’s plan. It exists because an address was seen inside it.'),
    ).toBeVisible();

    expect(console_.messages).toEqual([]);
  });

  test('a block whose fullness cannot be a percentage says so instead of showing 0 %', async ({
    page,
  }) => {
    const console_ = watchConsole(page);
    await openIpam(page);
    await selectBlock(page, '2001:db8:10::/64');

    const detail = await rpc<PrefixDetail>('ipam.prefix.get', { id: seeded.six });
    expect(detail.prefix.util_known).toBe(false);
    // The exact figure the backend sends as text, not the JSON number that
    // has already lost precision.
    await expect(page.getByText(`known to be in use, of ${detail.prefix.usable_text} addresses`)).toBeVisible();
    await expect(
      page.getByText('The block is too large for a percentage to mean anything', { exact: false }),
    ).toBeVisible();
    // A /64 is not listed address by address, and the screen says which it is.
    await expect(page.getByText(detail.note)).toBeVisible();

    expect(console_.messages).toEqual([]);
  });
});

test.describe('the address grid', () => {
  test('documented, seen, both and neither are four distinguishable states', async ({ page }) => {
    const console_ = watchConsole(page);
    await openIpam(page);
    await selectBlock(page, '10.10.10.0/24');

    const detail = await rpc<PrefixDetail>('ipam.prefix.get', { id: seeded.labStaff });
    const states = await cellStates(page);
    expect(Object.keys(states)).toHaveLength(detail.addresses.length);

    const expected = (address: Address) =>
      address.status === 'conflict'
        ? 'conflict'
        : address.documented && address.observed
          ? 'both'
          : address.documented
            ? 'documented'
            : address.observed
              ? 'observed'
              : 'unknown';
    for (const address of detail.addresses) {
      expect(states[address.ip], address.ip).toBe(expected(address));
    }
    // All four really occur in this block, so the four fills are all on screen.
    const seen = new Set(Object.values(states));
    expect([...seen].sort()).toEqual(expect.arrayContaining(['both', 'observed', 'unknown']));

    // Four fills that are really different colours, painted by the browser.
    const fills = await page.evaluate(() => {
      const of = (state: string) => {
        const cell = document.querySelector(`[role="gridcell"][data-state="${state}"]`);
        return cell ? window.getComputedStyle(cell).backgroundColor : null;
      };
      return { both: of('both'), observed: of('observed'), unknown: of('unknown') };
    });
    expect(new Set(Object.values(fills)).size).toBe(3);

    expect(console_.messages).toEqual([]);
  });

  test('“free” is never offered as availability: the engine’s own note and its coverage are beside the grid', async ({
    page,
  }) => {
    const console_ = watchConsole(page);
    await openIpam(page);
    await selectBlock(page, '10.10.10.0/24');

    const detail = await rpc<PrefixDetail>('ipam.prefix.get', { id: seeded.labStaff });
    // Quoted, not paraphrased.
    await expect(page.getByText(detail.note)).toBeVisible();
    await expect(page.getByText('No record, nothing seen').first()).toBeVisible();
    await expect(
      page.getByText(`${detail.coverage.length} devices could have seen addresses in this block`),
    ).toBeVisible();

    // Nothing on this screen calls an address free or available.
    const words = await page.evaluate(() => document.body.innerText);
    expect(words).not.toMatch(/\bavailable\b/i);

    expect(console_.messages).toEqual([]);
  });

  test('an address the record and the network disagree about shows both sides and the evidence', async ({
    page,
  }) => {
    const console_ = watchConsole(page);
    await openIpam(page);
    await selectBlock(page, '10.10.10.0/24');

    await page.locator('[role="gridcell"][data-ip="10.10.10.2"]').click();
    const drawer = page.getByRole('dialog', { name: '10.10.10.2' });
    await expect(drawer.getByText('The record and the network disagree')).toBeVisible();

    const detail = await rpc<{ address: Address }>('ipam.address.get', {
      ip: '10.10.10.2',
      scope: `lab:${labId}`,
    });
    expect(detail.address.disagreements.length).toBeGreaterThan(0);
    // Both sides, with the device *named*: the backend puts ids on both sides
    // of a `device` disagreement.
    await expect(drawer.getByText('written').first()).toBeVisible();
    await expect(drawer.getByText('on the network').first()).toBeVisible();
    await expect(drawer.getByText('Campus/AS1').first()).toBeVisible();
    await expect(drawer.getByText('Campus/DS1').first()).toBeVisible();
    expect(await drawer.innerText()).not.toContain(seeded.as1 ?? 'no-id');
    // The record is one half; the sightings are the other, each timed.
    await expect(drawer.getByText('What a person wrote down')).toBeVisible();
    await expect(drawer.getByText('What the devices said')).toBeVisible();
    await expect(drawer.getByText('table read', { exact: false }).first()).toBeVisible();
    // And nothing offers to resolve it.
    await expect(
      drawer.getByText('a record is left exactly as it was written', { exact: false }),
    ).toBeVisible();

    expect(console_.messages).toEqual([]);
  });
});

test.describe('next free', () => {
  test('a suggested address carries its basis, with no raw epoch in it', async ({ page }) => {
    const console_ = watchConsole(page);
    await openIpam(page);
    await selectBlock(page, '10.10.10.0/24');

    await page.getByRole('button', { name: 'Free space' }).first().click();
    const dialog = page.getByRole('dialog', { name: 'Free space in 10.10.10.0/24' });
    await expect(dialog.getByText('is not known to be free', { exact: false })).toBeVisible({
      timeout: 20_000,
    });
    // The engine's sentence embeds an epoch in milliseconds; a person cannot
    // read that, so the screen rewrites the stamp and keeps the claim.
    expect(await dialog.innerText()).not.toMatch(/\b1[0-9]{12}\b/);
    // What it skipped, and why.
    await expect(dialog.getByText('network address')).toBeVisible();
    await expect(dialog.getByText('configured on an interface')).toBeVisible();
    // The next free child block is a suggestion, and says nothing was created.
    await expect(dialog.getByText('nothing was created', { exact: false })).toBeVisible();

    expect(console_.messages).toEqual([]);
  });
});

test.describe('reconciliation', () => {
  test('leads with coverage, and tells “nobody looked” from “it is not there”', async ({ page }) => {
    const console_ = watchConsole(page);
    await openIpam(page, 'reconcile');

    const report = await rpc<Reconciliation>('ipam.reconcile', {});
    await expect(page.getByText('What was looked at')).toBeVisible({ timeout: 30_000 });
    await expect(
      page.getByText(
        `${report.coverage.devices_with_addresses} of ${report.coverage.devices_total} devices reported addresses`,
        { exact: false },
      ),
    ).toBeVisible();

    // The documented address in a block no device could have reported.
    const printer = report.unobserved.find((row) => row.ip === '10.20.1.50');
    expect(printer?.had_coverage).toBe(false);
    // A section that found something starts open; one that found nothing
    // starts closed, which is why neither is clicked here.
    const unobserved = page.getByRole('region', { name: /In the plan, not on the network/ });
    await expect(unobserved.getByText('10.20.1.50')).toBeVisible();
    await expect(unobserved.getByText('Nobody looked').first()).toBeVisible();
    await expect(
      unobserved.getByText('no evidence either way', { exact: false }).first(),
    ).toBeVisible();

    // The same contradiction the drawer shows, in the report: one row per
    // field, with the devices named rather than their ids.
    const mismatches = page.getByRole('region', { name: /The record and the device disagree/ });
    await expect(mismatches.getByText('Campus/AS1').first()).toBeVisible();
    expect(await mismatches.innerText()).not.toContain(seeded.as1 ?? 'no-id');

    expect(console_.messages).toEqual([]);
  });

  test('prints what the engine will not decide, word for word, and offers no button that decides it', async ({
    page,
  }) => {
    const console_ = watchConsole(page);
    await openIpam(page, 'reconcile');

    const report = await rpc<Reconciliation>('ipam.reconcile', {});
    expect(report.refused.length).toBeGreaterThan(0);
    const refused = page.getByRole('region', { name: 'What this report will not decide' });
    for (const line of report.refused) {
      await expect(refused.getByText(line, { exact: true })).toBeVisible();
    }
    // Nothing here resolves, fixes or applies anything.
    const buttons = await page.getByRole('button').allInnerTexts();
    expect(buttons.filter((label) => /resolve|fix|apply|accept/i.test(label))).toEqual([]);

    expect(console_.messages).toEqual([]);
  });

  test('a section that found nothing says what finding nothing means', async ({ page }) => {
    const console_ = watchConsole(page);
    await openIpam(page, 'reconcile');

    const report = await rpc<Reconciliation>('ipam.reconcile', {});
    expect(report.prefixes_without_evidence.length).toBeGreaterThan(0);
    // The IPv6 block and the HQ blocks are documented and nothing has been
    // seen in them; the section that is empty in this seed is the mask one.
    expect(report.mask_mismatches).toEqual([]);
    const masks = page.getByRole('button', { name: /An interface mask that is not the block’s/ });
    await expect(masks).toBeVisible();
    await masks.click();
    await expect(
      page.getByText('Every interface mask matches the block it is in.'),
    ).toBeVisible();

    expect(console_.messages).toEqual([]);
  });
});

test.describe('conflicts and VLANs', () => {
  test('a conflict is shown with both sides of its evidence, and nothing resolves it', async ({
    page,
  }) => {
    const console_ = watchConsole(page);
    await openIpam(page, 'conflicts');

    const conflicts = await rpc<Conflict[]>('ipam.conflicts');
    const gateway = conflicts.find((conflict) => conflict.kind === 'gateway_mismatch');
    const orphan = conflicts.find((conflict) => conflict.kind === 'outside_any_prefix');
    expect(gateway).toBeDefined();
    expect(orphan).toBeDefined();
    if (!gateway || !orphan) throw new Error('the seed produced no conflicts');

    await expect(page.getByText(gateway.detail)).toBeVisible({ timeout: 30_000 });
    await expect(page.getByText(orphan.detail)).toBeVisible();
    await expect(page.getByText('written down').first()).toBeVisible();
    await expect(page.getByText('seen on the network').first()).toBeVisible();
    await expect(page.getByText('reported, never resolved', { exact: false })).toBeVisible();

    expect(console_.messages).toEqual([]);
  });

  test('a VLAN the switches call something else is flagged against the record', async ({ page }) => {
    const console_ = watchConsole(page);
    await openIpam(page, 'vlans');

    const vlans = await rpc<VlanRow[]>('ipam.vlans', {});
    const documented = vlans.find((row) => row.vid === 10 && row.documented);
    expect(documented).toBeDefined();
    const table = page.getByRole('table', { name: 'VLANs' });
    await expect(table).toBeVisible({ timeout: 30_000 });
    const row = table.getByRole('row').filter({ hasText: 'Staff wired' }).first();
    await expect(row.getByText('Documented and seen')).toBeVisible();
    // Three access switches call VLAN 10 "Staff"; the record says otherwise.
    await expect(row.getByText(/devices differ|Staff/).first()).toBeVisible();
    // A VLAN with no record has no "written" time: the backend sends 0 for it.
    expect(await table.innerText()).not.toContain('56 years ago');

    expect(console_.messages).toEqual([]);
  });
});

test.describe('search', () => {
  test('finds an address record, and the block that contains it', async ({ page }) => {
    const console_ = watchConsole(page);
    await openIpam(page);

    await page.getByRole('searchbox', { name: /Search blocks/ }).fill('printer');
    const addresses = page.getByRole('region', { name: 'Addresses that match' });
    await expect(addresses.getByText('10.20.1.50')).toBeVisible({ timeout: 20_000 });
    await expect(addresses.getByText('Documented only')).toBeVisible();
    await expect(addresses.getByText('never seen')).toBeVisible();

    // A whole address finds its record *and* its block, as two kinds of hit.
    await page.getByRole('searchbox', { name: /Search blocks/ }).fill('10.20.1.50');
    const blocks = page.getByRole('region', { name: 'Blocks that match' });
    await expect(blocks.getByText('10.20.1.0/24')).toBeVisible();
    await expect(
      page.getByRole('region', { name: 'Addresses that match' }).getByText('10.20.1.50'),
    ).toBeVisible();

    // An address a *lab* documented. The engine finds it now, and also adds
    // its production-namespace row for a whole typed address — so the screen
    // must show one hit, in the namespace that knows the address, and not the
    // same address twice with opposite answers.
    await page.getByRole('searchbox', { name: /Search blocks/ }).fill('10.10.10.2');
    const found = page.getByRole('region', { name: 'Addresses that match' });
    await expect(found.getByText('10.10.10.2')).toHaveCount(1, { timeout: 20_000 });
    await expect(found.getByText('Documented and seen')).toBeVisible();
    await expect(found.getByText('gw-staff')).toBeVisible();
    await expect(found.getByText('Lab: Campus')).toBeVisible();
    await expect(found.getByText('No record, nothing seen')).toHaveCount(0);

    // And an address nothing knows anywhere keeps its one empty row: that is
    // the answer to that search, not an artefact to hide.
    await page.getByRole('searchbox', { name: /Search blocks/ }).fill('10.10.10.77');
    await expect(found.getByText('10.10.10.77')).toHaveCount(1);
    await expect(found.getByText('No record, nothing seen')).toBeVisible();

    // A partial address, which the engine used to mistake for a MAC.
    await page.getByRole('searchbox', { name: /Search blocks/ }).fill('10.10.10');
    await expect(found.getByText('10.10.10.1').first()).toBeVisible();
    await expect(found.getByText('Lab: Campus').first()).toBeVisible();

    expect(console_.messages).toEqual([]);
  });
});

test.describe('writing things down', () => {
  test('documenting a discovered block adopts it, keeping where it came from', async ({ page }) => {
    const console_ = watchConsole(page);
    await openIpam(page);

    const rows = await rpc<PrefixRow[]>('ipam.prefixes', {});
    const target = rows.find((row) => row.source === 'discovered' && row.cidr === '10.10.20.0/24');
    if (!target) throw new Error('the seed has no discovered 10.10.20.0/24');

    await selectBlock(page, target.cidr);
    await page.getByRole('button', { name: 'Document this block' }).first().click();
    const dialog = page.getByRole('dialog', { name: `Document ${target.cidr}` });
    await expect(dialog.getByText('Saving it keeps that provenance', { exact: false })).toBeVisible();
    await dialog.getByLabel('Name').fill('Guest (lab)');
    await dialog.getByRole('button', { name: 'Save' }).click();

    await expect(block(page, target.cidr).getByText('Documented')).toBeVisible({ timeout: 20_000 });
    const after = await rpc<PrefixRow[]>('ipam.prefixes', {});
    const saved = after.find((row) => row.id === target.id);
    expect(saved?.source).toBe('manual');
    expect(saved?.name).toBe('Guest (lab)');
    // The provenance is kept, not thrown away by the adoption.
    expect(saved?.discovered_from.length).toBeGreaterThan(0);

    expect(console_.messages).toEqual([]);
  });

  test('a prefix with host bits set is refused with the correction offered', async ({ page }) => {
    const console_ = watchConsole(page);
    await openIpam(page);

    await page.getByRole('button', { name: 'Add a block' }).first().click();
    const dialog = page.getByRole('dialog', { name: 'Add a block' });
    await dialog.getByLabel('Block').fill('10.44.5.7/24');
    await dialog.getByRole('button', { name: 'Add the block' }).click();
    // The engine's own refusal, with the network it corrected to.
    await expect(
      dialog.getByText('`10.44.5.7/24` has host bits set', { exact: false }),
    ).toBeVisible({ timeout: 15_000 });
    await dialog.getByRole('button', { name: 'Use 10.44.5.0/24' }).click();
    await dialog.getByRole('button', { name: 'Add the block' }).click();
    await expect(block(page, '10.44.5.0/24')).toBeVisible({ timeout: 20_000 });

    expect(console_.messages).toEqual([]);
  });

  test('reserving an address something already holds is refused, and names the holder', async ({
    page,
  }) => {
    const console_ = watchConsole(page);
    await openIpam(page);
    await selectBlock(page, '10.10.10.0/24');

    await page.getByRole('button', { name: 'Reserve' }).first().click();
    const dialog = page.getByRole('dialog', { name: /Reserve an address in 10.10.10.0\/24/ });
    await dialog.getByLabel('Address').fill('10.10.10.3');
    await dialog.getByRole('button', { name: 'Reserve it' }).click();
    await expect(dialog.getByText('That address is taken')).toBeVisible({ timeout: 15_000 });
    await expect(dialog.getByText('Campus/DS2', { exact: false })).toBeVisible();

    expect(console_.messages).toEqual([]);
  });

  test('an import is previewed line by line before anything is written', async ({ page }) => {
    const console_ = watchConsole(page);
    await openIpam(page);

    await page.getByRole('button', { name: 'Import / export' }).click();
    const dialog = page.getByRole('dialog', { name: 'Import and export' });
    await dialog
      .getByLabel('CSV')
      .fill('cidr,name,site,role\n10.60.0.0/24,Voice,HQ,voice\nnonsense,Bad,HQ,');
    await dialog.getByRole('button', { name: 'Preview' }).click();
    await expect(dialog.getByText('1 new')).toBeVisible({ timeout: 15_000 });
    await expect(dialog.getByText('1 invalid')).toBeVisible();
    await expect(dialog.getByText('Nothing has been written yet.')).toBeVisible();
    // The preview really wrote nothing.
    const before = await rpc<Summary>('ipam.summary');
    await dialog.getByRole('button', { name: /Write 1 row\b/ }).click();
    await expect(dialog.getByText(/Written: 1 new/)).toBeVisible({ timeout: 15_000 });
    const after = await rpc<Summary>('ipam.summary');
    expect(after.prefixes).toBe(before.prefixes + 1);

    expect(console_.messages).toEqual([]);
  });
});
