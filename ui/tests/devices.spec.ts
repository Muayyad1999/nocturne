/**
 * The Devices screens, against a real engine.
 *
 * A `nocturne-server` is started on its own empty data directory and seeded
 * through the same `/api/rpc` endpoint the app uses, so nothing here is
 * faked: what the screen shows is what the backend answered.
 */

import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import { openApp, pageLoaded, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor, seedCredential, seedDevices } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

/** How long a run with a cold cargo cache may take. */
test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let uiDir: string;

test.beforeAll(async () => {
  test.setTimeout(300_000);
  // Built outside `ui/dist`: the server crate embeds that directory, so
  // writing to it would make cargo rebuild the whole binary first.
  uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
  await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
    cwd: UI_DIR,
    timeout: 180_000,
  });
  backend = await startBackend({ uiDir });
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

/** The device table's rows, by their name cell. */
function rows(page: Page) {
  return page.getByRole('table', { name: 'Devices' }).locator('tbody tr');
}

test.describe('the device list', () => {
  test('shows what the engine holds, and filters as you type', async ({ page, request }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await seedCredential(rpc);
    await seedDevices(rpc, 24, { prefix: 'a' });

    await openApp(page, '/devices', { backend });

    await expect(page.getByRole('heading', { level: 1, name: 'Devices' })).toBeVisible();
    await expect(rows(page)).toHaveCount(24);
    await expect(page.getByText('24 devices')).toBeVisible();

    // The search filters without a round trip per keystroke.
    await page.getByRole('searchbox', { name: 'Search devices' }).fill('core-a6');
    await expect(rows(page)).toHaveCount(1);
    await expect(page.getByRole('link', { name: 'core-a6' })).toBeVisible();

    // A search that matches nothing says so, and offers the way back.
    await page.getByRole('searchbox', { name: 'Search devices' }).fill('nothing matches this');
    await expect(page.getByText('No device matches')).toBeVisible();
    await page.getByRole('button', { name: 'Clear filters' }).click();
    await expect(rows(page)).toHaveCount(24);

    // A facet chip narrows the list to one site, counted by the backend.
    const filters = page.getByRole('group', { name: 'Filter devices' });
    await filters.getByRole('button', { name: 'Site' }).click();
    // A facet chip's items are toggles, so their role is menuitemcheckbox;
    // the label carries the count the backend worked out.
    await page.getByRole('menuitemcheckbox', { name: 'DC1 · 4' }).click();
    await expect(rows(page)).toHaveCount(4);
    await expect(filters.getByRole('button', { name: 'Site: DC1' })).toBeVisible();
    await page.getByRole('button', { name: 'Clear', exact: true }).click();
    await expect(rows(page)).toHaveCount(24);

    expect(console.messages, backend.log()).toEqual([]);
  });
});

test.describe('adding a device', () => {
  test('tests the connection and says exactly where it failed', async ({ page, request }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await seedCredential(rpc, 'Lab login');

    await openApp(page, '/devices', { backend });
    // The empty state offers the same action when there are no devices yet.
    await page.getByRole('button', { name: 'Add device' }).first().click();

    // The drawer is named "Add device" until the device exists and then
    // "Edit …", so it is located as the one open dialog.
    const drawer = page.getByRole('dialog');
    await expect(drawer).toContainText('Add device');
    await drawer.getByRole('textbox', { name: 'Name', exact: true }).fill('unreachable-rtr');
    // Reserved for documentation (RFC 5737): nothing answers there.
    await drawer.getByRole('textbox', { name: 'Address', exact: true }).fill('192.0.2.11');
    await drawer.getByRole('textbox', { name: 'Port', exact: true }).fill('65022');

    await drawer.getByRole('button', { name: 'Save and test connection' }).click();

    // The result is a sentence, not a stack trace: the stage it reached, and
    // what to do about it.
    const result = drawer.getByRole('status');
    await expect(result).toContainText('Could not connect', { timeout: 60_000 });
    await expect(result).toContainText('Reach the device');
    await expect(result).toContainText(/check the address and port/i);

    // Escape, rather than one of the two buttons both named "Close".
    await page.keyboard.press('Escape');
    // The table renders only the rows in view, so find it rather than scroll.
    await page.getByRole('searchbox', { name: 'Search devices' }).fill('unreachable-rtr');
    await expect(page.getByRole('link', { name: 'unreachable-rtr' })).toBeVisible();

    // The device really was created, with what the form said.
    const created = await rpc<{ address: string; port: number }>('devices.get', {
      id: 'unreachable-rtr',
    });
    expect(created.address).toBe('192.0.2.11');
    expect(created.port).toBe(65_022);

    expect(console.messages, backend.log()).toEqual([]);
  });
});

