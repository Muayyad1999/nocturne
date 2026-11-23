/**
 * `pnpm shots` — the Operations screen, photographed in both themes, at both
 * sizes, with and without the assistant dock.
 *
 * Everything in these pictures was produced by a real engine: a real lab with
 * a simulated switch, a real alert raised by a real rule, a real change
 * planned and applied, a real backup job, and an incident whose timeline
 * gathered itself from all of them. No fixtures, no mock transport.
 *
 * Two engines are started, because "nothing has happened here yet" cannot be
 * staged on one where something has — and the empty states are the half of
 * this screen that is easiest to get wrong.
 */

import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import type { EvidenceWithBody, Incident } from '../src/api/types/ops';
import { goTo, openApp, settle, type Theme } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor, type Rpc } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));
const SHOTS = resolve(fileURLToPath(new URL('./shots/ops', import.meta.url)));
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };
const THEMES: readonly Theme[] = ['dark', 'light'];

test.describe.configure({ mode: 'serial', timeout: 900_000 });

/** The engine with a shift's worth of history on it. */
let working: Backend;
/** A second engine where nothing has ever happened. */
let fresh: Backend;
let uiDir: string;
let prebuilt: string | undefined;
let incidentId = '';
let jobId = '';
/** The evidence the engine was asked to keep, one of each kind. */
const evidence = { terminal: '', text: '', drawing: '', chart: '', long: '' };

async function finished(rpc: Rpc, id: string): Promise<void> {
  await expect
    .poll(async () => (await rpc<{ state: string }>('jobs.get', { id })).state, {
      timeout: 180_000,
      intervals: [500, 1_000, 2_000],
    })
    .not.toBe('running');
}

