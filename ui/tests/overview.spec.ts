/**
 * The Overview, against a real engine.
 *
 * A `nocturne-server` is started on its own empty data directory and put into
 * state through the same `/api/rpc` endpoint the app uses: real devices, a
 * real simulated switch in a real lab, a real alert, a real change, a real
 * configuration backup, a real compliance run and a real running job.
 *
 * The rule these tests are written to: **nothing is asserted against a number
 * written in this file.** Every expectation about a figure on screen is
 * compared with what `overview.get` itself answers at that moment, because a
 * test that hard-codes "3" proves only that the test and the screen were
 * written by the same person. Only the *shapes* are pinned here — that a
 * measured zero shows a zero, that an absent part shows no figure at all, and
 * that a capped count says it is a floor.
 *
 * The scheduler is switched off before anything is seeded (monitoring would
 * otherwise mark devices up or down while a test reads them) and so is the
 * automatic insight scan, so that what the screen shows changes only when a
 * test changes it.
 */

import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Locator, type Page } from '@playwright/test';
import type { Overview } from '../src/api/types/ops';
import { openApp, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor, type Rpc } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let uiDir: string;
let prebuilt: string | undefined;

test.beforeAll(async ({ request }) => {
  test.setTimeout(300_000);
  // Built outside `ui/dist`: the server crate embeds that directory, so
  // writing there would make cargo rebuild the binary first. A build that has
  // already been made can be handed over in NOCTURNE_UI_DIST.
  if (process.env.NOCTURNE_UI_DIST) {
    prebuilt = process.env.NOCTURNE_UI_DIST;
  } else {
    uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
    await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
      cwd: UI_DIR,
      timeout: 240_000,
    });
  }
  backend = await startBackend({ uiDir: prebuilt ?? uiDir });

  const rpc = rpcFor(request, backend);
  // Nothing may move while a test is looking at it: no polling, no automatic
  // scan. Everything these tests see was put there by these tests.
  await rpc('settings.set', { section: 'monitoring', value: { enabled: false } });
  await rpc('settings.set', { section: 'insights', value: { auto_scan: false } });
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

// ------------------------------------------------------------------ helpers

/** What the engine itself says the overview is, right now. */
function read(rpc: Rpc): Promise<Overview> {
  return rpc<Overview>('overview.get', { refresh: true });
}

function tile(page: Page, id: string): Locator {
  return page.locator(`[data-tile="${id}"]`);
}

function attention(page: Page): Locator {
  return page.getByRole('list', { name: 'Things that need attention' });
}

/** Waits until the screen has caught up with a figure the engine reports. */
async function showsFigure(tileLocator: Locator, figure: string): Promise<void> {
  await expect(tileLocator).toContainText(figure, { timeout: 15_000 });
}

// --------------------------------------------------------------- first run

test('an empty inventory is a welcome, not a wall of zeroes', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  const before = await read(rpc);
  expect(before.devices.total, 'this test needs a fresh engine').toBe(0);

  await openApp(page, '/', { backend });

  await expect(page.getByRole('heading', { name: 'Nothing is in the inventory yet' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Explore the demo network' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Add a device' })).toBeVisible();
  // No tile pretends to measure an estate that does not exist.
  await expect(tile(page, 'devices')).toHaveCount(0);
  await expect(tile(page, 'alerts')).toHaveCount(0);

  expect(console.messages).toEqual([]);
});

test('the first-run door really builds a network, and the screen fills itself', async ({
  page,
}) => {
  test.setTimeout(300_000);
  const console = watchConsole(page);
  // Its own engine: this test puts devices into an inventory, which the
  // tests after it would otherwise inherit.
  const engine = await startBackend({ uiDir: prebuilt ?? uiDir });
  try {
    await openApp(page, '/', { backend: engine });
    await page.getByRole('button', { name: 'Start the demo network' }).click();

    // The welcome gives way to the screen itself, with the devices the
    // simulator really created.
    await expect(page.locator('[data-tile="devices"]')).toBeVisible({ timeout: 120_000 });
    const after = await rpcFor(page.request, engine)<Overview>('overview.get', { refresh: true });
    expect(after.devices.total).toBeGreaterThan(1);
    expect(after.devices.lab).toBe(after.devices.total);
    await expect(page.locator('[data-tile="devices"]')).toContainText(
      `of ${after.devices.total}`,
      { timeout: 30_000 },
    );
    // Everything on this screen now comes from simulated devices, and the
    // tile says so.
    await expect(page.locator('[data-tile="devices"]')).toContainText(
      `${after.devices.lab} simulated`,
    );
  } finally {
    await engine.stop();
  }

  expect(console.messages).toEqual([]);
});