test.describe('import and export', () => {
  test('previews a pasted CSV before anything is written, then commits it', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);

    await openApp(page, '/devices', { backend });
    const before = await rpc<unknown[]>('devices.list');

    await page.getByRole('button', { name: 'Import' }).click();
    const dialog = page.getByRole('dialog', { name: 'Import devices' });
    await dialog.getByRole('textbox', { name: 'Devices to import' }).fill(
      [
        'name,address,platform,site,role,tags',
        'imported-sw1,10.90.0.1,ios,Imported,access,from-csv',
        'imported-sw2,10.90.0.2,eos,Imported,access,from-csv',
        'broken-row,,ios,Imported,access,',
      ].join('\n'),
    );

    await expect(dialog.getByText('2 new')).toBeVisible();
    await expect(dialog.getByText('1 unusable')).toBeVisible();
    await expect(dialog.getByText('Read as CSV.')).toBeVisible();
    // The invalid row says why, rather than disappearing.
    await expect(dialog.getByRole('table', { name: 'Devices to import' })).toContainText(
      'no address',
    );

    // Nothing has been written while the preview is on screen.
    expect(await rpc<unknown[]>('devices.list')).toHaveLength(before.length);

    await dialog.getByRole('button', { name: 'Import 2 devices' }).click();
    await expect(page.getByRole('link', { name: 'imported-sw1' })).toBeVisible();

    const imported = await rpc<{ profile: string }>('devices.get', { id: 'imported-sw2' });
    expect(imported.profile, 'the platform alias is mapped to a Nocturne device type').toBe(
      'arista_eos',
    );

    expect(console.messages, backend.log()).toEqual([]);
  });

  test('shows the export before it leaves, with no secrets in it', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/devices', { backend });

    await page.getByRole('button', { name: 'Export' }).click();
    const dialog = page.getByRole('dialog', { name: 'Export devices' });
    await expect(dialog).toContainText('name,address,profile');
    await expect(dialog).toContainText('imported-sw1');
    expect(await dialog.textContent()).not.toContain('not-a-real-password');

    await dialog.getByText('Ansible inventory', { exact: true }).click();
    await expect(dialog).toContainText('ansible_host');
    // The device type is carried over as the Ansible network OS.
    await expect(dialog).toContainText('ansible_network_os: cisco.ios.ios');
    expect(await dialog.textContent()).not.toContain('not-a-real-password');

    // Escape, rather than one of the two controls both named "Close".
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    expect(console.messages, backend.log()).toEqual([]);
  });
});

test.describe('the bulk bar', () => {
  test('changes only the fields that were ticked', async ({ page, request }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await openApp(page, '/devices', { backend });

    await page.getByRole('searchbox', { name: 'Search devices' }).fill('imported-sw');
    await expect(rows(page)).toHaveCount(2);
    await page.getByRole('checkbox', { name: 'Select all rows' }).click();
    await expect(page.getByText('2 selected')).toBeVisible();

    await page.getByRole('button', { name: 'Edit…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Edit 2 devices' });
    await dialog.getByRole('checkbox', { name: 'Site' }).check();
    await dialog.getByRole('textbox', { name: 'Site', exact: true }).fill('Moved');
    const tags = dialog.getByRole('textbox', { name: 'Add tags' });
    await tags.fill('bulk-edited');
    await tags.press('Enter');
    await expect(dialog).toContainText('This will set the site to “Moved”');
    await dialog.getByRole('button', { name: 'Apply to 2 devices' }).click();

    const after = await rpc<{ site: string; role: string; tags: string[] }>('devices.get', {
      id: 'imported-sw1',
    });
    expect(after.site).toBe('Moved');
    expect(after.tags).toContain('bulk-edited');
    expect(after.role, 'a field that was not ticked is left alone').toBe('access');

    expect(console.messages, backend.log()).toEqual([]);
  });
});