test.beforeAll(async ({ request }) => {
  test.setTimeout(900_000);
  if (process.env.NOCTURNE_UI_DIST) {
    prebuilt = process.env.NOCTURNE_UI_DIST;
  } else {
    uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
    await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
      cwd: UI_DIR,
      timeout: 240_000,
    });
  }
  working = await startBackend({ uiDir: prebuilt ?? uiDir });
  fresh = await startBackend({ uiDir: prebuilt ?? uiDir });

  for (const backend of [working, fresh]) {
    const rpc = rpcFor(request, backend);
    // Nothing moves between the shot and the picture.
    await rpc('settings.set', { section: 'monitoring', value: { enabled: false, down_after: 1 } });
    await rpc('settings.set', { section: 'insights', value: { auto_scan: false } });
  }

  const rpc = rpcFor(request, working);
  for (const [name, address, site, role] of [
    ['core-sw1', '10.0.0.1', 'HQ', 'core'],
    ['dist-sw1', '10.0.0.2', 'HQ', 'distribution'],
    ['access-sw2', '10.0.1.2', 'Branch North', 'access'],
  ]) {
    await rpc('devices.create', { name, address, site, role, profile: 'cisco_ios' });
  }
  const lab = await rpc<{ id: string }>('labs.create', { name: 'Campus' });
  await rpc('labs.node.add', { lab_id: lab.id, model: 'switch', name: 'SW1' });
  await rpc('labs.start', { id: lab.id });
  const devices = await rpc<{ id: string; name: string }[]>('devices.list');
  const id = (name: string) => devices.find((device) => device.name === name)?.id ?? '';
  const node = id('Campus/SW1');

  // A change, really applied to the simulated switch.
  const created = await rpc<{ change: { id: string } }>('changes.create', {
    title: 'Describe the uplink',
    ticket: 'NET-412',
    targets: [
      {
        device_id: node,
        intent: {
          type: 'lines',
          lines: ['interface FastEthernet0/1', 'description uplink to core', 'exit'],
        },
      },
    ],
    plan: true,
  });
  const changeId = created.change.id;
  await expect
    .poll(async () => (await rpc<{ state: string }>('changes.get', { id: changeId })).state, {
      timeout: 120_000,
    })
    .toBe('planned');
  const planned = await rpc<{ targets: { plan: { plan_hash: string } }[] }>('changes.get', {
    id: changeId,
  });
  await rpc('changes.approve', {
    id: changeId,
    plan_hashes: planned.targets.map((target) => target.plan.plan_hash),
  });
  await finished(rpc, (await rpc<{ job_id: string }>('changes.apply', { id: changeId })).job_id);

  // A backup, so the job history has something with a log in it.
  const backup = await rpc<{ job_id: string }>('config.backup', {
    device_ids: [node],
    kinds: ['running'],
  });
  jobId = backup.job_id;
  await finished(rpc, backup.job_id);

  // Two real alerts from two real rules: one gets linked to the incident, the
  // other stays unlinked so the timeline shows what it offers.
  for (const name of ['Lab switch down', 'Lab switch unreachable']) {
    await rpc('alerts.rules.save', {
      name,
      severity: name.endsWith('down') ? 'critical' : 'high',
      for_s: 0,
      scope: { sites: [], roles: [], tags: [], device_ids: [], include_lab: true },
      condition: { type: 'device_down' },
    });
  }
  await rpc('labs.node.update', { lab_id: lab.id, node: 'SW1', powered: false });
  await finished(rpc, (await rpc<{ job_id: string }>('monitor.poll_now', { device_ids: [node] })).job_id);
  const alerts = await rpc<{ id: string; rule_name: string; started_at: number }[]>('alerts.list', {
    state: 'open',
  });
  const first = alerts.find((alert) => alert.rule_name === 'Lab switch down');

  const incident = await rpc<Incident>('incidents.create', {
    from_alert: first?.id,
    author: 'mo',
    assignee: 'mo',
    summary: 'The simulated campus switch stopped answering during the evening shift.',
    impact: 'Everything behind Campus/SW1, from 22:05.',
    device_ids: [id('dist-sw1')],
  });
  incidentId = incident.id;
  const started = first?.started_at ?? Date.now();
  // Notes at the times the things happened, not the time they were typed:
  // this is what makes the picture a timeline rather than a list.
  await rpc('incidents.note', {
    id: incidentId,
    text: 'The optic on Fa0/1 was reseated during the afternoon maintenance window.',
    ts: started - 900_000,
    author: 'mo',
  });
  await rpc('incidents.note', {
    id: incidentId,
    text: 'Console answers; the uplink shows no light. Fetching the spare from the HQ cabinet.',
    ts: started + 120_000,
    author: 'mo',
  });
  await rpc('incidents.acknowledge', { id: incidentId, author: 'mo' });
  await rpc('incidents.link', { id: incidentId, change_ids: [changeId], author: 'mo' });
  await rpc('incidents.update', {
    id: incidentId,
    follow_ups: [
      { text: 'Order a spare optic for the HQ cabinet', done: false },
      { text: 'Tell the service desk the branch is back', done: true },
    ],
    author: 'mo',
  });

  // A second incident, resolved, so the list shows more than one state.
  const other = await rpc<Incident>('incidents.create', {
    title: 'Branch North wireless is slow in the afternoon',
    severity: 'sev3',
    author: 'ana',
    device_ids: [id('access-sw2')],
  });
  await rpc('incidents.resolve', {
    id: other.id,
    root_cause: 'A neighbouring tenant moved onto the same channel; the radio was re-planned.',
    author: 'ana',
  });

  // A runbook, with a device link that resolves and one that does not.
  await rpc('notes.save', {
    title: 'How the HQ uplink fails over',
    body: [
      '## When the HQ uplink drops',
      '',
      '1. Check [[core-sw1]] — `show ip route` should still have the default via the backup.',
      '2. The spare optic is in the cabinet beside [[dist-sw1]].',
      '3. The old [[acc-sw9]] is gone; ignore anything that still mentions it.',
    ].join('\n'),
    tags: ['wan', 'failover'],
    sites: ['HQ'],
    author: 'mo',
  });

  // ---- evidence, one of each kind, every one captured by the engine.
  // A second simulated switch that is still powered on, with a real terminal
  // session on it and something printed.
  await rpc('labs.node.add', { lab_id: lab.id, model: 'switch', name: 'SW2' });
  const second =
    (await rpc<{ id: string; name: string }[]>('devices.list')).find((device) => device.name === 'Campus/SW2')?.id ?? '';
  const session = await rpc<{ id: string }>('terminal.open', { device_id: second });
  for (const command of ['show version', 'show interfaces status']) {
    await rpc('terminal.input', { session_id: session.id, data: `${command}\r` });
  }
  await expect
    .poll(
      async () => (await rpc<{ text: string }>('terminal.snapshot', { session_id: session.id, lines: 80 })).text,
      { timeout: 30_000 },
    )
    .toContain('Processor board ID');
  const capture = await rpc<EvidenceWithBody>('evidence.capture_terminal', {
    session_id: session.id,
    lines: 80,
    title: 'SW2 while the uplink was dark',
  });
  evidence.terminal = capture.id;
  await rpc('evidence.attach', { id: capture.id, incident_id: incidentId, author: 'mo' });

  const drawing = await rpc<EvidenceWithBody>('evidence.capture_topology', {
    title: 'The campus as it was drawn at the time',
  });
  evidence.drawing = drawing.id;
  await rpc('evidence.attach', { id: drawing.id, incident_id: incidentId, author: 'mo' });

  // Samples with a real hole in them: polls, a pause, polls.
  const since = Date.now() - 30_000;
  const pollOnce = async () => {
    await finished(rpc, (await rpc<{ job_id: string }>('monitor.poll_now', { device_ids: [second] })).job_id);
    await new Promise((done) => setTimeout(done, 2_100));
  };
  for (let index = 0; index < 4; index += 1) await pollOnce();
  await new Promise((done) => setTimeout(done, 12_000));
  for (let index = 0; index < 3; index += 1) await pollOnce();
  const chart = await rpc<EvidenceWithBody>('evidence.capture_chart', {
    series: [
      { device_id: second, metric: 'up' },
      { device_id: second, metric: 'cpu.pct' },
    ],
    since,
    until: Date.now() + 1_000,
    step_s: 1,
    title: 'SW2 reachability and CPU around the pause',
  });
  evidence.chart = chart.id;

  // Text a person holds, with what a device can print in it: it is kept as
  // supplied by a person, and its control characters are shown, not obeyed.
  const text = await rpc<EvidenceWithBody>('evidence.add_text', {
    text: [
      'SW1#\u001b[2J\u001b[31mshow logging | include LINK\u001b[0m\u0007',
      '%LINK-3-UPDOWN: Interface FastEthernet0/1, changed state to down',
      'no shutdown\rshutdown   ',
      'enable secret ThisIsNotARealSecret',
    ].join('\n'),
    title: 'Read out from the console over the phone',
    note: 'ticket NET-431',
  });
  evidence.text = text.id;

  // A paste of more lines than the viewer puts on screen at once.
  const long = await rpc<EvidenceWithBody>('evidence.add_text', {
    // 5,500 lines of about forty bytes: inside the 256 KiB an item holds.
    text: Array.from(
      { length: 5_500 },
      (_value, index) => `Gi1/0/${(index % 48) + 1} is up, line protocol is up [${index + 1}]`,
    ).join('\n'),
    title: 'The whole of a show interfaces, as it was pasted',
  });
  evidence.long = long.id;

  // One that was attached and then deleted, so the timeline has a tombstone.
  const gone = await rpc<EvidenceWithBody>('evidence.add_text', {
    text: 'show ip interface brief',
    title: 'First look, before the session was logged',
  });
  await rpc('evidence.attach', { id: gone.id, incident_id: incidentId, author: 'mo' });
  await rpc('evidence.delete', { id: gone.id, author: 'mo' });

  // The second engine gets one device and nothing else: the first hour.
  await rpcFor(request, fresh)('devices.create', {
    name: 'core-sw1',
    address: '10.0.0.1',
    site: 'HQ',
    role: 'core',
    profile: 'cisco_ios',
  });
});

