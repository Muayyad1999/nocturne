/**
 * The Operations screen, against a real engine.
 *
 * A `nocturne-server` is started on its own empty data directory and put into
 * state through the same `/api/rpc` endpoint the app uses. Everything these
 * tests assert against is real:
 *
 * - a **real lab** with a simulated IOS switch, which is an inventory device
 *   with `transport: sim` and goes through every pipeline as hardware does;
 * - a **real alert**, raised by a **real rule** — a `device_down` rule scoped
 *   to include lab devices, which fires when the node is powered off and the
 *   engine polls it. Nothing calls `alerts.raise`;
 * - a **real change**, planned, approved and applied to that switch, so the
 *   commands really were sent and the configuration really moved;
 * - **real audit rows** written by those operations, and **real jobs** (the
 *   poll, the backup, the apply) with their own logs.
 *
 * The rule the timeline tests are written to: **no expectation is a timestamp
 * written in this file**. The order the screen shows is compared with the
 * order `incidents.get` itself reports, because a test that hard-codes the
 * order proves only that the test and the screen were written together.
 *
 * One deliberate fault injection, marked where it happens: `app.methods` is
 * trimmed in the last test so that a build without the alerting, change and
 * handover features can be driven, which cannot be reached from the outside.
 */

import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Locator, type Page } from '@playwright/test';
import type { AuditSearchResult, Handover, Incident } from '../src/api/types/ops';
import { goTo, openApp, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor, type Rpc } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let uiDir: string;
let prebuilt: string | undefined;

/** The state every test reads, built once by `beforeAll`. */
const world: {
  labId: string;
  switchId: string;
  switchName: string;
  alertId: string;
  incidentId: string;
  changeId: string;
  backupJobId: string;
  /** A password that must never appear anywhere on the audit tab. */
  secret: string;
} = {
  labId: '',
  switchId: '',
  switchName: '',
  alertId: '',
  incidentId: '',
  changeId: '',
  backupJobId: '',
  secret: 'correct-horse-battery-staple-9471',
};

async function finished(rpc: Rpc, jobId: string): Promise<string> {
  await expect
    .poll(async () => (await rpc<{ state: string }>('jobs.get', { id: jobId })).state, {
      timeout: 180_000,
      intervals: [500, 1_000, 2_000],
    })
    .not.toBe('running');
  return (await rpc<{ state: string }>('jobs.get', { id: jobId })).state;
}