test.describe('one device', () => {
  test('says what has never been collected instead of showing zeroes', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await rpc('devices.create', {
      name: 'detail-sw1',
      address: '192.0.2.30',
      site: 'HQ',
      role: 'access',
      // A device type that cannot read everything: `linux` has no command
      // for neighbours, VLANs or hardware, which the Data tab must say.
      profile: 'linux',
      tags: ['detail'],
    });

    await openApp(page, '/devices', { backend });
    await page.getByRole('searchbox', { name: 'Search devices' }).fill('detail-sw1');
    await page.getByRole('link', { name: 'detail-sw1' }).click();
    await pageLoaded(page);

    await expect(
      page.getByRole('main').getByRole('heading', { name: 'detail-sw1' }),
    ).toBeVisible();
    await expect(page.getByText('192.0.2.30').first()).toBeVisible();
    // Reachability is unknown, not "down": nothing has checked it.
    await expect(page.getByText('Unknown').first()).toBeVisible();
    await expect(page.getByText('Facts have never been collected')).toBeVisible();

    await page.getByRole('tab', { name: 'Interfaces' }).click();
    await expect(page.getByText('Interfaces have never been collected')).toBeVisible();

    await page.getByRole('tab', { name: 'Data' }).click();
    // A Cisco IOS profile cannot read everything; the ones it cannot are said
    // to be impossible rather than merely missing.
    await expect(page.getByText('Never collected.').first()).toBeVisible();
    await expect(page.getByText('This device type has no command for it.').first()).toBeVisible();

    await page.getByRole('tab', { name: 'Notes' }).click();
    await page
      .getByRole('textbox', { name: 'Notes about detail-sw1' })
      .fill('Lives in rack 4, port 12 of the patch panel.');
    await page.getByRole('button', { name: 'Save note' }).click();
    await expect(page.getByText('Not saved yet.')).toBeHidden();

    const saved = await rpc<{ notes: string }>('devices.get', { id: 'detail-sw1' });
    expect(saved.notes).toContain('rack 4');

    expect(console.messages, backend.log()).toEqual([]);
  });

  test('deletes a device only after saying what will go with it', async ({ page, request }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await rpc('devices.create', { name: 'doomed-sw', address: '192.0.2.99' });

    await openApp(page, '/devices', { backend });
    await page.getByRole('searchbox', { name: 'Search devices' }).fill('doomed-sw');
    await expect(rows(page)).toHaveCount(1);
    await page.getByRole('button', { name: 'Row actions' }).first().click();
    await page.getByRole('menuitem', { name: 'Delete…' }).click();

    const confirmation = page.getByRole('dialog', { name: 'Delete doomed-sw?' });
    await expect(confirmation).toContainText('everything collected from them');
    await expect(confirmation).toContainText('The devices themselves are not touched.');
    await confirmation.getByRole('button', { name: 'Delete' }).click();

    await expect(page.getByText('No device matches')).toBeVisible();
    await expect(rpc('devices.get', { id: 'doomed-sw' })).rejects.toThrow(/not_found/);

    expect(console.messages, backend.log()).toEqual([]);
  });
});

test.describe('the device list at scale', () => {
  // Last on purpose: it leaves two thousand devices in the inventory, and
  // every test after it would pay for them.
  test('stays usable with two thousand devices', async ({ page, request }) => {
    // Two thousand creates, one call each: the backend offers no bulk add.
    test.setTimeout(900_000);
    const rpc = rpcFor(request, backend);
    await seedDevices(rpc, 2_000, { prefix: 'bulk', concurrency: 8 });

    const console = watchConsole(page);
    await openApp(page, '/devices', { backend });

    const facets = await rpc<{ total: number }>('devices.facets');
    // `formatCount` groups digits with a thin space, as the toolbar shows them.
    const shown = String(facets.total).replace(/\B(?=(\d{3})+(?!\d))/g, '\u2009');
    await expect(page.getByText(`${shown} devices`)).toBeVisible({ timeout: 60_000 });

    // Only the visible rows are in the DOM: the table virtualises above 200.
    const rendered = await rows(page).count();
    expect(rendered).toBeGreaterThan(5);
    expect(rendered).toBeLessThan(200);

    // Typing filters the whole inventory in the browser, not over RPC.
    const started = Date.now();
    await page.getByRole('searchbox', { name: 'Search devices' }).fill('core-bulk1992');
    await expect(rows(page)).toHaveCount(1);
    expect(Date.now() - started).toBeLessThan(5_000);

    expect(console.messages, backend.log()).toEqual([]);
  });
});