test.afterAll(async () => {
  await working?.stop();
  await fresh?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

async function shoot(page: Page, theme: Theme, name: string): Promise<void> {
  const directory = join(SHOTS, THEME_FOLDER[theme]);
  await mkdir(directory, { recursive: true });
  await settle(page);
  await page.screenshot({ path: join(directory, `${name}.png`), animations: 'disabled' });
}

/** Closes the "Download started" notices, so they are not in the next pictures. */
async function dismissNotices(page: Page): Promise<void> {
  const dismiss = page.getByRole('region', { name: 'Notifications' }).getByRole('button', { name: 'Dismiss' });
  while ((await dismiss.count()) > 0) await dismiss.first().click();
}

for (const theme of THEMES) {
  test(`operations — ${theme}`, async ({ page }) => {
    test.setTimeout(600_000);

    await openApp(page, '/ops/incidents', { backend: working, theme });
    await expect(page.getByRole('table', { name: 'Incidents' })).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'incidents');

    // The centrepiece: the timeline, with the note box under it and an
    // unlinked alert offered at its own time.
    await goTo(page, `/ops/incidents?incident=${incidentId}`);
    await expect(page.getByTestId('timeline-row').first()).toBeVisible({ timeout: 60_000 });
    await expect(page.locator('[data-offered]').first()).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'incident');

    // The same page with one line's evidence open: where it came from.
    await page.locator('details > summary').first().click();
    await shoot(page, theme, 'incident-evidence');

    await goTo(page, '/ops/handover');
    await expect(page.locator('[data-section="incidents"]')).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'handover');

    await goTo(page, '/ops/reports');
    await expect(page.getByText('What this build can report on')).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'reports');

    await goTo(page, '/ops/notes');
    await page.getByText('How the HQ uplink fails over').click();
    await expect(page.getByRole('textbox', { name: 'Body' })).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'notes');

    await goTo(page, '/ops/audit');
    await expect(page.getByTestId('audit-entry').first()).toBeVisible({ timeout: 60_000 });
    await page.getByTestId('audit-entry').first().getByRole('button').first().click();
    await shoot(page, theme, 'audit');

    await goTo(page, `/ops/jobs?job=${jobId}`);
    await expect(page.getByRole('dialog')).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'jobs');
  });

  test(`evidence — ${theme}`, async ({ page }) => {
    test.setTimeout(600_000);

    // The list: kinds, lab labels, what is attached, and what it is a page of.
    await openApp(page, '/ops/evidence', { backend: working, theme });
    await expect(page.getByTestId('evidence-row').first()).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'evidence-list');

    // A terminal capture: preformatted text, its source and its hash.
    await goTo(page, `/ops/evidence?item=${evidence.terminal}`);
    await expect(page.getByTestId('evidence-text')).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'evidence-terminal');

    // Text a person handed in: labelled as that, control characters as pictures.
    await goTo(page, `/ops/evidence?item=${evidence.text}`);
    await expect(page.getByTestId('evidence-pictured')).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'evidence-text-supplied');

    // The engine's drawing, as an image.
    await goTo(page, `/ops/evidence?item=${evidence.drawing}`);
    const drawing = page.getByTestId('evidence-drawing');
    await expect(drawing).toBeVisible({ timeout: 60_000 });
    await expect
      .poll(async () => drawing.evaluate((img: HTMLImageElement) => (img.complete ? img.naturalWidth : 0)))
      .toBeGreaterThan(0);
    await shoot(page, theme, 'evidence-drawing');
    // The PNG "Save as PNG" makes of it, to be looked at like the chart's.
    const savingDrawing = page.waitForEvent('download');
    await page.getByRole('button', { name: 'Save as PNG' }).click();
    await (await savingDrawing).saveAs(join(SHOTS, THEME_FOLDER[theme], 'saved-drawing.png'));
    await dismissNotices(page);

    // A capture of more lines than are put on screen at once: which lines
    // are shown, and the way to the others.
    await goTo(page, `/ops/evidence?item=${evidence.long}`);
    await expect(page.getByTestId('evidence-lines-shown')).toBeVisible({ timeout: 60_000 });
    await page.getByRole('button', { name: 'Later lines' }).click();
    await shoot(page, theme, 'evidence-text-paged');
    await goTo(page, `/ops/evidence?item=${evidence.drawing}`);
    await expect(page.getByTestId('evidence-drawing')).toBeVisible({ timeout: 60_000 });

    // The delete confirmation, which names the incident that keeps a tombstone.
    await page.getByRole('button', { name: 'Delete…' }).click();
    await expect(page.getByRole('dialog', { name: 'Delete this evidence?' })).toBeVisible();
    await shoot(page, theme, 'evidence-delete-confirm');
    await page.keyboard.press('Escape');

    // A chart drawn again from the stored points: the gap, and the coverage in words.
    await goTo(page, `/ops/evidence?item=${evidence.chart}`);
    const figure = page.locator('figure');
    await expect(figure.locator('canvas')).toBeVisible({ timeout: 60_000 });
    await expect(page.getByTestId('evidence-gaps')).toBeVisible();
    await shoot(page, theme, 'evidence-chart');
    await figure.getByRole('button', { name: 'Export this chart' }).click();
    await expect(page.getByRole('menu')).toBeVisible();
    await shoot(page, theme, 'evidence-chart-export-menu');
    // What "Save as PNG" hands over, kept beside the screenshots so that the
    // picture itself can be looked at: the plot with its title, its legend
    // and the note about the gap, in the theme on screen.
    const savingChart = page.waitForEvent('download');
    await page.getByRole('menuitem', { name: 'Save as PNG' }).click();
    await (await savingChart).saveAs(join(SHOTS, THEME_FOLDER[theme], 'saved-chart.png'));
    await dismissNotices(page);

    // The incident's timeline: evidence entries that open their item, and a
    // tombstone drawn as what it is.
    await goTo(page, `/ops/incidents?incident=${incidentId}`);
    await expect(page.locator('[data-tombstone]')).toBeVisible({ timeout: 60_000 });
    await page.locator('[data-tombstone]').scrollIntoViewIfNeeded();
    await shoot(page, theme, 'incident-evidence-and-tombstone');

    // "Attach evidence…": pick from what is kept, or capture from a session.
    await page.getByRole('button', { name: 'Attach evidence…' }).click();
    const attach = page.getByRole('dialog', { name: 'Attach evidence' });
    await expect(attach.getByRole('radio').first()).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'attach-evidence-pick');
    await attach.getByRole('tab', { name: 'From a terminal session' }).click();
    await expect(attach.getByRole('combobox', { name: 'Session' })).toBeVisible();
    await shoot(page, theme, 'attach-evidence-session');
    await attach.getByRole('tab', { name: 'Text I hold' }).click();
    await expect(attach.getByRole('textbox', { name: 'Text' })).toBeVisible();
    await shoot(page, theme, 'attach-evidence-text');
    await page.keyboard.press('Escape');

    // The handover lists what was captured in the period.
    await goTo(page, '/ops/handover');
    const section = page.locator('[data-section="evidence"]');
    await expect(section).toBeVisible({ timeout: 60_000 });
    await section.scrollIntoViewIfNeeded();
    await shoot(page, theme, 'handover-evidence');

    // The map's export: a PNG of the engine's drawing, and "Keep as evidence".
    await goTo(page, '/topology');
    await page.getByRole('button', { name: 'Export this drawing' }).click({ timeout: 60_000 });
    const exporting = page.getByRole('dialog', { name: 'Export this drawing' });
    await exporting.getByRole('combobox', { name: 'Format' }).selectOption('png');
    await expect(exporting.getByRole('img', { name: 'The drawing that will be saved' })).toBeVisible({
      timeout: 60_000,
    });
    await shoot(page, theme, 'topology-export-png');
    await exporting.getByRole('button', { name: 'Keep as evidence…' }).click();
    await expect(page.getByRole('dialog', { name: 'Keep as evidence' })).toBeVisible();
    await shoot(page, theme, 'keep-as-evidence');
    await page.keyboard.press('Escape');
    await page.keyboard.press('Escape');
  });

  test(`evidence — ${theme}, narrow and with the dock`, async ({ page }) => {
    test.setTimeout(600_000);
    await page.setViewportSize({ width: 1280, height: 720 });

    await openApp(page, '/ops/evidence', { backend: working, theme, dock: true });
    await expect(page.getByTestId('evidence-row').first()).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'evidence-list-1280-dock');

    await goTo(page, `/ops/evidence?item=${evidence.terminal}`);
    await expect(page.getByTestId('evidence-text')).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'evidence-terminal-1280-dock');

    await goTo(page, `/ops/evidence?item=${evidence.chart}`);
    await expect(page.locator('figure canvas')).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'evidence-chart-1280-dock');

    await page.setViewportSize({ width: 1440, height: 900 });
  });

  test(`operations — ${theme}, narrow and with the dock`, async ({ page }) => {
    test.setTimeout(600_000);
    // 1280 with the assistant dock open is the hardest size this screen has
    // to survive: six tabs, and a timeline whose note box must stay on screen.
    await page.setViewportSize({ width: 1280, height: 720 });

    await openApp(page, '/ops/incidents', { backend: working, theme, dock: true });
    await expect(page.getByRole('table', { name: 'Incidents' })).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'incidents-1280-dock');

    await goTo(page, `/ops/incidents?incident=${incidentId}`);
    await expect(page.getByTestId('timeline-row').first()).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'incident-1280-dock');

    await goTo(page, '/ops/handover');
    await expect(page.locator('[data-section="incidents"]')).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'handover-1280-dock');

    await goTo(page, '/ops/audit');
    await expect(page.getByTestId('audit-entry').first()).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'audit-1280-dock');

    await page.setViewportSize({ width: 1440, height: 900 });
  });

  test(`operations — ${theme}, nothing recorded yet`, async ({ page }) => {
    test.setTimeout(600_000);
    // The empty states, which are half of this screen: each says what the tab
    // is for, and none of them pretends anything happened.
    await openApp(page, '/ops/incidents', { backend: fresh, theme });
    await expect(page.getByText('No incident is being worked on')).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'empty-incidents');

    await goTo(page, '/ops/handover');
    await expect(page.locator('[data-section="incidents"]')).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'empty-handover');

    await goTo(page, '/ops/notes');
    await expect(page.getByText('Nothing is open')).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'empty-notes');

    await goTo(page, '/ops/jobs');
    await expect(page.getByText('Nothing has run yet')).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'empty-jobs');

    await goTo(page, '/ops/evidence');
    await expect(page.getByText('No evidence has been kept yet').first()).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'empty-evidence');
  });
}