test.beforeAll(async ({ request }) => {
  test.setTimeout(600_000);
  if (process.env.NOCTURNE_UI_DIST) {
    prebuilt = process.env.NOCTURNE_UI_DIST;
  } else {
    // Built outside `ui/dist`: the server crate embeds that directory, so
    // writing there would make cargo rebuild the binary first.
    uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
    await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
      cwd: UI_DIR,
      timeout: 240_000,
    });
  }
  backend = await startBackend({ uiDir: prebuilt ?? uiDir });
  const rpc = rpcFor(request, backend);

  // Nothing may move while a test is looking at it: the scheduler would poll
  // on its own, and the insight scan would write findings and audit rows.
  // `down_after: 1` is the one setting these tests change for their own sake:
  // the engine damps a device down over two failed polls in a row, and with
  // the scheduler off the only polls are the ones these tests ask for.
  await rpc('settings.set', {
    section: 'monitoring',
    value: { enabled: false, down_after: 1 },
  });
  await rpc('settings.set', { section: 'insights', value: { auto_scan: false } });

  // ---- a real lab with a real (simulated) switch
  const lab = await rpc<{ id: string }>('labs.create', { name: 'Ops' });
  world.labId = lab.id;
  await rpc('labs.node.add', { lab_id: lab.id, model: 'switch', name: 'SW1' });
  await rpc('labs.start', { id: lab.id });
  world.switchName = 'Ops/SW1';
  const devices = await rpc<{ id: string; name: string }[]>('devices.list');
  world.switchId = devices.find((device) => device.name === world.switchName)?.id ?? '';
  expect(world.switchId, 'the lab node is in the inventory').not.toBe('');

  // ---- a real change, planned, approved and applied to it
  const created = await rpc<{ change: { id: string } }>('changes.create', {
    title: 'Describe the uplink',
    ticket: 'NET-412',
    targets: [
      {
        device_id: world.switchId,
        intent: {
          type: 'lines',
          lines: ['interface FastEthernet0/1', 'description uplink to core', 'exit'],
        },
      },
    ],
    plan: true,
  });
  world.changeId = created.change.id;
  await expect
    .poll(async () => (await rpc<{ state: string }>('changes.get', { id: world.changeId })).state, {
      timeout: 120_000,
    })
    .toBe('planned');
  const planned = await rpc<{ targets: { plan: { plan_hash: string } }[] }>('changes.get', {
    id: world.changeId,
  });
  await rpc('changes.approve', {
    id: world.changeId,
    plan_hashes: planned.targets.map((target) => target.plan.plan_hash),
  });
  const applying = await rpc<{ job_id: string }>('changes.apply', { id: world.changeId });
  expect(await finished(rpc, applying.job_id)).toBe('succeeded');

  // ---- a real backup, so there is a real job with a real log
  const backup = await rpc<{ job_id: string }>('config.backup', {
    device_ids: [world.switchId],
    kinds: ['running'],
  });
  world.backupJobId = backup.job_id;
  await finished(rpc, backup.job_id);

  // ---- a real alert, from a real rule
  // The built-in rules deliberately exclude lab devices, so this is a rule of
  // its own that includes them; powering the node off and polling makes the
  // engine raise the alert. Nothing here calls `alerts.raise`.
  const rule = await rpc<{ id: string }>('alerts.rules.save', {
    name: 'Lab switch down',
    severity: 'critical',
    for_s: 0,
    scope: { sites: [], roles: [], tags: [], device_ids: [], include_lab: true },
    condition: { type: 'device_down' },
  });
  await rpc('labs.node.update', { lab_id: lab.id, node: 'SW1', powered: false });
  const poll = await rpc<{ job_id: string }>('monitor.poll_now', { device_ids: [world.switchId] });
  await finished(rpc, poll.job_id);
  expect(
    (await rpc<{ state: { reachable: boolean | null } }>('devices.get', { id: world.switchId }))
      .state.reachable,
    'the engine saw the node go down',
  ).toBe(false);
  await expect
    .poll(async () => (await rpc<{ id: string }[]>('alerts.list', { state: 'open' })).length, {
      timeout: 30_000,
    })
    .toBeGreaterThan(0);
  const alerts = await rpc<{ id: string; rule_id: string }[]>('alerts.list', { state: 'open' });
  const raised = alerts.find((alert) => alert.rule_id === rule.id);
  expect(raised, 'the rule raised the alert').toBeDefined();
  world.alertId = raised?.id ?? '';

  // ---- a credential with a real password, so the audit tab can be checked
  // for leaking one. Nothing in the trail may contain it.
  await rpc('credentials.create', {
    name: 'Campus switches',
    username: 'netops',
    password: world.secret,
    enable_secret: world.secret,
  });
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

// ------------------------------------------------------------------ helpers

function timelineRows(page: Page): Locator {
  return page.getByTestId('timeline-row');
}

/** The ids of the timeline rows on screen, in the order they are drawn. */
async function drawnOrder(page: Page): Promise<string[]> {
  return timelineRows(page).evaluateAll((rows) =>
    rows.map((row) => (row as HTMLElement).dataset.row ?? ''),
  );
}

function incidentUrl(id: string): string {
  return `/ops/incidents?incident=${encodeURIComponent(id)}`;
}

// -------------------------------------------------------------------- tests

test('the six tabs are the record, and every one of them opens', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/ops', { backend });

  const tabs = page.getByRole('tablist', { name: 'Operations' });
  for (const name of ['Incidents', 'Handover', 'Reports', 'Notes', 'Audit', 'Jobs']) {
    await expect(tabs.getByRole('tab', { name, exact: false })).toBeVisible();
  }

  // Each one is a route of its own, so each can be linked to and reloaded.
  for (const [tab, heading] of [
    ['handover', 'Incidents'],
    ['reports', 'What this build can report on'],
    ['notes', 'Notes'],
    ['jobs', 'Jobs'],
  ] as const) {
    await goTo(page, `/ops/${tab}`);
    await expect(page.getByText(heading).first()).toBeVisible({ timeout: 30_000 });
  }

  await goTo(page, '/ops/audit');
  await expect(page.getByTestId('audit-entry').first()).toBeVisible({ timeout: 30_000 });

  expect(console.messages).toEqual([]);
});