// --------------------------------------------- quiet is not absent

test('a quiet feature and an absent feature do not look the same', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  // One device, nothing wrong with it, and no compliance run ever made.
  await rpc('devices.create', {
    name: 'core-sw1',
    address: '10.0.0.1',
    site: 'HQ',
    role: 'core',
    profile: 'cisco_ios',
  });

  const overview = await read(rpc);
  // The engine's own answer is the premise of this test: alerting is present
  // and says zero; compliance is null with a reason beside it.
  expect(overview.alerts, 'alerting should be present in this build').not.toBeNull();
  expect(overview.alerts?.open).toBe(0);
  expect(overview.compliance, 'nothing has ever been checked').toBeNull();
  const why = overview.unavailable.find((entry) => entry.section === 'compliance');
  expect(why?.reason).toBe('no_data');

  await openApp(page, '/', { backend });

  // The feature that is there and quiet: a figure, and what the zero means.
  const alerts = tile(page, 'alerts');
  await showsFigure(alerts, String(overview.alerts?.open ?? -1));
  await expect(alerts).toContainText('Nothing firing');

  // The feature that has nothing to say: no figure at all, a headline of its
  // own, and the engine's own sentence for why.
  const compliance = tile(page, 'compliance');
  await expect(compliance).toContainText('Nothing recorded yet');
  await expect(compliance).toContainText(why?.message ?? '');
  await expect(compliance).toHaveAttribute('data-kind', 'unavailable');
  await expect(alerts).toHaveAttribute('data-kind', 'value');
  // The thing that must never happen: a missing source reading as a zero.
  await expect(compliance).not.toContainText('0');

  // And the list says what it could not look at, with the same reason.
  await expect(page.getByText('What this list could not look at')).toBeVisible();

  expect(console.messages).toEqual([]);
});

// ------------------------------------------------- every tile, against the RPC

