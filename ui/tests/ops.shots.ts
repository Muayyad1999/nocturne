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
import type { Incident } from '../src/api/types/ops';
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
  });
}