test('an incident opened from a real alert starts with that alert on its timeline', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const alert = await rpc<{ title: string; started_at: number; severity: string }>('alerts.get', {
    id: world.alertId,
  });

  await openApp(page, '/ops/incidents', { backend });
  await page.getByRole('button', { name: 'Open an incident' }).first().click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();

  // The alert is offered by the combobox; choosing it is the whole form.
  await dialog.getByRole('combobox', { name: 'Open alert' }).fill('Lab switch down');
  // The list is in a portal, so it is outside the dialog in the DOM.
  await page.getByRole('listbox', { name: 'Open alert' }).getByRole('option').first().click();
  await expect(dialog.getByText(alert.title, { exact: false })).toBeVisible();
  await dialog.getByRole('textbox', { name: 'Your name' }).fill('mo');
  await dialog.getByRole('button', { name: 'Open it' }).click();

  // It lands on the incident, which took the alert's title and severity.
  await expect(page.getByRole('heading', { level: 2, name: alert.title })).toBeVisible({
    timeout: 30_000,
  });
  await expect(page.getByText('Sev 1').first()).toBeVisible();

  const incidents = await rpc<{ incidents: { id: string; title: string }[] }>('incidents.list', {
    state: 'open',
  });
  const opened = incidents.incidents.find((incident) => incident.title === alert.title);
  expect(opened).toBeDefined();
  world.incidentId = opened?.id ?? '';

  // The alert's own event is on the timeline at the alert's own time, with
  // the evidence that says where it was read from.
  const record = await rpc<Incident>('incidents.get', { id: world.incidentId });
  const alertEntry = record.timeline.find((entry) => entry.kind === 'alert');
  expect(alertEntry, 'the alert came with its own history').toBeDefined();
  expect(alertEntry?.ts).toBe(alert.started_at);
  expect(alertEntry?.source.from).toBe('alerts.get');

  const row = page.locator(`[data-row="${alertEntry?.id}"]`);
  await expect(row).toBeVisible();
  await expect(row).toContainText(alert.title);
  await row.locator('details > summary').click();
  await expect(row).toContainText('alerts.get');

  expect(console.messages).toEqual([]);
});

test('the timeline interleaves notes, the alert and the change at the times they happened', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const alert = await rpc<{ started_at: number }>('alerts.get', { id: world.alertId });

  // Two notes about things that happened before anybody opened the record:
  // one before the alert fired, one after it. Their times are the point.
  await rpc('incidents.note', {
    id: world.incidentId,
    text: 'The optic was reseated five minutes before any of this.',
    ts: alert.started_at - 300_000,
    author: 'mo',
  });
  await rpc('incidents.note', {
    id: world.incidentId,
    text: 'Console answers; the uplink shows no light.',
    ts: alert.started_at + 60_000,
    author: 'mo',
  });
  // And the change that really was applied to this switch.
  await rpc('incidents.link', { id: world.incidentId, change_ids: [world.changeId], author: 'mo' });

  const record = await rpc<Incident>('incidents.get', { id: world.incidentId });
  const expected = [...record.timeline]
    .sort((a, b) => a.ts - b.ts || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
    .map((entry) => entry.id);
  // The engine's own answer has the four kinds in it; otherwise this test
  // would be asserting the order of a list of one thing.
  expect(new Set(record.timeline.map((entry) => entry.kind)).size).toBeGreaterThanOrEqual(3);
  expect(record.timeline.some((entry) => entry.kind === 'change')).toBe(true);

  await openApp(page, incidentUrl(world.incidentId), { backend });
  await expect(timelineRows(page).first()).toBeVisible({ timeout: 30_000 });
  await expect
    .poll(async () => (await drawnOrder(page)).filter((id) => expected.includes(id)).length)
    .toBe(expected.length);

  // The screen's order is the engine's order: by when it happened, and
  // nothing else.
  const drawn = (await drawnOrder(page)).filter((id) => expected.includes(id));
  expect(drawn).toEqual(expected);

  // The note about five minutes before the outage really is above the alert.
  const earliest = record.timeline[0];
  expect(earliest?.text).toContain('five minutes before');
  expect(drawn[0]).toBe(earliest?.id);

  expect(console.messages).toEqual([]);
});

