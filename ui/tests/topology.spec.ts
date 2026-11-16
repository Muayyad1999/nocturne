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

  await seedRates(devices);
}

/**
 * Real interface rates on the links, measured by monitoring from the
 * devices' own counters.
 *
 * Two reads are needed for a rate, and they have to straddle a simulated
 * minute: the simulator reports its uptime in whole minutes, a rate is a
 * counter delta over an *uptime* delta, and two reads inside one minute
 * measure no interval at all and are recorded as a gap. Hence the wait, which
 * is the same one the Monitoring spec pays.
 *
 * The fixture is uneven on purpose, because the four answers this screen has
 * to keep apart are four different states of the data:
 *
 * * the campus switches are polled, so their links carry measured rates, and
 *   the demo lab's traffic flows are asymmetrical, so the two directions of a
 *   link really differ;
 * * EDGE and the ISP router are not monitored at all, so the links at the top
 *   of the drawing have no rate — which must not be drawn as idle;
 * * DS2 is polled and then taken off monitoring with a five-second interval,
 *   so its readings really are older than its own cadence allows and the
 *   screen has to call them stale rather than current;
 * * every device is given a half-hour interval first, so the scheduler does
 *   not re-read the counters in the middle of a test and leave the drawing
 *   showing a different number from the one the test just read.
 */
/**
 * Polls until monitoring has really worked a rate out, rather than assuming
 * that two polls a minute apart did. Each poll is waited for as a job, so the
 * two readings are a whole simulated minute apart and the uptime delta
 * between them is real; if the first pair still measures nothing (a poll that
 * lands at the wrong end of a minute measures no interval at all), it tries
 * again instead of leaving a later test to fail mysteriously.
 */
async function pollUntilMeasured(ids: readonly string[]): Promise<void> {
  const poll = async () => {
    const started = await rpc<{ job_id: string }>('monitor.poll_now', { device_ids: [...ids] });
    await waitForJob(started.job_id);
  };
  const anyRate = async () => {
    const rows = await rpc<{ device_id: string; out_bps: number | null }[]>('monitor.interfaces', {
      limit: 5000,
    });
    return rows.some((row) => ids.includes(row.device_id) && (row.out_bps ?? 0) > 0);
  };

  await poll();
  for (let attempt = 0; attempt < 4; attempt += 1) {
    await new Promise((done) => setTimeout(done, 65_000));
    await poll();
    if (await anyRate()) return;
  }
  throw new Error('monitoring never measured a rate on the campus switches');
}