test('every tile says what the engine says, and the attention list agrees', async ({
  page,
  request,
}) => {
  test.setTimeout(300_000);
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  // A second real device, a simulated switch in a running lab, a real alert,
  // a real change and a real configuration backup of the simulated switch.
  await rpc('devices.create', {
    name: 'access-sw2',
    address: '10.0.1.2',
    site: 'Branch North',
    role: 'access',
    profile: 'cisco_ios',
  });
  const lab = await rpc<{ id: string }>('labs.create', { name: 'ovlab' });
  await rpc('labs.node.add', { lab_id: lab.id, model: 'switch', name: 'SW1' });
  await rpc('labs.start', { id: lab.id });

  const devices = await rpc<{ id: string; name: string }[]>('devices.list');
  const core = devices.find((device) => device.name === 'core-sw1');
  const simulated = devices.find((device) => device.name === 'ovlab/SW1');
  expect(core && simulated).toBeTruthy();

  await rpc('alerts.raise', {
    device_id: core?.id,
    severity: 'critical',
    title: 'Device unreachable',
    rule: 'reachability',
    dedup_key: `reach:${core?.id}`,
  });
  await rpc('changes.create', {
    title: 'Describe the uplink',
    ticket: 'NET-412',
    targets: [
      {
        device_id: simulated?.id,
        intent: { type: 'lines', lines: ['interface FastEthernet0/1', ' description uplink'] },
      },
    ],
  });
  const backup = await rpc<{ job_id: string }>('config.backup', {
    device_ids: [simulated?.id],
    kinds: ['running'],
  });
  await expect
    .poll(async () => (await rpc<{ state: string }>('jobs.get', { id: backup.job_id })).state, {
      timeout: 120_000,
      intervals: [500, 1_000, 2_000],
    })
    .toBe('succeeded');

  // One scan, so the findings on the list are ones the engine really made
  // about the configuration it really read.
  const scan = await rpc<{ job_id: string }>('insights.scan', {});
  await expect
    .poll(async () => (await rpc<{ state: string }>('jobs.get', { id: scan.job_id })).state, {
      timeout: 120_000,
      intervals: [500, 1_000, 2_000],
    })
    .toBe('succeeded');

  await openApp(page, '/', { backend });

  // Read the engine once the screen is up, and hold every tile against it.
  const overview = await read(rpc);
  const alerts = overview.alerts;
  const changes = overview.changes;
  const backups = overview.backups;
  expect(alerts && changes && backups).toBeTruthy();

  const devicesTile = tile(page, 'devices');
  await showsFigure(devicesTile, String(overview.devices.up));
  await expect(devicesTile).toContainText(`of ${overview.devices.total}`);
  // Never polled is its own state, and it is not "up".
  if (overview.devices.unknown > 0) {
    await expect(devicesTile).toContainText(`${overview.devices.unknown} never polled`);
  }
  if (overview.devices.lab > 0) {
    await expect(devicesTile).toContainText(`${overview.devices.lab} simulated`);
  }

  await showsFigure(tile(page, 'alerts'), String(alerts?.open));
  await showsFigure(tile(page, 'changes'), String(changes?.awaiting_approval));

  const coverage = Math.round(
    (100 * (backups?.devices_with_backup ?? 0)) /
      ((backups?.devices_with_backup ?? 0) + (backups?.devices_without ?? 0)),
  );
  await showsFigure(tile(page, 'backups'), `${coverage}%`);
  await expect(tile(page, 'backups')).toContainText(
    `${backups?.devices_with_backup} of ${(backups?.devices_with_backup ?? 0) + (backups?.devices_without ?? 0)}`,
  );

  // The alert the engine reports as newest is the row the list shows, with
  // the words the engine gave it.
  const newest = alerts?.newest[0];
  expect(newest, 'the alert that was raised should be here').toBeTruthy();
  await expect(attention(page).getByText(newest?.title ?? '')).toBeVisible();

  // Nothing on this screen is invented: the devices with no backup are the
  // ones the engine counted, said once, on the tile that links to them.
  await expect(tile(page, 'backups')).toContainText(
    `${backups?.devices_without} devices never backed up`,
  );

  // Every finding the engine put at the top of the list is on the list, in
  // its own words.
  const top = overview.insights?.top ?? [];
  expect(top.length, 'the scan should have found something').toBeGreaterThan(0);
  for (const finding of top.slice(0, 3)) {
    await expect(attention(page)).toContainText(finding.title);
  }

  // Those findings are about a simulated device, and say so where they appear.
  expect(top.some((finding) => finding.lab)).toBe(true);
  await expect(
    attention(page).getByTitle('Lab device: simulated, not production').first(),
  ).toBeVisible();

  expect(console.messages).toEqual([]);
});

test('running the check once turns the same tile into a reading', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  await openApp(page, '/', { backend });
  // Still nothing recorded: the seeding above backed a device up but checked
  // nothing against it.
  await expect(tile(page, 'compliance')).toHaveAttribute('data-kind', 'unavailable');

  await rpc('compliance.run', {});
  await expect
    .poll(async () => (await read(rpc)).compliance !== null, { timeout: 60_000 })
    .toBe(true);

  const overview = await read(rpc);
  const score = overview.compliance?.score_pct;
  expect(score, 'the run should have produced a score').not.toBeNull();

  // No reload: the screen follows the events the run raised.
  const compliance = tile(page, 'compliance');
  await expect(compliance).toHaveAttribute('data-kind', 'value', { timeout: 30_000 });
  await showsFigure(compliance, `${Math.round(score as number)}%`);

  expect(console.messages).toEqual([]);
});

test('the time the data was read is on screen, and it keeps moving', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  await openApp(page, '/', { backend });
  await expect(page.getByText(/^Read /)).toBeVisible();
  await expect(page.getByText(/Overview read /)).toBeVisible();

  const before = await read(rpc);
  await page.getByRole('button', { name: 'Refresh' }).click();
  // Asking by hand really asks the engine again: the reading's own timestamp
  // moves forward.
  await expect
    .poll(async () => (await read(rpc)).generated_at, { timeout: 30_000 })
    .toBeGreaterThan(before.generated_at);
  await expect(page.getByText('· asked by hand')).toBeVisible();

  expect(console.messages).toEqual([]);
});

// --------------------------------------------------------- acting on a row