test('a note typed into the box is on the record a moment later', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const before = await rpc<Incident>('incidents.get', { id: world.incidentId });

  await openApp(page, incidentUrl(world.incidentId), { backend });
  // Who is writing is remembered per browser, and this is a fresh one.
  await page.getByRole('textbox', { name: 'You', exact: true }).fill('mo');
  const box = page.getByRole('textbox', { name: 'Add a note to the timeline' });
  // Always on screen: no disclosure to open, nothing to scroll to.
  await expect(box).toBeVisible();
  await box.fill('Swapped to the spare optic on Gi1/0/48.');
  await page.getByRole('button', { name: 'Add note' }).click();

  await expect(page.getByText('Swapped to the spare optic on Gi1/0/48.')).toBeVisible({
    timeout: 30_000,
  });
  await expect(box).toHaveValue('');

  const after = await rpc<Incident>('incidents.get', { id: world.incidentId });
  expect(after.timeline).toHaveLength(before.timeline.length + 1);
  const added = after.timeline.find((entry) => entry.text.includes('spare optic'));
  expect(added?.kind).toBe('note');
  expect(added?.author).toBe('mo');
  expect(added?.source.from).toBe('operator');

  expect(console.messages).toEqual([]);
});

test('an unlinked alert inside the window is offered where it happened, and can be adopted', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  // A second real rule, so a second real alert fires on the same device from
  // the same (still powered-off) node: same facts, a different rule, a
  // different alert — and nobody has linked it to the incident.
  const second = await rpc<{ id: string }>('alerts.rules.save', {
    name: 'Lab switch unreachable',
    severity: 'high',
    for_s: 0,
    scope: { sites: [], roles: [], tags: [], device_ids: [], include_lab: true },
    condition: { type: 'device_down' },
  });
  const poll = await rpc<{ job_id: string }>('monitor.poll_now', { device_ids: [world.switchId] });
  await finished(rpc, poll.job_id);
  await expect
    .poll(
      async () =>
        (await rpc<{ rule_id: string }[]>('alerts.list', { state: 'open' })).filter(
          (alert) => alert.rule_id === second.id,
        ).length,
      { timeout: 30_000 },
    )
    .toBe(1);
  const alerts = await rpc<{ id: string; rule_id: string; started_at: number }[]>('alerts.list', {
    state: 'open',
  });
  const offer = alerts.find((alert) => alert.rule_id === second.id);
  expect(offer).toBeDefined();

  const suggested = await rpc<{ alerts: { id: string }[] }>('incidents.suggest_links', {
    id: world.incidentId,
  });
  expect(suggested.alerts.map((alert) => alert.id)).toContain(offer?.id);

  await openApp(page, incidentUrl(world.incidentId), { backend });
  const row = page.locator(`[data-row="alert:${offer?.id}"]`);
  await expect(row).toBeVisible({ timeout: 30_000 });
  // It is drawn as an offer, not as something that happened.
  await expect(row).toHaveAttribute('data-offered', '');
  await expect(row).toContainText('not linked');
  // And it sits in the timeline at its own time, not at the end of it.
  const record = await rpc<Incident>('incidents.get', { id: world.incidentId });
  const order = await drawnOrder(page);
  const later = record.timeline.filter((entry) => entry.ts > (offer?.started_at ?? 0));
  for (const entry of later) {
    expect(order.indexOf(`alert:${offer?.id}`)).toBeLessThan(order.indexOf(entry.id));
  }

  await row.getByRole('button', { name: 'Link it' }).click();
  await expect(page.locator(`[data-row="alert:${offer?.id}"]`)).toHaveCount(0, { timeout: 30_000 });

  const after = await rpc<Incident>('incidents.get', { id: world.incidentId });
  expect(after.alert_ids).toContain(offer?.id);
  // Linking brought the alert's own events onto the timeline.
  expect(after.timeline.some((entry) => entry.id.startsWith(`alert:${offer?.id}`))).toBe(true);

  expect(console.messages).toEqual([]);
});

