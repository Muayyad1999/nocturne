/**
 * The Topology screen, against a real engine with a real network behind it.
 *
 * Nothing here is mocked. A `nocturne-server` runs on a fresh data directory,
 * the UI is the production bundle it serves, and the network is the demo
 * campus lab: sixteen simulated devices whose CDP neighbour tables, interface
 * addresses, MAC tables and routing tables the collectors really read. The
 * drawing is then built from that, which is the only way to prove this screen
 * — a drawing of nothing says nothing.
 *
 * The seed deliberately produces all three kinds of link:
 *
 * * `confirmed`  — both ends reported it (DS1 ↔ AS1 and the rest of the core)
 * * `one_sided`  — a cable added after one end was last collected from, so
 *                  only the end that was collected again knows about it
 * * `inferred`   — the layer-3 view's shared subnets, and a host found in a
 *                  MAC table on an access port
 *
 * plus a link drawn by a person, and a node that is not in the inventory.
 *
 * `use_lab_links` is turned off on purpose: with it on, the lab's own wiring
 * is authoritative and every link is `confirmed`, which would never exercise
 * the thing this screen exists to get right.
 */

import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import { openApp, pageLoaded, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let uiDir: string;

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

interface JobRow {
  state: 'running' | 'succeeded' | 'failed' | 'cancelled';
}

async function waitForJob(id: string): Promise<void> {
  for (let attempt = 0; attempt < 120; attempt += 1) {
    const job = await rpc<JobRow>('jobs.get', { id });
    if (job.state !== 'running') return;
    await new Promise((done) => setTimeout(done, 1_000));
  }
  throw new Error(`job ${id} did not finish`);
}

async function collectFrom(ids: string[]): Promise<void> {
  const started = await rpc<{ job_id: string }>('collect.run', { device_ids: ids });
  await waitForJob(started.job_id);
}

/** The campus, collected from, with one cable only one end knows about. */
async function seedCampus(): Promise<void> {
  const lab = await rpc<{ id: string }>('labs.create', { name: 'Campus', example: 'demo' });
  await rpc('labs.start', { id: lab.id });

  // The graph is to be built from what the devices reported, not from the
  // lab's own wiring: that is the path a real network takes.
  await rpc('settings.set', {
    section: 'topology',
    value: {
      stale_after_hours: 24,
      endpoint_max_macs: 3,
      fold_port_channels: true,
      max_snapshots: 200,
      default_algorithm: 'layered',
      use_lab_links: false,
      use_monitor_utilisation: true,
    },
  });

  const devices = await rpc<{ id: string; name: string }[]>('devices.list');
  await collectFrom(devices.map((device) => device.id));

  // A cable added after both ends were collected from, then only one end
  // collected again: the backend calls that link `one_sided`, and the screen
  // must not draw it like a link both ends reported.
  await rpc('labs.link.add', {
    lab_id: lab.id,
    a: { node: 'AS1', interface: 'FastEthernet0/3' },
    b: { node: 'AS2', interface: 'FastEthernet0/3' },
  });
  const as1 = devices.find((device) => device.name.endsWith('/AS1'));
  if (!as1) throw new Error('the demo lab has no AS1');
  await collectFrom([as1.id]);

  // Something discovery cannot see: the circuit to the internet provider.
  await rpc('topology.manual.save', {
    a: { device: 'Campus/EDGE', interface: 'GigabitEthernet0/0' },
    b: { name: 'ISP uplink', kind: 'cloud' },
    label: '100 Mb/s fibre',
  });
}

test.beforeAll(async () => {
  test.setTimeout(600_000);
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

/** A node on the canvas: its accessible name starts with the device's name. */
function node(page: Page, name: string) {
  return page.getByRole('button', { name: new RegExp(`^${name},`) });
}

async function openTopology(page: Page): Promise<void> {
  await openApp(page, '/topology', { backend });
  await expect(node(page, 'Campus/DS1')).toBeVisible({ timeout: 30_000 });
}

/** Every link on the canvas, with how the backend says it is known. */
async function links(page: Page): Promise<{ confidence: string; source: string }[]> {
  return page.evaluate(() =>
    [...document.querySelectorAll('g[data-confidence][data-source]')].map((element) => ({
      confidence: element.getAttribute('data-confidence') ?? '',
      source: element.getAttribute('data-source') ?? '',
    })),
  );
}

/** The stroke the browser really paints a link's line with. */
async function wireStyle(
  page: Page,
  confidence: string,
): Promise<{ dash: string; colour: string } | null> {
  return page.evaluate((want) => {
    const group = [...document.querySelectorAll('g[data-confidence]')].find(
      (element) => element.getAttribute('data-confidence') === want,
    );
    const wire = group?.querySelector('path:nth-of-type(2)');
    if (!wire) return null;
    const style = window.getComputedStyle(wire);
    return { dash: style.strokeDasharray, colour: style.stroke };
  }, confidence);
}

// -------------------------------------------------------------------- tests

test.describe('the drawing', () => {
  test('draws the campus the devices described, and says what is in it', async ({ page }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    // Every device the lab created is on the drawing.
    for (const name of ['Campus/EDGE', 'Campus/DS1', 'Campus/DS2', 'Campus/AS1', 'Campus/AS3']) {
      await expect(node(page, name)).toBeVisible();
    }
    // The lab devices are labelled as such wherever they appear.
    await expect(node(page, 'Campus/DS1')).toHaveAttribute('data-lab', 'true');

    // The count under the toolbar is the backend's own stats.
    const stats = await rpc<{ stats: { devices: number; links: number } }>('topology.get', {});
    await expect(page.getByText(/\d+ devices · \d+ links/)).toContainText(
      `${stats.stats.devices} devices · ${stats.stats.links} links`,
    );

    expect(console_.messages).toEqual([]);
  });

  test('tells the three kinds of link apart, and explains them in the legend', async ({ page }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    const drawn = await links(page);
    expect(drawn.some((link) => link.confidence === 'confirmed')).toBe(true);
    expect(drawn.some((link) => link.confidence === 'one_sided')).toBe(true);
    expect(drawn.some((link) => link.source === 'drawn')).toBe(true);

    // Not just a different attribute: a different line, really painted.
    const confirmed = await wireStyle(page, 'confirmed');
    const oneSided = await wireStyle(page, 'one_sided');
    expect(confirmed).not.toBeNull();
    expect(oneSided).not.toBeNull();
    expect(oneSided?.dash).not.toBe(confirmed?.dash);
    expect(oneSided?.colour).not.toBe(confirmed?.colour);

    // And it says so in words on the line, not only in a pattern.
    await expect(page.locator('svg text', { hasText: 'one end only' }).first()).toBeVisible();
    await expect(page.locator('svg text', { hasText: 'drawn by hand' }).first()).toBeVisible();

    const legend = page.getByRole('region', { name: 'Legend' });
    await expect(legend.getByText('both ends reported the link')).toBeVisible();
    await expect(legend.getByText('only one end reported it')).toBeVisible();
    await expect(legend.getByText('Drawn by a person')).toBeVisible();

    expect(console_.messages).toEqual([]);
  });

  test('the layer-3 view is built of inferred links and subnet groups', async ({ page }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    await page.getByRole('combobox', { name: 'Layer' }).selectOption('l3');
    await expect
      .poll(async () => (await links(page)).every((link) => link.confidence === 'inferred'), {
        timeout: 15_000,
      })
      .toBe(true);

    // A subnet with three or more devices in it is a group, not a line.
    await expect(page.locator('svg text', { hasText: '10.10.10.0/24' }).first()).toBeVisible();
    const legend = page.getByRole('region', { name: 'Legend' });
    await expect(legend.getByText('derived from addresses or MAC tables')).toBeVisible();

    expect(console_.messages).toEqual([]);
  });

  test('shows the drawing’s own caveats, in the backend’s words', async ({ page }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    const graph = await rpc<{ warnings: string[] }>('topology.get', {});
    expect(graph.warnings.length).toBeGreaterThan(0);

    const band = page.getByRole('region', { name: 'What this drawing cannot tell you' });
    await band.getByRole('button', { name: /cannot tell you/ }).click();
    // Quoted, not paraphrased.
    for (const warning of graph.warnings) {
      await expect(band.getByText(warning, { exact: true })).toBeVisible();
    }

    expect(console_.messages).toEqual([]);
  });

  test('a name two devices answer to is a warning, not a guessed link', async ({ page }) => {
    const console_ = watchConsole(page);
    // A second device that answers to "DS1": the neighbour reports now match
    // two devices, and the backend refuses to choose.
    const created = await rpc<{ id: string }>('devices.create', {
      name: 'DS1',
      address: '10.255.0.99',
      profile: 'cisco_ios',
    });
    try {
      await openTopology(page);
      const band = page.getByRole('region', { name: 'What this drawing cannot tell you' });
      await band.getByRole('button', { name: /cannot tell you/ }).click();
      await expect(
        band.getByText(/more than one device answers to that name/).first(),
      ).toBeVisible();
      expect(console_.messages).toEqual([]);
    } finally {
      await rpc('devices.delete', { ids: [created.id] });
    }
  });
});

test.describe('one thing at a time', () => {
  test('a node says what it is connected to, and how each link is known', async ({ page }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    await node(page, 'Campus/DS1').click();
    const panel = page.getByRole('complementary', { name: 'Selection' });
    await expect(panel.getByRole('heading', { name: 'Campus/DS1' })).toBeVisible();
    await expect(panel.getByText('Layer-3 switch')).toBeVisible();

    // The rows are `topology.neighbors`, with the evidence on each one.
    await expect(panel.getByText('Connected to', { exact: false })).toBeVisible();
    await expect(panel.getByRole('button', { name: /Campus\/EDGE/ }).first()).toBeVisible();
    await expect(panel.getByText(/confirmed · CDP/).first()).toBeVisible();

    expect(console_.messages).toEqual([]);
  });

  test('a one-sided link says that only one end reported it, and not which', async ({ page }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    // The cable added after AS2 was last collected from.
    await page.locator('g[data-confidence="one_sided"] path').first().click({ force: true });
    const panel = page.getByRole('complementary', { name: 'Selection' });
    await expect(panel.getByText('How this link is known')).toBeVisible();
    await expect(panel.getByText(/Only one end reported this link/)).toBeVisible();
    await expect(panel.getByText(/unmanaged, silent, or has never been collected from/)).toBeVisible();
    await expect(panel.getByText('one-sided')).toBeVisible();

    expect(console_.messages).toEqual([]);
  });

  test('a link a person drew is never shown as one the devices reported', async ({ page }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    await page.locator('g[data-source="drawn"] path').first().click({ force: true });
    const panel = page.getByRole('complementary', { name: 'Selection' });
    await expect(panel.getByText('A person drew this link. No device reported it.')).toBeVisible();
    await expect(panel.getByText('100 Mb/s fibre')).toBeVisible();
    // It can only be changed where it was made, and the button says so.
    await expect(
      panel.getByRole('button', { name: 'Edit it in the list of drawn links…' }),
    ).toBeVisible();

    // And the far end of it is honestly not a device.
    await node(page, 'ISP uplink').click();
    await expect(panel.getByText('Not in the inventory')).toBeVisible();
    await expect(
      panel.getByText(/This end of a link was named by a person, not discovered/),
    ).toBeVisible();

    expect(console_.messages).toEqual([]);
  });
});

test.describe('arranging it', () => {
  test('a node dragged somewhere stays there, through a reload', async ({ page }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    const target = node(page, 'Campus/AS3');
    const before = await target.boundingBox();
    expect(before).not.toBeNull();
    if (!before) return;

    await page.mouse.move(before.x + before.width / 2, before.y + before.height / 2);
    await page.mouse.down();
    await page.mouse.move(before.x + before.width / 2 + 120, before.y + before.height / 2 - 90, {
      steps: 12,
    });
    await page.mouse.up();

    // The backend was told, so the position survives everything.
    await expect
      .poll(
        async () => {
          const graph = await rpc<{ nodes: { pinned: boolean }[] }>('topology.get', {});
          return graph.nodes.filter((entry) => entry.pinned).length;
        },
        { timeout: 15_000 },
      )
      .toBe(1);

    await page.reload();
    await pageLoaded(page);
    await expect(target).toBeVisible({ timeout: 30_000 });
    const after = await target.boundingBox();
    expect(after).not.toBeNull();
    expect(Math.abs((after?.x ?? 0) - before.x)).toBeGreaterThan(30);

    expect(console_.messages).toEqual([]);
  });

  test('an automatic arrangement leaves no node on top of another', async ({ page }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    await page.getByRole('button', { name: 'Arrange the nodes' }).click();
    await page.getByRole('menuitem', { name: 'Layered' }).click();
    await expect(page.getByText(/Arranged \d+ nodes/)).toBeVisible({ timeout: 20_000 });

    const graph = await rpc<{ nodes: { x: number; y: number }[] }>('topology.get', {});
    const places = graph.nodes;
    expect(places.length).toBeGreaterThan(5);
    for (let one = 0; one < places.length; one += 1) {
      for (let other = one + 1; other < places.length; other += 1) {
        const a = places[one];
        const b = places[other];
        if (!a || !b) continue;
        // The drawn card is 184 x 52, inside the backend's 190 x 64 box.
        expect(Math.abs(a.x - b.x) >= 184 || Math.abs(a.y - b.y) >= 52).toBe(true);
      }
    }

    expect(console_.messages).toEqual([]);
  });
});

test.describe('the questions worth asking', () => {
  test('a path ends where the collected data runs out, and says why', async ({ page }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    await page.getByRole('tab', { name: 'Path' }).click();
    // An `<input list=…>` is a combobox in the accessibility tree, not a
    // textbox, so these are found by their label.
    await page.getByLabel('From', { exact: true }).fill('Campus/DS1');
    await page.getByLabel('To', { exact: true }).fill('8.8.8.8');
    await page.getByRole('button', { name: 'Trace', exact: true }).click();

    // Two hops that really happened, each with the device's own reasoning.
    await expect(page.getByText('did not arrive')).toBeVisible({ timeout: 20_000 });
    await expect(page.getByText(/default route 0\.0\.0\.0\/0 \(ospf\)/)).toBeVisible();
    await expect(page.getByText('Why it stops here')).toBeVisible();
    await expect(
      page.getByText(/is not an address of any device Nocturne manages/),
    ).toBeVisible();

    // And the stop is on the drawing, not only in the panel.
    await expect(page.locator('svg text', { hasText: 'the path stops here' })).toBeVisible();

    expect(console_.messages).toEqual([]);
  });

  test('a path that is two equal-cost choices says which one it followed', async ({ page }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    await page.getByRole('tab', { name: 'Path' }).click();
    await page.getByLabel('From', { exact: true }).fill('Campus/EDGE');
    await page.getByLabel('To', { exact: true }).fill('10.10.20.10');
    await page.getByRole('button', { name: 'Trace', exact: true }).click();

    await expect(page.getByText(/equal-cost paths/)).toBeVisible({ timeout: 20_000 });
    expect(console_.messages).toEqual([]);
  });

  test('finding a host names the access port and dismisses the trunk sightings', async ({
    page,
  }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    // The address of the one endpoint the MAC tables know about.
    const host = await rpc<{ nodes: { address?: string; id: string }[] }>('topology.get', {
      include_endpoints: true,
    });
    const endpoint = host.nodes.find((entry) => entry.id.startsWith('h:'));
    expect(endpoint?.address).toBeTruthy();
    if (!endpoint?.address) return;

    await page
      .getByRole('searchbox', { name: 'Search for a device, an IP or a MAC address' })
      .fill(endpoint.address);
    await page.keyboard.press('Enter');

    const panel = page.getByRole('region', { name: 'Find a host' });
    await expect(panel.getByText('The port it decided on')).toBeVisible({ timeout: 20_000 });
    await expect(panel.getByText('access port', { exact: true })).toBeVisible();
    await expect(
      panel.getByText(/the path frames take, not where it is plugged in/),
    ).toBeVisible();
    // The reasoning, step by step, as the backend wrote it.
    await expect(panel.getByText('How it worked that out')).toBeVisible();
    await expect(panel.getByText(/is in the MAC table of \d+ ports/)).toBeVisible();

    expect(console_.messages).toEqual([]);
  });

  test('a name that is in no drawing is said to be missing, not guessed at', async ({ page }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    await page
      .getByRole('searchbox', { name: 'Search for a device, an IP or a MAC address' })
      .fill('no-such-switch');
    await page.keyboard.press('Enter');
    await expect(page.getByText(/Nothing in this drawing is called/)).toBeVisible();

    expect(console_.messages).toEqual([]);
  });
});

test.describe('taking it away', () => {
  test('exports in all five formats, with the text on screen first', async ({ page }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    await page.getByRole('button', { name: 'Export this drawing' }).click();
    const dialog = page.getByRole('dialog', { name: 'Export this drawing' });
    await expect(dialog).toBeVisible();

    const expected: [string, RegExp][] = [
      ['svg', /<svg xmlns=/],
      ['drawio', /<mxfile host="nocturne"/],
      ['mermaid', /graph TD/],
      ['dot', /graph topology \{/],
      ['json', /"nodes": \[/],
    ];
    for (const [format, shape] of expected) {
      await dialog.getByRole('combobox', { name: 'Format' }).selectOption(format);
      await expect(dialog.getByText(shape).first()).toBeVisible({ timeout: 20_000 });
      await expect(dialog.getByText(/topology-physical\./).first()).toBeVisible();
    }

    const download = page.waitForEvent('download');
    await dialog.getByRole('button', { name: 'Download' }).click();
    expect((await download).suggestedFilename()).toBe('topology-physical.json');

    await page.keyboard.press('Escape');
    expect(console_.messages).toEqual([]);
  });

  test('the links drawn by hand are listed apart from what was discovered', async ({ page }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    await page.getByRole('button', { name: 'Links drawn by hand' }).click();
    const drawer = page.getByRole('dialog', { name: 'Links drawn by hand' });
    await expect(drawer.getByText('Campus/EDGE')).toBeVisible();
    await expect(drawer.getByText('ISP uplink')).toBeVisible();
    await expect(drawer.getByText('100 Mb/s fibre')).toBeVisible();

    // Drawing one more, as a person would.
    await drawer.getByRole('button', { name: 'Draw a link' }).click();
    await drawer.getByLabel('Device', { exact: true }).first().fill('Campus/AS3');
    await drawer.getByLabel('Call it', { exact: true }).fill('Cupboard switch');
    await drawer.getByLabel('Label', { exact: true }).fill('no management address');
    await drawer.getByRole('button', { name: 'Draw it', exact: true }).click();
    await expect(drawer.getByText('Cupboard switch')).toBeVisible({ timeout: 20_000 });

    await page.keyboard.press('Escape');
    // It is on the canvas, and marked as something a person put there.
    await expect(node(page, 'Cupboard switch')).toBeVisible({ timeout: 20_000 });
    expect((await links(page)).filter((link) => link.source === 'drawn').length).toBe(2);

    expect(console_.messages).toEqual([]);
  });
});