test('an alert can be acknowledged from the list, and the engine agrees', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  await openApp(page, '/', { backend });

  const before = await read(rpc);
  const alert = before.alerts?.newest.find((entry) => entry.state === 'firing');
  expect(alert, 'a firing alert is needed for this test').toBeTruthy();

  const row = attention(page).getByRole('listitem').filter({ hasText: alert?.title ?? '' }).first();
  await row.getByRole('button', { name: 'Acknowledge' }).click();

  await expect
    .poll(
      async () => (await rpc<{ state: string }>('alerts.get', { id: alert?.id })).state,
      { timeout: 30_000 },
    )
    .toBe('acknowledged');
  // The row stays — the alert is still open — but it no longer offers the
  // button, because there is nothing left to acknowledge.
  await expect(row.getByRole('button', { name: 'Acknowledge' })).toHaveCount(0, {
    timeout: 30_000,
  });

  expect(console.messages).toEqual([]);
});

test('a new alert reaches the screen without a reload', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  await openApp(page, '/', { backend });
  const before = await read(rpc);
  const open = before.alerts?.open ?? 0;
  await showsFigure(tile(page, 'alerts'), String(open));

  await rpc('alerts.raise', {
    severity: 'high',
    title: 'A fan has stopped',
    rule: 'environment',
    dedup_key: 'fan:1',
  });

  // No navigation, no reload: the event did it.
  await showsFigure(tile(page, 'alerts'), String(open + 1));
  await expect(attention(page)).toContainText('A fan has stopped', { timeout: 30_000 });

  expect(console.messages).toEqual([]);
});

// -------------------------------------------------------------- running work

test('a running job is shown with its progress and can be stopped', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  await openApp(page, '/', { backend });

  // A sweep of a private range: long enough to be seen running, and it
  // touches nothing in the inventory.
  const started = await rpc<{ job_id: string }>('discovery.scan', {
    targets: '10.77.0.0/24',
    probe_ssh: true,
  });

  const running = page.getByRole('list', { name: 'Jobs running now' });
  await expect(running).toBeVisible({ timeout: 30_000 });
  const job = (await read(rpc)).jobs?.running.find((entry) => entry.id === started.job_id);
  expect(job, 'the scan should be running').toBeTruthy();
  await expect(running).toContainText(job?.title ?? '');

  // Stopping says what it will and will not undo before it does anything.
  await running.getByRole('button', { name: 'Cancel' }).first().click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('is not undone');
  await dialog.getByRole('button', { name: 'Stop the job' }).click();

  await expect
    .poll(async () => (await rpc<{ state: string }>('jobs.get', { id: started.job_id })).state, {
      timeout: 60_000,
    })
    .toBe('cancelled');
  await expect(page.getByText('Nothing running')).toBeVisible({ timeout: 30_000 });

  expect(console.messages).toEqual([]);
});

// ------------------------------------------------------------ a capped count

test('a count past the list limit is still exact, because the summary knows it', async ({
  page,
  request,
}) => {
  test.setTimeout(300_000);
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  // `overview.get` reads at most 500 alerts from `alerts.list`, so more than
  // that used to come back as a floor — "at least 500". It does not any more:
  // `alerts.summary` knows the real number without listing anything, and the
  // overview reads it. (It had been failing to parse the summary and falling
  // back to the capped list, which is what this test used to assert; the
  // capped *rendering* is still covered, in `overview.test.ts`.)
  // Raising this many is the only honest way to pass the limit.
  const raise = async (index: number) =>
    rpc('alerts.raise', {
      severity: 'medium',
      title: `Bulk ${index}`,
      rule: 'bulk',
      dedup_key: `bulk:${index}`,
    });
  const numbers = Array.from({ length: 520 }, (_value, index) => index);
  let next = 0;
  await Promise.all(
    Array.from({ length: 16 }, async () => {
      for (let index = next++; index < numbers.length; index = next++) await raise(index);
    }),
  );

  const overview = await read(rpc);
  expect(overview.alerts?.from, 'the summary should answer, not the capped list').toBe(
    'alerts.summary',
  );
  expect(overview.alerts?.truncated, 'a summary count is not a floor').toBe(false);
  expect(overview.alerts?.open, 'every alert raised is counted').toBeGreaterThanOrEqual(520);

  await openApp(page, '/', { backend });

  const alerts = tile(page, 'alerts');
  await showsFigure(alerts, String(overview.alerts?.open));
  // And it says the number plainly, rather than hedging a figure it knows.
  await expect(alerts).not.toContainText('at least');
  await expect(alerts).not.toContainText('there may be worse');

  expect(console.messages).toEqual([]);
});