test('every item of a generated handover links to the record it came from', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const since = Date.now() - 8 * 3_600_000;
  const generated = await rpc<Handover>('handover.generate', { since });

  await openApp(page, '/ops/handover', { backend });
  await expect(page.locator('[data-section="incidents"]')).toBeVisible({ timeout: 30_000 });

  const items = page.getByTestId('handover-item');
  await expect(items.first()).toBeVisible();
  const links = await items.evaluateAll((rows) =>
    rows.map((row) => row.querySelector('a')?.getAttribute('href') ?? null),
  );
  expect(links.length).toBeGreaterThan(2);
  // The contract of this tab: nothing is a sentence nobody can check.
  expect(links.filter((href) => href === null)).toEqual([]);
  for (const href of links) expect(href).toContain('#/');

  // The incident really is in it, and its link really opens it.
  const incidents = page.locator('[data-section="incidents"]');
  await expect(incidents.getByTestId('handover-item').first()).toBeVisible();
  const first = incidents.getByTestId('handover-item').first().getByRole('link');
  await expect(first).toHaveAttribute('href', /ops\/incidents\?incident=/);

  // A section the engine read and found empty says that, rather than nothing.
  expect(generated.jobs_failed).toEqual([]);
  await expect(page.locator('[data-section="jobs_failed"]')).toContainText(
    'No job failed in this period.',
  );

  // The change that was applied in the period is in the changes section, by
  // the title the change pipeline gave it.
  await expect(page.locator('[data-section="changes"]')).toContainText('Describe the uplink');

  // This build has an assistant, so the one thing it may touch is offered —
  // and only that one thing.
  await expect(page.getByRole('button', { name: 'Tidy my notes' })).toBeVisible();

  expect(console.messages).toEqual([]);
});

test('a report states what it covers, and counts the rows the engine counted', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  await openApp(page, '/ops/reports', { backend });
  await expect(page.getByText('What this build can report on')).toBeVisible({ timeout: 30_000 });

  const inventory = await rpc<{ rows: number; content: string }>('reports.generate', {
    kind: 'inventory',
    format: 'markdown',
  });
  await expect(page.getByText('This report describes the present')).toBeVisible();
  // The figure is the engine's; the wording is the screen's.
  await expect(page.getByText(new RegExp(`${inventory.rows} rows? of data\\.`))).toBeVisible();
  // The document on screen is the engine's, not this screen's rendering of it.
  await expect(page.getByText('# Inventory')).toBeVisible();
  await expect(page.getByText(world.switchName).first()).toBeVisible();

  // A period report with nothing in it says there was nothing to report —
  // which is a different sentence from a section that could not be read.
  await page.getByRole('combobox', { name: 'Report' }).selectOption('availability');
  await expect(page.getByText('This report covers the period in its header')).toBeVisible({
    timeout: 30_000,
  });

  expect(console.messages).toEqual([]);
});

test('the audit trail links what it mentions, cannot be edited, and shows no secret', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  await openApp(page, '/ops/audit', { backend });
  const entries = page.getByTestId('audit-entry');
  await expect(entries.first()).toBeVisible({ timeout: 30_000 });

  // The operations feature's own writes are in it, joined to the incident.
  const opened = page.locator('[data-action="incidents.create"]').first();
  await expect(opened).toBeVisible();
  await expect(opened.getByRole('link').last()).toHaveAttribute(
    'href',
    /ops\/incidents\?incident=/,
  );

  // Nothing in the record is editable: the only controls in it are the
  // disclosures that show what was recorded.
  const list = page.getByRole('list', { name: 'Recorded actions' });
  await expect(list.locator('input, textarea, select, [contenteditable="true"]')).toHaveCount(0);

  // A secret must never reach the trail. This does not mask one — masking
  // would hide the defect and leave the secret in the database and in every
  // export — it asserts that none is there.
  const page_text = (await page.locator('main').innerText()).toLowerCase();
  expect(page_text).not.toContain(world.secret);
  const everything = await rpc<AuditSearchResult>('audit.search', { limit: 1_000 });
  expect(JSON.stringify(everything)).not.toContain(world.secret);
  expect(everything.entries.some((entry) => entry.action === 'credential.save')).toBe(true);

  // The filter narrows it, in the engine, and the page says what it is showing.
  await page.getByRole('combobox', { name: 'Action' }).selectOption('incidents.');
  await expect
    .poll(async () => entries.count())
    .toBeLessThan(everything.entries.length);
  for (const action of await entries.evaluateAll((rows) =>
    rows.map((row) => (row as HTMLElement).dataset.action ?? ''),
  )) {
    expect(action.startsWith('incidents.')).toBe(true);
  }

  expect(console.messages).toEqual([]);
});