async function seedRates(devices: readonly { id: string; name: string }[]): Promise<void> {
  const polled = ['Campus/DS1', 'Campus/DS2', 'Campus/AS1', 'Campus/AS2', 'Campus/AS3'];
  const idOf = (name: string) => {
    const device = devices.find((entry) => entry.name === name);
    if (!device) throw new Error(`the demo lab has no ${name}`);
    return device.id;
  };

  for (const device of devices) {
    const monitored = polled.includes(device.name);
    await rpc('devices.update', {
      id: device.id,
      monitor: { enabled: monitored, icmp: monitored, snmp: false, interval_s: 1_800 },
    });
  }

  const ids = polled.map(idOf);
  await pollUntilMeasured(ids);

  // DS2 is now measured, and from here on nothing refreshes it: a device set
  // to be read every five seconds whose last reading is a minute and a half
  // old is not being watched, and its rates are not current.
  await rpc('devices.update', {
    id: idOf('Campus/DS2'),
    monitor: { enabled: false, icmp: false, snmp: true, interval_s: 5 },
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

// -------------------------------------------------------------- the live map

/** One row of the interface table monitoring keeps: the drawing's own source. */
interface IfRow {
  device_id: string;
  name: string;
  in_bps: number | null;
  out_bps: number | null;
  speed_bps: number | null;
  at: number;
}

interface GraphEnd {
  node: string;
  interface?: string;
}

interface GraphLink {
  id: string;
  a: GraphEnd;
  b: GraphEnd;
  speed_bps?: number;
}

interface GraphNode {
  id: string;
  name: string;
  device_id?: string;
}

interface GraphAnswer {
  nodes: GraphNode[];
  links: GraphLink[];
}

/** The rates as the backend has them, by device and interface. */
async function rates(): Promise<Map<string, IfRow>> {
  const rows = await rpc<IfRow[]>('monitor.interfaces', { limit: 5000 });
  return new Map(rows.map((row) => [`${row.device_id}|${row.name.toLowerCase()}`, row]));
}

/** The load bars drawn for one link: the channel per direction and the bar in it. */
function loadOf(page: Page, linkId: string) {
  const group = page.locator(`[data-testid="load-${linkId}"]`);
  return {
    group,
    tracks: group.locator('path[data-kind]'),
    bars: group.locator('path[data-band]'),
  };
}

/** Zoom to 100 %, where both directions' numbers are drawn. */
async function zoomToFull(page: Page): Promise<void> {
  await page.getByRole('button', { name: /Reset to 100%/ }).click();
}

/** Selects one link by its id, whatever it looks like. */
async function selectLink(page: Page, linkId: string): Promise<void> {
  await page.locator(`g[data-link="${linkId}"] path`).first().click({ force: true });
}

test.describe('the load on the lines', () => {
  test('draws what monitoring measured, in both directions, as the length of the bar', async ({
    page,
  }) => {
    const console_ = watchConsole(page);
    const graph = await rpc<GraphAnswer>('topology.get', {});
    const measured = await rates();
    const deviceOf = new Map(
      graph.nodes.filter((node) => node.device_id).map((node) => [node.id, node.device_id as string]),
    );
    const sampleOf = (end: GraphEnd) => {
      const device = deviceOf.get(end.node);
      return device && end.interface
        ? measured.get(`${device}|${end.interface.toLowerCase()}`)
        : undefined;
    };

    // The DS1–AS1 uplink: both ends are polled, the demo lab's traffic flows
    // up it, and the two cards are a layout rank apart, so the line is long
    // enough on screen to carry what is drawn on it.
    const idOf = (name: string) => graph.nodes.find((entry) => entry.name === name)?.id;
    const ds1 = idOf('Campus/DS1');
    const as1 = idOf('Campus/AS1');
    const link = graph.links.find(
      (entry) =>
        (entry.a.node === ds1 && entry.b.node === as1) ||
        (entry.a.node === as1 && entry.b.node === ds1),
    );
    expect(link, 'the fixture must have a DS1–AS1 link').toBeTruthy();
    if (!link) return;
    // …and monitoring must really have measured both of its ends, or the
    // assertions below would be about nothing.
    expect(sampleOf(link.a)?.out_bps, 'DS1 or AS1 was not measured').not.toBeNull();
    expect(sampleOf(link.b)?.out_bps).not.toBeNull();

    await openTopology(page);

    const drawn = loadOf(page, link.id);
    // One channel per direction, and a bar in each: both were measured.
    await expect(drawn.tracks).toHaveCount(2);
    await expect(drawn.bars).toHaveCount(2);

    // The bar's length *is* the number: `pathLength` is 100, so the first
    // figure of the dash pattern is the percentage, and it has to agree with
    // the rate the backend measured over the speed the port reported.
    const sample = sampleOf(link.a);
    // The link's own speed first, then the port's: the same order the screen
    // works it out in, and for this link the same number either way.
    const speed = link.speed_bps ?? sample?.speed_bps ?? 0;
    const expected = Math.max(1.6, ((sample?.out_bps ?? 0) / speed) * 100);
    const pattern = await drawn.bars.first().getAttribute('stroke-dasharray');
    expect(pattern, `dash pattern for ${link.id}`).toBeTruthy();
    expect(Number(pattern?.split(' ')[0])).toBeCloseTo(expected, 1);

    // And the panel says where each number came from, per direction.
    await selectLink(page, link.id);
    const panel = page.getByRole('complementary', { name: 'Selection' });
    await expect(panel.getByRole('heading', { name: 'Load' })).toBeVisible();
    await expect(panel.getByText(/sent by/).first()).toBeVisible();
    await expect(panel.getByText(/ of \d/).first()).toBeVisible();
    await expect(panel.getByText(/Measured/).first()).toBeVisible();

    expect(console_.messages, backend.log()).toEqual([]);
  });

  test('a link nothing has measured says so, and is never drawn as idle', async ({ page }) => {
    const console_ = watchConsole(page);
    const graph = await rpc<GraphAnswer>('topology.get', {});
    const measured = await rates();
    const deviceOf = new Map(
      graph.nodes.filter((node) => node.device_id).map((node) => [node.id, node.device_id as string]),
    );
    const known = (end: GraphEnd) => {
      const device = deviceOf.get(end.node);
      const row =
        device && end.interface
          ? measured.get(`${device}|${end.interface.toLowerCase()}`)
          : undefined;
      return row?.in_bps != null || row?.out_bps != null;
    };

    // EDGE and the ISP router are not monitored, so neither end of the link
    // between them has ever been measured.
    const link = graph.links.find((entry) => !known(entry.a) && !known(entry.b));
    expect(link, 'the fixture must have a link neither end of which is measured').toBeTruthy();
    if (!link) return;

    await openTopology(page);

    // The layer is on and drawing bars elsewhere, so what follows is not a
    // test that passes because nothing is drawn at all.
    await expect(page.locator('[data-testid^="load-"] path[data-band]').first()).toBeAttached();

    const drawn = loadOf(page, link.id);
    // Nothing on this one: no channel and no bar. An empty channel would say
    // monitoring looked and found nothing moving, a different claim.
    await expect(drawn.tracks).toHaveCount(0);
    await expect(drawn.bars).toHaveCount(0);

    await selectLink(page, link.id);
    const panel = page.getByRole('complementary', { name: 'Selection' });
    await expect(panel.getByText('no rate has been measured for this direction')).toHaveCount(2);
    await expect(
      panel.getByText(/monitoring has not read these ports/),
    ).toBeVisible();
    // And the one number this screen must never show here.
    await expect(panel.getByText('0%')).toHaveCount(0);
    await expect(panel.getByText('idle')).toHaveCount(0);

    // In writing on the line, too, once the zoom leaves room for it.
    await zoomToFull(page);
    await expect(page.locator('svg text', { hasText: 'no rate' }).first()).toBeAttached();

    expect(console_.messages, backend.log()).toEqual([]);
  });

  test('a reading older than its device’s own cadence is drawn stale, not live', async ({
    page,
  }) => {
    const console_ = watchConsole(page);
    const graph = await rpc<GraphAnswer>('topology.get', {});
    const ds2 = graph.nodes.find((node) => node.name === 'Campus/DS2');
    const edge = graph.nodes.find((node) => node.name === 'Campus/EDGE');
    expect(ds2 && edge).toBeTruthy();
    if (!ds2 || !edge) return;
    // The only measurement this link has is DS2's, taken before DS2 was
    // taken off monitoring: EDGE is not monitored, so there is nothing
    // fresher to prefer.
    const link = graph.links.find(
      (entry) =>
        (entry.a.node === ds2.id && entry.b.node === edge.id) ||
        (entry.a.node === edge.id && entry.b.node === ds2.id),
    );
    expect(link, 'the fixture must have an EDGE–DS2 link').toBeTruthy();
    if (!link) return;

    await openTopology(page);

    // The reading ages while the page is open; the drawing re-reckons it on
    // its own, without a refetch, because time passing is not an event.
    await expect
      .poll(async () => loadOf(page, link.id).group.locator('path[data-age="stale"]').count(), {
        message: 'waiting for DS2’s reading to pass its own cadence',
        timeout: 150_000,
      })
      .toBeGreaterThan(0);

    await selectLink(page, link.id);
    const panel = page.getByRole('complementary', { name: 'Selection' });
    await expect(panel.getByText('Stale').first()).toBeVisible();
    // It still says what was measured and when — stale is not unknown.
    await expect(panel.getByText(/Measured/).first()).toBeVisible();

    expect(console_.messages, backend.log()).toEqual([]);
  });

  test('says when the load was measured, and can be switched off', async ({ page }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    const asOf = page.getByTestId('load-as-of');
    await expect(asOf).toContainText(/load measured/);

    // Off: the bars go, and so does the line about them.
    await page.getByRole('button', { name: 'Load' }).click();
    await expect(page.locator('[data-testid^="load-"]')).toHaveCount(0);
    await expect(asOf).toHaveCount(0);

    // And it is remembered, which is what a toggle on a map has to do.
    await page.reload();
    await pageLoaded(page);
    await expect(node(page, 'Campus/DS1')).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('[data-testid^="load-"]')).toHaveCount(0);
    await page.getByRole('button', { name: 'Load' }).click();
    await expect(page.getByTestId('load-as-of')).toContainText(/load measured/);

    expect(console_.messages, backend.log()).toEqual([]);
  });

  test('the legend explains idle, no speed, no rate and stale apart from each other', async ({
    page,
  }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    const legend = page.getByRole('region', { name: 'Legend' });
    const load = legend.getByRole('list', { name: 'Link load' });
    await expect(load.getByText('the bar is how much of the port speed is in use')).toBeVisible();
    await expect(load.getByText('nothing has been measured — not the same as idle')).toBeVisible();

    expect(console_.messages, backend.log()).toEqual([]);
  });
});

test.describe('a session from the map', () => {
  test.afterEach(async () => {
    const open = await rpc<{ id: string }[]>('terminal.list');
    for (const session of open) await rpc('terminal.close', { session_id: session.id });
  });

  test('a right-click gives a terminal on the device, in the backend’s own session', async ({
    page,
  }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    await node(page, 'Campus/AS1').click({ button: 'right' });
    const menu = page.getByRole('menu', { name: 'Actions for Campus/AS1' });
    await expect(menu).toBeVisible();
    await menu.getByRole('menuitem', { name: 'Terminal' }).click();

    // The dock, under the drawing, with the device's own prompt in it.
    const dock = page.getByRole('region', { name: 'Terminal on Campus/AS1' });
    await expect(dock).toBeVisible({ timeout: 60_000 });
    await expect
      .poll(
        async () =>
          page.evaluate(() => {
            const rows = [...document.querySelectorAll('.xterm-rows')].find(
              (element) => (element as HTMLElement).offsetParent !== null,
            );
            return rows ? (rows.textContent ?? '') : '';
          }),
        { message: 'waiting for AS1’s prompt', timeout: 60_000 },
      )
      .toContain('AS1');

    // It is one session, in the backend, on that device — not a second way in.
    const sessions = await rpc<{ id: string; device_id?: string | null; state: string }[]>(
      'terminal.list',
    );
    expect(sessions).toHaveLength(1);
    const devices = await rpc<{ id: string; name: string }[]>('devices.list');
    const as1 = devices.find((device) => device.name === 'Campus/AS1');
    expect(sessions[0]?.device_id).toBe(as1?.id);

    // Asking again lands in the session that is open rather than dialling the
    // device twice: two prompts on one switch is how a person types into the
    // wrong one.
    await node(page, 'Campus/AS1').click();
    const panel = page.getByRole('complementary', { name: 'Selection' });
    await expect(panel.getByRole('button', { name: 'Show the session' })).toBeVisible();
    await panel.getByRole('button', { name: 'Show the session' }).click();
    expect(await rpc<unknown[]>('terminal.list')).toHaveLength(1);

    expect(console_.messages, backend.log()).toEqual([]);
  });

  test('a node that cannot be given a session says why instead of offering a button', async ({
    page,
  }) => {
    const console_ = watchConsole(page);
    await openTopology(page);

    // The far end of the drawn link is a name a person typed.
    await node(page, 'ISP uplink').click();
    const panel = page.getByRole('complementary', { name: 'Selection' });
    await expect(panel.getByTestId('no-terminal')).toContainText(/somebody typed/);
    await expect(panel.getByRole('button', { name: 'Terminal' })).toBeDisabled();

    // And the right-click menu says the same in the words a menu has room for.
    await node(page, 'ISP uplink').click({ button: 'right' });
    const menu = page.getByRole('menu', { name: 'Actions for ISP uplink' });
    await expect(menu.getByRole('menuitem', { name: /Terminal — not a device/ })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    await page.keyboard.press('Escape');

    expect(console_.messages, backend.log()).toEqual([]);
  });

  test('a lab device that is powered off says so rather than failing to connect', async ({
    page,
  }) => {
    const console_ = watchConsole(page);
    const labs = await rpc<{ id: string; name: string }[]>('labs.list');
    const lab = labs.find((entry) => entry.name === 'Campus');
    expect(lab).toBeTruthy();
    if (!lab) return;

    await rpc('labs.node.update', { lab_id: lab.id, node: 'AS3', powered: false });
    try {
      await openTopology(page);
      await node(page, 'Campus/AS3').click();
      const panel = page.getByRole('complementary', { name: 'Selection' });
      await expect(panel.getByTestId('no-terminal')).toContainText(/powered off/);
      await expect(panel.getByRole('button', { name: 'Terminal' })).toBeDisabled();
      expect(console_.messages, backend.log()).toEqual([]);
    } finally {
      await rpc('labs.node.update', { lab_id: lab.id, node: 'AS3', powered: true });
    }
  });
});