test('the job history keeps the backup that ran, with its log', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const job = await rpc<{ title: string; kind: string; log: { message: string }[] }>('jobs.get', {
    id: world.backupJobId,
  });

  await openApp(page, `/ops/jobs?job=${world.backupJobId}`, { backend });
  const drawer = page.getByRole('dialog');
  await expect(drawer).toBeVisible({ timeout: 30_000 });
  await expect(drawer).toContainText(job.kind);
  await expect(drawer).toContainText(world.backupJobId);
  if (job.log.length > 0) {
    await expect(drawer.getByText(job.log[0]?.message ?? '', { exact: false }).first()).toBeVisible();
  }

  // Closing it goes back to the history, which has the job in it.
  await page.keyboard.press('Escape');
  await expect(page.getByRole('table', { name: 'Jobs' })).toContainText(job.kind);

  expect(console.messages).toEqual([]);
});

test('a build without the alerting and change features says so rather than showing dead controls', async ({
  page,
}) => {
  const console = watchConsole(page);

  // The one fault injection in this file: `app.methods` is the list the
  // screen gates on, and a build assembled without those feature crates
  // cannot be produced from the outside. Everything else still answers.
  await page.route('**/api/rpc', async (route) => {
    const body = route.request().postDataJSON() as { method?: string };
    if (body.method !== 'app.methods') return route.continue();
    const response = await route.fetch();
    const payload = (await response.json()) as { ok: boolean; result?: string[] };
    if (payload.ok && payload.result) {
      payload.result = payload.result.filter(
        (method) =>
          !method.startsWith('alerts.') &&
          !method.startsWith('changes.') &&
          !method.startsWith('handover.') &&
          !method.startsWith('ai.'),
      );
    }
    await route.fulfill({ response, json: payload });
  });

  await openApp(page, '/ops/incidents', { backend });
  const tabs = page.getByRole('tablist', { name: 'Operations' });
  await expect(tabs.getByRole('tab', { name: 'Incidents' })).toBeVisible();
  // The tab whose feature is gone is not offered at all.
  await expect(tabs.getByRole('tab', { name: 'Handover' })).toHaveCount(0);
  // The ones that do not depend on it are untouched.
  await expect(tabs.getByRole('tab', { name: 'Audit' })).toBeVisible();
  await expect(tabs.getByRole('tab', { name: 'Jobs' })).toBeVisible();

  // Reaching it by its address says what is missing, in one sentence.
  await goTo(page, '/ops/handover');
  await expect(page.getByText('Handover is not in this build')).toBeVisible();

  // Opening an incident from an alert is not offered either, but opening one
  // by hand still is.
  await goTo(page, '/ops/incidents');
  await page.getByRole('button', { name: 'Open an incident' }).first().click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('combobox', { name: 'Open alert' })).toHaveCount(0);
  await expect(dialog.getByRole('textbox', { name: 'What is wrong' })).toBeVisible();
  await page.keyboard.press('Escape');

  // The incident page itself still works: its own records do not depend on
  // the features that are gone.
  await goTo(page, incidentUrl(world.incidentId));
  await expect(page.getByTestId('timeline-row').first()).toBeVisible({ timeout: 30_000 });

  expect(console.messages).toEqual([]);
});

test('a build with no assistant does not offer to tidy anything', async ({ page }) => {
  const console = watchConsole(page);

  // The same injection, narrowed to the assistant: every other tab is whole,
  // so this proves the tabs degrade one feature at a time.
  await page.route('**/api/rpc', async (route) => {
    const body = route.request().postDataJSON() as { method?: string };
    if (body.method !== 'app.methods') return route.continue();
    const response = await route.fetch();
    const payload = (await response.json()) as { ok: boolean; result?: string[] };
    if (payload.ok && payload.result) {
      payload.result = payload.result.filter((method) => !method.startsWith('ai.'));
    }
    await route.fulfill({ response, json: payload });
  });

  await openApp(page, '/ops/handover', { backend });
  await expect(page.locator('[data-section="incidents"]')).toBeVisible({ timeout: 30_000 });
  await expect(page.getByRole('button', { name: 'Tidy my notes' })).toHaveCount(0);
  // The notes box itself is still there: the prose is a person's either way.
  await expect(page.getByRole('textbox', { name: 'Notes for the next shift' })).toBeVisible();

  expect(console.messages).toEqual([]);
});
