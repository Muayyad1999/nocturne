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
 *
 * **Evidence** (the second half of this file) is tested the same way. Every
 * item is made by the engine: a terminal capture of a real session on a
 * second lab switch, the engine's own drawing of the topology, the samples
 * the monitor really stored, and text handed in through `evidence.add_text`.
 * The injections made there are marked where they happen, and there are
 * four kinds of them:
 *
 * - `app.methods` without `evidence.*`, for a build assembled without it;
 * - **what is stored for an item is replaced in the engine's own table**
 *   (`storeBody`). A stored drawing is hostile input by contract, but the
 *   engine's own drawing never contains anything that runs, so there is no
 *   call that makes it store one. The item is real and so is everything
 *   after the write: `evidence.get` and `incidents.export` of the running
 *   engine answer with what is stored, and the browser is shown that;
 * - one capture answered `unavailable` in the engine's own recorded words: a
 *   full build has every feature a capture reads from, so it never says so;
 * - `URL.createObjectURL` made to throw, for a window that cannot hand a
 *   file to the browser.
 */

import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Locator, type Page } from '@playwright/test';
import type {
  AuditSearchResult,
  Evidence,
  EvidenceListResult,
  EvidenceWithBody,
  Handover,
  Incident,
} from '../src/api/types/ops';
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
  /** A second lab switch, powered on, for the evidence cases. */
  sw2Id: string;
  /** A terminal session on it, opened through the RPC surface. */
  sessionId: string;
  /** The terminal capture attached from the timeline. */
  captureId: string;
  /** The operator-supplied text with control characters in it. */
  textId: string;
} = {
  sw2Id: '',
  sessionId: '',
  captureId: '',
  textId: '',
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

test('the seven tabs are the record, and every one of them opens', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/ops', { backend });

  const tabs = page.getByRole('tablist', { name: 'Operations' });
  for (const name of ['Incidents', 'Handover', 'Reports', 'Notes', 'Evidence', 'Audit', 'Jobs']) {
    await expect(tabs.getByRole('tab', { name, exact: false })).toBeVisible();
  }

  // Each one is a route of its own, so each can be linked to and reloaded.
  for (const [tab, heading] of [
    ['handover', 'Incidents'],
    ['reports', 'What this build can report on'],
    ['notes', 'Notes'],
    ['evidence', 'No evidence has been kept yet'],
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

// ================================================================== evidence

/** A PNG starts with these eight bytes, and its size is in the header that follows. */
function pngSize(file: Buffer): { width: number; height: number } {
  expect([...file.subarray(0, 8)]).toEqual([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  return { width: file.readUInt32BE(16), height: file.readUInt32BE(20) };
}

function evidenceUrl(id: string): string {
  return `/ops/evidence?item=${encodeURIComponent(id)}`;
}

/** Leaves `evidence.*` out of `app.methods`: a build assembled without it. */
async function withoutEvidence(page: Page): Promise<void> {
  await page.route('**/api/rpc', async (route) => {
    const body = route.request().postDataJSON() as { method?: string };
    if (body.method !== 'app.methods') return route.continue();
    const response = await route.fetch();
    const payload = (await response.json()) as { ok: boolean; result?: string[] };
    if (payload.ok && payload.result) {
      payload.result = payload.result.filter((method) => !method.startsWith('evidence.'));
    }
    await route.fulfill({ response, json: payload });
  });
}

test('a capture from a lab session is attached from the timeline and kept with its source and hash', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  // ---- a second lab switch, powered on (the first was powered off to raise
  // the alert), and a real terminal session on it with something printed.
  await rpc('labs.node.add', { lab_id: world.labId, model: 'switch', name: 'SW2' });
  const devices = await rpc<{ id: string; name: string }[]>('devices.list');
  world.sw2Id = devices.find((device) => device.name === 'Ops/SW2')?.id ?? '';
  expect(world.sw2Id, 'the second lab node is in the inventory').not.toBe('');
  const session = await rpc<{ id: string; lab: boolean }>('terminal.open', { device_id: world.sw2Id });
  world.sessionId = session.id;
  expect(session.lab).toBe(true);
  await rpc('terminal.input', { session_id: session.id, data: 'show version\r' });
  await expect
    .poll(
      async () => (await rpc<{ text: string }>('terminal.snapshot', { session_id: session.id, lines: 60 })).text,
      { timeout: 30_000 },
    )
    .toContain('Processor board ID');

  // ---- from the incident's timeline
  await openApp(page, incidentUrl(world.incidentId), { backend });
  await page.getByRole('textbox', { name: 'You', exact: true }).fill('mo');
  await page.getByRole('button', { name: 'Attach evidence…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Attach evidence' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('tab', { name: 'From a terminal session' }).click();
  await dialog.getByRole('combobox', { name: 'Session' }).selectOption(session.id);
  await dialog.getByRole('spinbutton', { name: 'Last lines' }).fill('60');
  await dialog.getByRole('textbox', { name: 'Title' }).fill('What SW2 printed');
  await dialog.getByRole('button', { name: 'Capture and attach' }).click();
  await expect(dialog).toHaveCount(0, { timeout: 30_000 });

  // ---- the engine stored it: read by `terminal.snapshot`, from that session,
  // labelled lab, and hashed as stored.
  const listed = await rpc<EvidenceListResult>('evidence.list', { incident_id: world.incidentId });
  expect(listed.items).toHaveLength(1);
  const head = listed.items[0] as Evidence;
  world.captureId = head.id;
  expect(head.title).toBe('What SW2 printed');
  expect(head.kind).toBe('terminal');
  expect(head.source.from).toBe('terminal.snapshot');
  expect(head.source.session_id).toBe(session.id);
  expect(head.source.lab).toBe(true);
  expect(head.redacted).toBe(true);
  const stored = await rpc<EvidenceWithBody>('evidence.get', { id: head.id });
  expect(stored.body.type).toBe('text');
  const text = stored.body.type === 'text' ? stored.body.text : '';
  expect(text).toContain('Processor board ID');
  expect(createHash('sha256').update(text).digest('hex')).toBe(stored.sha256);

  // ---- on the timeline: a reference at the capture time, never the text
  const record = await rpc<Incident>('incidents.get', { id: world.incidentId });
  const entry = record.timeline.find((candidate) => candidate.id === `evidence:${head.id}`);
  expect(entry?.kind).toBe('evidence');
  expect(entry?.ts).toBe(head.captured_at);
  expect(entry?.ref).toEqual({ type: 'evidence', id: head.id });
  expect(entry?.author).toBe('mo');
  expect(JSON.stringify(record)).not.toContain('Processor board ID');

  const row = page.locator(`[data-row="evidence:${head.id}"]`);
  await expect(row).toBeVisible({ timeout: 30_000 });
  await expect(row).toHaveAttribute('data-kind', 'evidence');
  await expect(row).toContainText('What SW2 printed');
  await expect(row).not.toContainText('Processor board ID');

  // ---- the entry opens the item, which shows what was printed and where
  // it came from
  await row.getByRole('link', { name: 'What SW2 printed' }).click();
  await expect(page.getByTestId('evidence-title')).toHaveText('What SW2 printed', { timeout: 30_000 });
  await expect(page.getByTestId('evidence-text')).toContainText('Processor board ID');
  await expect(page.getByTestId('evidence-source')).toContainText(
    'Read by the engine from the terminal session “Ops/SW2”',
  );
  await expect(page.getByTestId('evidence-sha256')).toHaveText(stored.sha256);
  await expect(page.getByText('LAB').first()).toBeVisible();
  await expect(page.getByTestId('evidence-incident')).toContainText(record.title);

  // The file that "Download as stored" hands over is the stored text byte
  // for byte: its hash is the hash on screen.
  const saving = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Download as stored' }).click();
  const saved = await saving;
  expect(saved.suggestedFilename()).toBe(`what-sw2-printed-${head.id}.txt`);
  const file = await readFile(await saved.path());
  expect(createHash('sha256').update(file).digest('hex')).toBe(stored.sha256);

  // ---- and under Evidence, with the lab label and what it is attached to
  await goTo(page, '/ops/evidence');
  const listRow = page.locator(`[data-evidence="${head.id}"]`);
  await expect(listRow).toBeVisible({ timeout: 30_000 });
  const tableRow = page.getByRole('row').filter({ has: listRow });
  await expect(tableRow).toContainText('LAB');
  await expect(tableRow).toContainText('Terminal');
  await expect(tableRow).toContainText(record.title);
  await expect(page.getByTestId('evidence-coverage-line')).toContainText('All 1 item that are kept.');

  // ---- the incident's own panel counts it and links to exactly these items
  await goTo(page, incidentUrl(world.incidentId));
  await expect(page.getByRole('link', { name: '1 item attached' })).toHaveAttribute(
    'href',
    new RegExp(`ops/evidence\\?attached=${world.incidentId}$`),
  );

  // ---- and the handover of this shift lists it: the record, not the text
  await goTo(page, '/ops/handover');
  const section = page.locator('[data-section="evidence"]');
  await expect(section).toBeVisible({ timeout: 30_000 });
  const line = section.getByTestId('handover-item').filter({ hasText: 'What SW2 printed' });
  await expect(line.getByRole('link')).toHaveAttribute('href', new RegExp(`ops/evidence\\?item=${head.id}$`));
  await expect(line).toContainText('terminal.snapshot');
  await expect(line).toContainText(`sha256 ${stored.sha256.slice(0, 12)}`);
  await expect(section).not.toContainText('Processor board ID');

  expect(console.messages).toEqual([]);
});

test('text is shown as it was printed, with its control characters visible and obeyed by nothing', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  // What a device can print: a clear-screen and a colour, a bell, a line
  // overwritten with a carriage return, a direction override — and markup,
  // which is only text here.
  const printed = [
    'SW2#\u001b[2J\u001b[31mshow log\u001b[0m\u0007',
    'no shutdown\rshutdown   ',
    'permit ip any any \u202eyna yna pi yned',
    '<script>window.__evidence_pwned = "text"</script><img src=x onerror="window.__evidence_pwned = \'img\'">',
  ].join('\n');
  const item = await rpc<EvidenceWithBody>('evidence.add_text', {
    text: printed,
    title: 'Read out over the phone',
    note: 'ticket NET-431',
  });
  world.textId = item.id;
  expect(item.source.from).toBe('operator');
  // Stored as it was sent: the escape character is in the item.
  expect(item.body.type === 'text' && item.body.text).toContain('\u001b[31m');

  await openApp(page, evidenceUrl(item.id), { backend });
  const text = page.getByTestId('evidence-text');
  await expect(text).toBeVisible({ timeout: 30_000 });

  // Each control character is a picture of itself, in the page's own text.
  const shown = await text.locator('pre').innerText();
  expect(shown).toContain('SW2#\u241b[2J\u241b[31mshow log\u241b[0m\u2407');
  expect(shown).toContain('no shutdown\u240dshutdown');
  expect(shown).toContain('permit ip any any \ufffdyna yna pi yned');
  expect(shown).not.toContain('\u001b');
  expect(shown).not.toContain('\u202e');
  // The markup is on screen as the characters it is made of.
  expect(shown).toContain('<script>window.__evidence_pwned = "text"</script>');
  await expect(page.getByTestId('evidence-pictured')).toContainText('control characters');

  // Nothing ran, nothing of it became an element, and it was not written
  // into a terminal emulator, which would have obeyed the escape sequences.
  expect(await page.evaluate(() => (window as { __evidence_pwned?: string }).__evidence_pwned)).toBeUndefined();
  await expect(text.locator('script, img')).toHaveCount(0);
  await expect(page.locator('.xterm')).toHaveCount(0);

  // Text a person handed in says so, in the header and in its source.
  await expect(page.getByText('Supplied by a person').first()).toBeVisible();
  await expect(page.getByTestId('evidence-source')).toContainText('Supplied by a person');
  await expect(page.getByText('ticket NET-431')).toBeVisible();

  expect(console.messages).toEqual([]);
});

test('the engine’s drawing is shown as an image on a data address, and saved as a PNG of it', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  // ---- the engine's own drawing of the topology, kept by the engine
  const kept = await rpc<EvidenceWithBody>('evidence.capture_topology', { title: 'The lab as drawn' });
  expect(kept.source.from).toBe('topology.export');
  expect(kept.source.lab).toBe(true);
  const svg = kept.body.type === 'svg' ? kept.body.svg : '';
  expect(svg).toContain('<svg');
  expect(createHash('sha256').update(svg).digest('hex')).toBe(kept.sha256);

  await openApp(page, evidenceUrl(kept.id), { backend });
  const drawing = page.getByTestId('evidence-drawing');
  await expect(drawing).toBeVisible({ timeout: 30_000 });
  // It really is a picture of the drawing: an <img> on a data: address that
  // decoded to something with a size.
  expect(await drawing.evaluate((img: HTMLImageElement) => img.tagName)).toBe('IMG');
  expect(await drawing.getAttribute('src')).toMatch(/^data:image\/svg\+xml;base64,/);
  await expect
    .poll(async () => drawing.evaluate((img: HTMLImageElement) => img.complete && img.naturalWidth))
    .toBeGreaterThan(100);
  // The address holds the stored drawing, exactly.
  const address = (await drawing.getAttribute('src')) ?? '';
  expect(Buffer.from(address.slice(address.indexOf(',') + 1), 'base64').toString('utf8')).toBe(svg);

  // A PNG of it can be saved, and it is a PNG of that size or larger.
  const saving = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Save as PNG' }).click();
  const png = await saving;
  expect(png.suggestedFilename()).toBe(`the-lab-as-drawn-${kept.id}.png`);
  const natural = await drawing.evaluate((img: HTMLImageElement) => ({ width: img.naturalWidth, height: img.naturalHeight }));
  const size = pngSize(await readFile(await png.path()));
  expect(size.width).toBe(natural.width * 2);
  expect(size.height).toBe(natural.height * 2);

  // What a stored drawing can carry, and what must then not happen, is the
  // case of its own further down: `a stored drawing that carries a script …`.
  expect(console.messages).toEqual([]);
});

test('a chart is drawn again from the stored points, with its gap left as a gap and said in words', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  // Real samples with a real hole in them: three polls two seconds apart, a
  // pause in which nothing is recorded, then two more. The scheduler is off,
  // so these polls are the only samples there are. (Two seconds, because the
  // store keeps one point per second: polls any closer would be one sample.)
  const pollOnce = async () => {
    const poll = await rpc<{ job_id: string }>('monitor.poll_now', { device_ids: [world.sw2Id] });
    await finished(rpc, poll.job_id);
    await page.waitForTimeout(2_100);
  };
  const since = Date.now() - 60_000;
  for (let index = 0; index < 3; index += 1) await pollOnce();
  await page.waitForTimeout(9_000);
  for (let index = 0; index < 2; index += 1) await pollOnce();
  const until = Date.now() + 1_000;

  const kept = await rpc<EvidenceWithBody>('evidence.capture_chart', {
    series: [{ device_id: world.sw2Id, metric: 'up' }],
    since,
    until,
    step_s: 1,
    title: 'SW2 reachability around the pause',
  });
  expect(kept.body.type).toBe('chart');
  const body = kept.body.type === 'chart' ? kept.body : null;
  const points = (body?.series[0]?.points ?? []) as [number, number][];
  expect(points.length).toBeGreaterThanOrEqual(5);
  // The engine's own coverage says there is a stretch with nothing in it.
  const coverage = body?.coverage.series[0];
  expect(coverage?.points).toBe(points.length);
  expect(coverage?.longest_gap_ms ?? 0).toBeGreaterThan(6_000);
  expect(kept.source.lab).toBe(true);

  await openApp(page, evidenceUrl(kept.id), { backend });
  const figure = page.locator('figure').filter({ hasText: 'SW2 reachability around the pause' });
  await expect(figure.locator('canvas')).toBeVisible({ timeout: 30_000 });

  // The hole is said, not only drawn.
  await expect(page.getByTestId('evidence-gaps')).toContainText('nothing was recorded then');
  // And the coverage is the engine's: its count, its device, its lab label.
  const stated = page.getByTestId('evidence-coverage');
  await expect(stated).toContainText('Ops/SW2: up');
  await expect(stated).toContainText(`${points.length} points`);
  await expect(stated).toContainText('LAB');
  await expect(stated).toContainText('longest stretch between two of them');

  // The numbers behind it: one row per stored point and not one more — the
  // time the chart puts inside the hole to lift its pen is not a sample —
  // and no value that was not stored.
  await figure.getByRole('button', { name: 'Export this chart' }).click();
  const savingCsv = page.waitForEvent('download');
  await page.getByRole('menuitem', { name: 'Save numbers as CSV' }).click();
  const csvFile = await savingCsv;
  expect(csvFile.suggestedFilename()).toMatch(/^sw2-reachability-around-the-pause-.*\.csv$/);
  const csv = (await readFile(await csvFile.path(), 'utf8')).trimEnd().split('\r\n');
  expect(csv[0]).toBe('time_utc,up');
  expect(csv.slice(1).map((row) => row.split(','))).toEqual(
    points.map(([at, value]) => [new Date(at).toISOString(), String(value)]),
  );

  // A picture: the plot with its title, in the theme on screen.
  await figure.getByRole('button', { name: 'Export this chart' }).click();
  const savingPng = page.waitForEvent('download');
  await page.getByRole('menuitem', { name: 'Save as PNG' }).click();
  const pngFile = await savingPng;
  expect(pngFile.suggestedFilename()).toMatch(/\.png$/);
  const picture = pngSize(await readFile(await pngFile.path()));
  const plot = await figure.locator('canvas').evaluate((canvas: HTMLCanvasElement) => ({
    width: canvas.width,
    height: canvas.height,
  }));
  // Wider and taller than the plot alone: the title and the note are on it.
  expect(picture.width).toBeGreaterThan(plot.width);
  expect(picture.height).toBeGreaterThan(plot.height);

  expect(console.messages).toEqual([]);
});

test('renaming follows onto the timeline, and deleting leaves a tombstone that says what it is', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const before = await rpc<EvidenceWithBody>('evidence.get', { id: world.captureId });
  const incident = await rpc<Incident>('incidents.get', { id: world.incidentId });

  await openApp(page, evidenceUrl(world.captureId), { backend });
  await expect(page.getByTestId('evidence-title')).toHaveText('What SW2 printed', { timeout: 30_000 });

  // ---- rename
  await page.getByRole('button', { name: 'Rename…' }).click();
  const rename = page.getByRole('dialog', { name: 'Rename this evidence' });
  await expect(rename).toContainText('timeline entry that points at it will follow');
  await rename.getByRole('textbox', { name: 'Title' }).fill('SW2 show version, before the reload');
  await rename.getByRole('button', { name: 'Rename' }).click();
  await expect(page.getByTestId('evidence-title')).toHaveText('SW2 show version, before the reload', {
    timeout: 30_000,
  });
  const renamed = await rpc<Incident>('incidents.get', { id: world.incidentId });
  expect(renamed.timeline.find((entry) => entry.id === `evidence:${world.captureId}`)?.text).toBe(
    'SW2 show version, before the reload',
  );
  // The content did not move.
  await expect(page.getByTestId('evidence-sha256')).toHaveText(before.sha256);

  // ---- delete: the confirmation names the incident that keeps a tombstone
  await page.getByRole('button', { name: 'Delete…' }).click();
  const confirm = page.getByRole('dialog', { name: 'Delete this evidence?' });
  await expect(confirm).toContainText(`“${incident.title}” will keep a line on its timeline`);
  await expect(confirm).toContainText('tombstone');
  await confirm.getByRole('button', { name: 'Delete it' }).click();
  await expect(page.getByText('1 incident keeps a line saying it was there')).toBeVisible({ timeout: 30_000 });

  // The item and its content are gone from the engine …
  await expect(rpc('evidence.get', { id: world.captureId })).rejects.toThrow(/not_found/);
  // … and the incident keeps the entry, at the same time, without a reference.
  const after = await rpc<Incident>('incidents.get', { id: world.incidentId });
  const tombstone = after.timeline.find((entry) => entry.id === `evidence:${world.captureId}`);
  expect(tombstone?.ref).toBeUndefined();
  expect(tombstone?.ts).toBe(before.captured_at);
  expect(tombstone?.source.from).toBe('evidence.delete');
  expect(tombstone?.source.evidence).toContain(before.sha256);

  // ---- on the timeline it is drawn as what it is
  await goTo(page, incidentUrl(world.incidentId));
  const row = page.locator(`[data-row="evidence:${world.captureId}"]`);
  await expect(row).toBeVisible({ timeout: 30_000 });
  await expect(row).toHaveAttribute('data-tombstone', '');
  await expect(row).toContainText(tombstone?.text ?? 'evidence deleted on');
  await expect(row).toContainText('evidence deleted on');
  await expect(row).toContainText('Deleted');
  // Not a link, and nothing to detach.
  await expect(row.getByRole('link')).toHaveCount(0);
  await expect(row.getByRole('button', { name: 'Detach' })).toHaveCount(0);
  await expect(page.getByText('1 deleted item')).toBeVisible();
  // The hash of what it was is still behind the disclosure.
  await row.locator('details > summary').click();
  await expect(row).toContainText(before.sha256);

  // ---- and the address of the deleted item says it is gone
  await goTo(page, evidenceUrl(world.captureId));
  await expect(page.getByText('That evidence is not there')).toBeVisible({ timeout: 30_000 });

  expect(console.messages).toEqual([]);
});

test('an item is attached from its own page and detached from the timeline, and is still kept', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const incident = await rpc<Incident>('incidents.get', { id: world.incidentId });

  await openApp(page, evidenceUrl(world.textId), { backend });
  await expect(page.getByTestId('evidence-title')).toHaveText('Read out over the phone', { timeout: 30_000 });
  await expect(page.getByText('No incident. It is kept on its own')).toBeVisible();

  await page.getByRole('button', { name: 'Attach to an incident…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Attach to an incident' });
  await dialog.getByRole('combobox', { name: 'Incident' }).selectOption(world.incidentId);
  await dialog.getByRole('textbox', { name: 'Your name' }).fill('zahra');
  await dialog.getByRole('button', { name: 'Attach', exact: true }).click();
  await expect(page.getByTestId('evidence-incident')).toContainText(incident.title, { timeout: 30_000 });

  const attached = await rpc<Incident>('incidents.get', { id: world.incidentId });
  const entry = attached.timeline.find((candidate) => candidate.id === `evidence:${world.textId}`);
  expect(entry?.ref).toEqual({ type: 'evidence', id: world.textId });
  expect(entry?.author).toBe('zahra');

  // ---- detach it where it is seen: on the timeline
  await goTo(page, incidentUrl(world.incidentId));
  const row = page.locator(`[data-row="evidence:${world.textId}"]`);
  await expect(row).toBeVisible({ timeout: 30_000 });
  await row.getByRole('button', { name: 'Detach' }).click();
  await expect(row).toHaveCount(0, { timeout: 30_000 });

  // The entry is gone from the incident; the item is not gone at all.
  const detached = await rpc<Incident>('incidents.get', { id: world.incidentId });
  expect(detached.timeline.some((candidate) => candidate.id === `evidence:${world.textId}`)).toBe(false);
  const item = await rpc<EvidenceWithBody>('evidence.get', { id: world.textId });
  expect(item.incident_ids).toEqual([]);
  expect(item.body.type).toBe('text');

  expect(console.messages).toEqual([]);
});

test('the list filters and searches in the engine, pages by its cursor, and says what it is a page of', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  // Enough items for a second page, each a real stored item.
  for (let index = 1; index <= 52; index += 1) {
    await rpc('evidence.add_text', {
      text: `interface GigabitEthernet0/${index} is up`,
      title: `port check ${String(index).padStart(2, '0')}`,
    });
  }
  const everything: Evidence[] = [];
  for (let cursor: string | undefined; ; ) {
    const result: EvidenceListResult = await rpc<EvidenceListResult>('evidence.list', { limit: 200, cursor });
    everything.push(...result.items);
    cursor = result.next_cursor;
    if (!cursor) break;
  }
  expect(everything.length).toBeGreaterThan(50);

  await openApp(page, '/ops/evidence', { backend });
  const rows = page.getByTestId('evidence-row');
  await expect(rows.first()).toBeVisible({ timeout: 30_000 });
  const drawn = () => rows.evaluateAll((links) => links.map((link) => (link as HTMLElement).dataset.evidence ?? ''));

  // Newest first, in the engine's order, and the first page is a page.
  await expect(rows).toHaveCount(50);
  expect(await drawn()).toEqual(everything.slice(0, 50).map((item) => item.id));
  const coverage = page.getByTestId('evidence-coverage-line');
  await expect(coverage).toContainText('The newest 50 items; older ones exist and are not shown.');

  // The next page is the engine's next page, and says where it stands.
  await page.getByRole('button', { name: 'Older evidence' }).click();
  await expect(rows).toHaveCount(everything.length - 50, { timeout: 30_000 });
  expect(await drawn()).toEqual(everything.slice(50).map((item) => item.id));
  await expect(coverage).toContainText('the oldest there are');
  await page.getByRole('button', { name: 'Back to the newest' }).click();
  await expect(rows).toHaveCount(50, { timeout: 30_000 });

  // ---- by kind: only the drawings
  await page.getByRole('combobox', { name: 'Kind' }).selectOption('topology');
  const drawings = everything.filter((item) => item.kind === 'topology');
  expect(drawings.length).toBeGreaterThan(0);
  await expect(rows).toHaveCount(drawings.length, { timeout: 30_000 });
  expect(await drawn()).toEqual(drawings.map((item) => item.id));
  await expect(coverage).toContainText(`All ${drawings.length} item`);
  await expect(coverage).toContainText('that match');
  await page.getByRole('combobox', { name: 'Kind' }).selectOption('');

  // ---- by device: what was read from the second switch
  await page.getByRole('combobox', { name: 'Device' }).selectOption(world.sw2Id);
  const ofDevice = await rpc<EvidenceListResult>('evidence.list', { device_id: world.sw2Id, limit: 200 });
  expect(ofDevice.items.length).toBeGreaterThan(0);
  await expect(rows).toHaveCount(ofDevice.items.length, { timeout: 30_000 });
  expect(await drawn()).toEqual(ofDevice.items.map((item) => item.id));
  await page.getByRole('combobox', { name: 'Device' }).selectOption('');

  // ---- the search is the engine's: titles, not content
  await page.getByRole('searchbox', { name: 'Search evidence' }).fill('port check 07');
  await expect(rows).toHaveCount(1, { timeout: 30_000 });
  await expect(rows.first()).toHaveText('port check 07');
  // Every one of those items contains this word, and none is found by it.
  await page.getByRole('searchbox', { name: 'Search evidence' }).fill('GigabitEthernet0');
  await expect(page.getByText('Nothing matches that')).toBeVisible({ timeout: 30_000 });
  await expect(coverage).toContainText('not the content of an item');
  await page.getByRole('button', { name: 'Clear the filters' }).click();
  await expect(rows).toHaveCount(50, { timeout: 30_000 });

  expect(console.messages).toEqual([]);
});

test('a capture the engine refuses is answered with its own sentence, and nothing is stored', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const count = async () => (await rpc<EvidenceListResult>('evidence.list', { limit: 200 })).items.length;
  const before = await count();

  await openApp(page, incidentUrl(world.incidentId), { backend });
  await page.getByRole('button', { name: 'Attach evidence…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Attach evidence' });

  // ---- a session that is gone by the time the capture is asked for. The
  // engine cannot read it and says so; this screen adds nothing to that.
  const gone = await rpc<{ id: string }>('terminal.open', { device_id: world.sw2Id });
  await dialog.getByRole('tab', { name: 'From a terminal session' }).click();
  await expect(dialog.getByRole('combobox', { name: 'Session' }).locator(`option[value="${gone.id}"]`)).toHaveCount(
    1,
    { timeout: 30_000 },
  );
  await dialog.getByRole('combobox', { name: 'Session' }).selectOption(gone.id);
  await rpc('terminal.close', { session_id: gone.id });
  await dialog.getByRole('button', { name: 'Capture and attach' }).click();
  const refused = dialog.getByTestId('attach-refused');
  await expect(refused).toContainText(`terminal session \`${gone.id}\` was not found`, { timeout: 30_000 });
  await expect(refused).toContainText('Nothing was stored');
  expect(await count()).toBe(before);

  // ---- text past the limit: refused whole, with what to do, not cut
  await dialog.getByRole('tab', { name: 'Text I hold' }).click();
  await expect(dialog.getByText('supplied by a person')).toBeVisible();
  await dialog.getByRole('textbox', { name: 'Title' }).fill('a whole show tech');
  // Put in at once, as a paste is: typed a key at a time, a quarter of a
  // megabyte takes the test runner minutes.
  await dialog.getByRole('textbox', { name: 'Text' }).evaluate((area: HTMLTextAreaElement, pasted) => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')?.set?.call(area, pasted);
    area.dispatchEvent(new Event('input', { bubbles: true }));
  }, 'interface Gi0/1 is up\n'.repeat(12_000));
  await dialog.getByRole('button', { name: 'Keep and attach' }).click();
  await expect(refused).toContainText('an evidence item holds at most 256 KiB', { timeout: 30_000 });
  await expect(refused).toContainText('Nothing was stored');
  // The dialog is still open with the text in it, and nothing was kept or attached.
  await expect(dialog.getByRole('textbox', { name: 'Title' })).toHaveValue('a whole show tech');
  expect(await count()).toBe(before);
  await page.keyboard.press('Escape');

  expect(console.messages).toEqual([]);
});

test('the map is saved as a PNG of the engine’s drawing, and kept as evidence by the engine', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const before = (await rpc<EvidenceListResult>('evidence.list', { kind: 'topology', limit: 200 })).items.length;

  await openApp(page, '/topology', { backend });
  await page.getByRole('button', { name: 'Export this drawing' }).click({ timeout: 30_000 });
  const dialog = page.getByRole('dialog', { name: 'Export this drawing' });
  await expect(dialog).toBeVisible();

  // ---- PNG: made here from the engine's SVG, and the dialog says what it is
  await dialog.getByRole('combobox', { name: 'Format' }).selectOption('png');
  await expect(dialog.getByTestId('export-not-a-screenshot')).toContainText(
    'the engine’s drawing of the same graph, not a pixel copy of the screen',
  );
  const preview = dialog.getByRole('img', { name: 'The drawing that will be saved' });
  await expect(preview).toBeVisible({ timeout: 30_000 });
  expect(await preview.getAttribute('src')).toMatch(/^data:image\/svg\+xml;base64,/);
  await expect
    .poll(async () => preview.evaluate((img: HTMLImageElement) => (img.complete ? img.naturalWidth : 0)))
    .toBeGreaterThan(100);
  const natural = await preview.evaluate((img: HTMLImageElement) => ({ width: img.naturalWidth, height: img.naturalHeight }));

  const saving = page.waitForEvent('download');
  await dialog.getByRole('button', { name: 'Download' }).click();
  const file = await saving;
  expect(file.suggestedFilename()).toMatch(/^topology-.*\.png$/);
  expect(pngSize(await readFile(await file.path()))).toEqual({ width: natural.width * 2, height: natural.height * 2 });

  // ---- keep as evidence: the engine draws it again and stores its own drawing
  await dialog.getByRole('button', { name: 'Keep as evidence…' }).click();
  const keep = page.getByRole('dialog', { name: 'Keep as evidence' });
  await expect(keep).toContainText('not a pixel copy of the screen');
  await expect(keep).toContainText('topology.export');
  await keep.getByRole('textbox', { name: 'Title' }).fill('The lab before the change');
  await keep.getByRole('button', { name: 'Keep', exact: true }).click();
  await expect(page.getByText('Kept as evidence', { exact: true })).toBeVisible({ timeout: 30_000 });
  await expect(keep).toHaveCount(0);

  const after = await rpc<EvidenceListResult>('evidence.list', { kind: 'topology', limit: 200 });
  expect(after.items).toHaveLength(before + 1);
  const kept = after.items[0] as Evidence;
  expect(kept.title).toBe('The lab before the change');
  expect(kept.source.from).toBe('topology.export');
  expect(kept.source.lab).toBe(true);
  // What the engine stored is the drawing it would export for the same graph.
  const stored = await rpc<EvidenceWithBody>('evidence.get', { id: kept.id });
  expect(stored.body.type === 'svg' && stored.body.svg).toContain('<svg');

  expect(console.messages).toEqual([]);
});

test('a build without evidence shows no Evidence tab and offers to keep nothing', async ({ page }) => {
  const console = watchConsole(page);

  // The same kind of injection as the other "build without" tests: only the
  // method list is trimmed, every method that remains answers for real.
  await withoutEvidence(page);

  await openApp(page, '/ops/incidents', { backend });
  const tabs = page.getByRole('tablist', { name: 'Operations' });
  await expect(tabs.getByRole('tab', { name: 'Incidents' })).toBeVisible();
  await expect(tabs.getByRole('tab', { name: 'Evidence' })).toHaveCount(0);
  await expect(tabs.getByRole('tab', { name: 'Audit' })).toBeVisible();

  // Reached by its address, it says what is missing.
  await goTo(page, '/ops/evidence');
  await expect(page.getByText('Evidence is not in this build')).toBeVisible();

  // The timeline offers neither to attach nor to detach; what is already on
  // it is still shown, because it is part of the incident's record.
  await goTo(page, incidentUrl(world.incidentId));
  await expect(page.getByTestId('timeline-row').first()).toBeVisible({ timeout: 30_000 });
  await expect(page.getByRole('button', { name: 'Attach evidence…' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Detach' })).toHaveCount(0);
  await expect(page.locator('[data-tombstone]')).toHaveCount(1);
  // The panel beside it has no line about evidence, and so no link to a tab
  // that is not there.
  await expect(page.getByText('What it touches')).toBeVisible();
  await expect(page.getByRole('link', { name: /items? attached/ })).toHaveCount(0);
  await expect(page.getByText('“Attach evidence…” on the timeline puts some here')).toHaveCount(0);

  // The palette does not offer the tab either, while it still offers the others.
  await page.keyboard.press('ControlOrMeta+k');
  const palette = page.getByRole('dialog', { name: 'Command palette' });
  const ask = palette.getByRole('combobox', { name: 'Search commands and screens' });
  await ask.fill('handover');
  await expect(palette.getByRole('option', { name: 'Write the shift handover' })).toBeVisible();
  await ask.fill('evidence');
  await expect(palette.getByRole('option', { name: 'Write the shift handover' })).toHaveCount(0);
  await expect(palette.getByRole('option', { name: 'Open the evidence' })).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(palette).toHaveCount(0);

  // The map's export still exports, and has no "Keep as evidence".
  await goTo(page, '/topology');
  await page.getByRole('button', { name: 'Export this drawing' }).click({ timeout: 30_000 });
  const dialog = page.getByRole('dialog', { name: 'Export this drawing' });
  await expect(dialog.getByRole('button', { name: 'Download' })).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Keep as evidence…' })).toHaveCount(0);
  await dialog.getByRole('combobox', { name: 'Format' }).selectOption('png');
  await expect(dialog.getByRole('img', { name: 'The drawing that will be saved' })).toBeVisible({ timeout: 30_000 });

  expect(console.messages).toEqual([]);
});

// ====================================================== evidence, under attack
//
// What is in an evidence item came from a device or from a stranger's
// drawing, and it is kept as it was received. These cases show the screens
// the worst of it and require that nothing happens: nothing runs, nothing is
// fetched, nothing freezes, and nothing is claimed that did not take place.
// They stand on an incident and a lab switch of their own, made through the
// RPC surface, so any one of them can be run alone.

const stage = { incidentId: '', incidentTitle: 'Evidence under review', switchId: '', sessionId: '' };

async function staged(rpc: Rpc): Promise<typeof stage> {
  if (stage.incidentId !== '') return stage;
  await rpc('labs.node.add', { lab_id: world.labId, model: 'switch', name: 'SW3' });
  const devices = await rpc<{ id: string; name: string }[]>('devices.list');
  stage.switchId = devices.find((device) => device.name === 'Ops/SW3')?.id ?? '';
  expect(stage.switchId, 'the third lab node is in the inventory').not.toBe('');
  const session = await rpc<{ id: string }>('terminal.open', { device_id: stage.switchId });
  stage.sessionId = session.id;
  await rpc('terminal.input', { session_id: session.id, data: 'show version\r' });
  await expect
    .poll(
      async () => (await rpc<{ text: string }>('terminal.snapshot', { session_id: session.id, lines: 60 })).text,
      { timeout: 30_000 },
    )
    .toContain('Processor board ID');
  const incident = await rpc<Incident>('incidents.create', {
    title: stage.incidentTitle,
    severity: 'sev3',
    device_ids: [stage.switchId],
  });
  stage.incidentId = incident.id;
  return stage;
}

/**
 * THE FAULT INJECTION of these cases: what is stored for one item is replaced
 * in the engine's own table (`ops_evidence_bodies`) while the engine runs.
 *
 * A stored drawing "is hostile input" by contract, and the engine's own
 * drawing never is, so no method stores a hostile one. Writing it where the
 * engine keeps it is the nearest thing to a `topology.export` that returned
 * it: from here on nothing is stood in for. `evidence.get` and
 * `incidents.export` of the running engine answer with what is stored, and
 * the browser is shown that.
 */
function storeBody(id: string, body: string): void {
  const database = new DatabaseSync(join(backend.dataDir, 'nocturne.db'));
  try {
    // The engine may be in the middle of a write of its own.
    database.exec('PRAGMA busy_timeout = 10000');
    const written = database.prepare('UPDATE ops_evidence_bodies SET body = ? WHERE id = ?').run(body, id);
    expect(Number(written.changes), 'the item has a stored body to replace').toBe(1);
  } finally {
    database.close();
  }
}

/** An address nothing listens on: a request for it is the proof that something was fetched. */
const BEACON = 'http://127.0.0.1:9/evidence-beacon';
const SAID = 'nocturne-test: evidence spoke:';

/**
 * Everything a drawing can carry that must never run or load: a script, an
 * `onload`, an external script, style and image, frames inside a
 * `foreignObject`, a `javascript:` link over the whole picture, and a `use`
 * of another document. Each tells the page if it lives.
 */
const HOSTILE_DRAWING = [
  '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="400" height="200" viewBox="0 0 400 200" ' +
    `onload="window.__evidence_pwned = 'onload'; top.postMessage('evidence-pwned onload', '*')">`,
  "<script>window.__evidence_pwned = 'script'; top.postMessage('evidence-pwned script', '*'); alert('evidence-pwned')</script>",
  `<script href="${BEACON}.js"/>`,
  `<style>@import url("${BEACON}.css");</style>`,
  `<image href="${BEACON}.png" width="10" height="10"/>`,
  '<foreignObject width="400" height="200"><body xmlns="http://www.w3.org/1999/xhtml">' +
    `<iframe src="javascript:top.postMessage('evidence-pwned frame', '*')"></iframe>` +
    `<iframe src="${BEACON}.html"></iframe>` +
    `<img src="${BEACON}-img.png" onerror="top.postMessage('evidence-pwned img', '*')"/>` +
    '</body></foreignObject>',
  `<a xlink:href="javascript:top.postMessage('evidence-pwned link', '*')"><rect width="400" height="200" fill="#0a7d2c"/></a>`,
  `<use href="${BEACON}-use.svg#x"/>`,
  '<text x="20" y="100" font-size="20" fill="#fff">HOSTILE-DRAWING-MARKER</text>',
  '</svg>',
].join('');

interface Life {
  /** Every request any page or frame of this browser context made. */
  requests: string[];
  dialogs: string[];
  /** What something that should be inert managed to tell the page. */
  said: string[];
}

/** Listens for every sign of life: a request, a dialog, a message to the page. */
async function watchForLife(page: Page): Promise<Life> {
  const life: Life = { requests: [], dialogs: [], said: [] };
  page.context().on('request', (sent) => life.requests.push(sent.url()));
  page.on('dialog', (dialog) => {
    life.dialogs.push(`${dialog.type()}: ${dialog.message()}`);
    void dialog.dismiss();
  });
  page.on('console', (message) => {
    if (message.text().startsWith(SAID)) life.said.push(message.text());
  });
  await page.addInitScript((said) => {
    if (window !== window.top) return;
    window.addEventListener('message', (event) => {
      if (typeof event.data === 'string' && event.data.startsWith('evidence-pwned')) {
        console.info(`${said} ${event.data}`);
      }
    });
  }, SAID);
  return life;
}

/** Nothing ran, in any frame; nothing was fetched from anywhere but the engine; no dialog; no new window. */
async function nothingLived(page: Page, life: Life): Promise<void> {
  for (const frame of page.frames()) {
    const mark = await frame
      .evaluate(() => (window as { __evidence_pwned?: string }).__evidence_pwned)
      .catch(() => undefined);
    expect(mark, `a script ran in ${frame.url()}`).toBeUndefined();
  }
  expect(life.said).toEqual([]);
  expect(life.dialogs).toEqual([]);
  expect(life.requests.filter((url) => url.includes('evidence-beacon'))).toEqual([]);
  expect(life.requests.filter((url) => !url.startsWith(backend.url) && !/^(data|blob|about):/.test(url))).toEqual([]);
  expect(page.context().pages()).toHaveLength(1);
}

function decoded(address: string): string {
  return Buffer.from(address.slice(address.indexOf(',') + 1), 'base64').toString('utf8');
}

test('a stored drawing that carries a script, an onload, frames and addresses to fetch is a picture and nothing else, wherever it is shown', async ({
  page,
  request,
}) => {
  test.setTimeout(150_000);
  const console = watchConsole(page);
  const life = await watchForLife(page);
  const rpc = rpcFor(request, backend);
  const on = await staged(rpc);
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write'], { origin: backend.url });
  // A headless browser has no print dialog: `print()` is a line in the
  // console instead, in every frame, so that the print can be waited for.
  const printed: string[] = [];
  page.on('console', (message) => {
    if (message.text() === 'nocturne-test: print() was asked for') printed.push(message.text());
  });
  await page.addInitScript(() => {
    // (`globalThis.console`: in this file `console` is the test's own watch.)
    window.print = () => globalThis.console.info('nocturne-test: print() was asked for');
  });

  // A real item, the engine's own drawing, attached to a real incident …
  const kept = await rpc<EvidenceWithBody>('evidence.capture_topology', { title: 'The lab, as stored' });
  await rpc('evidence.attach', { id: kept.id, incident_id: on.incidentId, author: 'mo' });
  // … and then what is stored for it is the hostile drawing. The engine
  // itself now answers with it.
  storeBody(kept.id, HOSTILE_DRAWING);
  const stored = await rpc<EvidenceWithBody>('evidence.get', { id: kept.id });
  expect(stored.body.type === 'svg' && stored.body.svg).toBe(HOSTILE_DRAWING);

  // ---- the viewer
  await openApp(page, evidenceUrl(kept.id), { backend });
  const shown = page.getByTestId('evidence-drawing');
  await expect(shown).toBeVisible({ timeout: 30_000 });
  // It is the hostile drawing that is on screen, decoded as a picture of its size …
  await expect
    .poll(async () => shown.evaluate((img: HTMLImageElement) => (img.complete ? img.naturalWidth : 0)))
    .toBe(400);
  expect(await shown.evaluate((img: HTMLImageElement) => img.tagName)).toBe('IMG');
  const address = (await shown.getAttribute('src')) ?? '';
  expect(address).toMatch(/^data:image\/svg\+xml;base64,/);
  expect(decoded(address)).toBe(HOSTILE_DRAWING);
  // … and clicking where its link covers it does nothing: an image has no links.
  const box = await shown.boundingBox();
  if (box) await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);

  // Nothing of it is in the document as markup: no element of the drawing,
  // no frame, and its text nowhere but inside the encoded address.
  const dom = await page.evaluate(() => ({
    markup: document.documentElement.innerHTML.includes('HOSTILE-DRAWING-MARKER'),
    pwned: document.documentElement.innerHTML.includes('evidence-pwned'),
    foreign: document.querySelectorAll('foreignObject, iframe, object, embed, image').length,
    inlineScripts: [...document.querySelectorAll('script')].filter((script) => !script.src).length,
    images: document.querySelectorAll('main img').length,
  }));
  expect(dom).toEqual({ markup: false, pwned: false, foreign: 0, inlineScripts: 0, images: 1 });

  // ---- a PNG of it: decoded as an image again, so it is a picture of a
  // green rectangle at twice the size, and still nothing ran
  const savingPng = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Save as PNG' }).click();
  const png = await savingPng;
  expect(pngSize(await readFile(await png.path()))).toEqual({ width: 800, height: 400 });

  // ---- "Copy image" puts that picture on the clipboard, as a PNG
  await page.getByRole('button', { name: 'Copy image' }).click();
  await expect(page.getByText('Image copied')).toBeVisible({ timeout: 30_000 });
  const copied = await page.evaluate(async () => {
    const found: { type: string; bytes: number[] }[] = [];
    for (const entry of await navigator.clipboard.read()) {
      for (const type of entry.types) {
        const blob = await entry.getType(type);
        found.push({ type, bytes: [...new Uint8Array(await blob.arrayBuffer()).subarray(0, 24)] });
      }
    }
    return found;
  });
  expect(copied.map((entry) => entry.type)).toEqual(['image/png']);
  expect(pngSize(Buffer.from(copied[0]?.bytes ?? []))).toEqual({ width: 800, height: 400 });

  // ---- "Download SVG" hands over the stored bytes, as a file and not as a page
  const savingSvg = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Download SVG' }).click();
  const svgFile = await savingSvg;
  expect(svgFile.suggestedFilename()).toBe(`the-lab-as-stored-${kept.id}.svg`);
  expect(await readFile(await svgFile.path(), 'utf8')).toBe(HOSTILE_DRAWING);
  await nothingLived(page, life);

  // ---- the incident's timeline: a reference by title, never the drawing
  await goTo(page, incidentUrl(on.incidentId));
  const row = page.locator(`[data-row="evidence:${kept.id}"]`);
  await expect(row).toBeVisible({ timeout: 30_000 });
  await expect(row.getByRole('link', { name: 'The lab, as stored' })).toBeVisible();
  await expect(row.locator('img, image, iframe, object, embed, foreignObject')).toHaveCount(0);
  await row.locator('details > summary').click();
  // Its provenance names what it is; the drawing itself is not there.
  await expect(row).toContainText(/the topology.s drawing/);

  // ---- the review of that incident, previewed as the HTML the engine wrote:
  // the drawing is in its appendix, as an image, inside a frame that is
  // allowed nothing
  await page.getByRole('button', { name: 'Export review' }).click();
  const review = page.getByRole('dialog', { name: 'Post-incident review' });
  await review.getByRole('combobox', { name: 'Format' }).selectOption('html');
  const frame = review.locator('iframe[title$="(preview)"]');
  await expect(frame).toHaveAttribute('sandbox', '');
  const picture = frame.contentFrame().locator('img.drawing');
  await expect(picture).toHaveCount(1, { timeout: 30_000 });
  await expect
    .poll(async () => picture.evaluate((img: HTMLImageElement) => (img.complete ? img.naturalWidth : 0)))
    .toBe(400);
  expect(decoded((await picture.getAttribute('src')) ?? '')).toBe(HOSTILE_DRAWING);
  expect(
    await frame.contentFrame().locator('script, iframe, object, embed, foreignObject, image').count(),
    'the review holds the drawing as an image and as nothing else',
  ).toBe(0);
  // Printing it hands the same document to a second frame, the one place a
  // script of ours runs beside it. The drawing is an image there too.
  await review.getByRole('button', { name: 'Print' }).click();
  await expect.poll(() => printed.length, { timeout: 30_000 }).toBe(1);
  const printing = page.frames().find((candidate) => candidate.url() === 'about:srcdoc' && candidate.name() === '');
  expect(page.frames().length, 'the preview and the frame that prints').toBe(3);
  expect(printing).toBeDefined();
  await nothingLived(page, life);
  // The other formats are previewed as text, whatever they hold: Markdown
  // and CSV describe the drawing, and JSON carries its source as a string,
  // which on this screen is characters in a <pre>.
  for (const format of ['markdown', 'csv', 'json']) {
    await review.getByRole('combobox', { name: 'Format' }).selectOption(format);
    const source = review.locator('pre');
    await expect(source).toBeVisible({ timeout: 30_000 });
    // (The frame that printed is still there, out of sight; the preview is not a frame.)
    await expect(review.locator('iframe[title$="(preview)"]')).toHaveCount(0);
    await expect(source.locator('img, image, script, iframe, object, embed, foreignObject')).toHaveCount(0);
    if (format === 'json') await expect(source).toContainText('HOSTILE-DRAWING-MARKER');
    else await expect(source).not.toContainText('HOSTILE-DRAWING-MARKER');
  }
  await page.keyboard.press('Escape');
  await nothingLived(page, life);

  // ---- and under Evidence, in the list and in the picker of the attach
  // dialog, it is a title
  await goTo(page, '/ops/evidence?kind=topology');
  await expect(page.locator(`[data-evidence="${kept.id}"]`)).toHaveText('The lab, as stored', { timeout: 30_000 });
  await expect(page.locator('main img')).toHaveCount(0);
  await nothingLived(page, life);

  // ---- what is stored need not be a drawing at all. Then there is no
  // picture, and the screen says so instead of showing a broken one.
  storeBody(kept.id, 'not a drawing at all <script>top.postMessage("evidence-pwned text", "*")</script>');
  await openApp(page, evidenceUrl(kept.id), { backend });
  await expect(page.getByRole('alert').filter({ hasText: 'cannot be shown as a picture' })).toBeVisible({
    timeout: 30_000,
  });
  await expect(page.getByTestId('evidence-drawing')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Save as PNG' })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Copy image' })).toBeDisabled();
  // It can still be taken out, exactly as stored.
  const savingRaw = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Download SVG' }).click();
  expect(await readFile(await (await savingRaw).path(), 'utf8')).toContain('not a drawing at all');
  await nothingLived(page, life);

  // The console is clean as well: a blocked script would have been reported.
  expect(console.messages).toEqual([]);
});

test('text of any size and shape stays text, in every place it is shown, and the window stays usable', async ({
  page,
  request,
}) => {
  test.setTimeout(150_000);
  const console = watchConsole(page);
  const life = await watchForLife(page);
  const rpc = rpcFor(request, backend);
  const on = await staged(rpc);

  // A title is text a person or a device supplied, and it travels: the list,
  // the page's heading, a timeline, a dialog, a toast.
  const title = `<img src="${BEACON}-title.png" onerror="top.postMessage('evidence-pwned title','*')"> ‮eltit`;
  // One line of a hundred thousand characters: escape sequences, a direction
  // override, markup that would run if it were markup, and an entity.
  const line = `\u001b[31mA‮B<img src="${BEACON}-text.png" onerror="top.postMessage('evidence-pwned text','*')"><script>top.postMessage('evidence-pwned text script','*')</script>\u0007&amp;`
    .repeat(1_000)
    .slice(0, 100_000);
  const wide = await rpc<EvidenceWithBody>('evidence.add_text', { text: `${line}\nend of it`, title });
  await rpc('evidence.attach', { id: wide.id, incident_id: on.incidentId, author: 'mo' });
  // And the most lines an item can hold: the 256 KiB limit in lines of one character.
  const tall = await rpc<EvidenceWithBody>('evidence.add_text', {
    text: 'a\n'.repeat(131_000),
    title: 'a hundred and thirty-one thousand lines',
  });
  expect(tall.lines).toBe(131_000);

  // ---- the long line
  await openApp(page, evidenceUrl(wide.id), { backend });
  const text = page.getByTestId('evidence-text');
  await expect(text).toBeVisible({ timeout: 30_000 });
  const pre = text.locator('pre');
  // The first numbered line: its number, then its text.
  const first = await pre.locator('code > span').first().locator('span').nth(1).innerText();
  const storedLine = (wide.body.type === 'text' ? wide.body.text : '').split('\n')[0] ?? '';
  expect(storedLine).toHaveLength(100_000);
  expect(first).toHaveLength(100_000);
  // Every control character is a picture of itself, every override a mark,
  // and the markup is the characters it is made of.
  expect(first.startsWith('␛[31mA�B<img src="')).toBe(true);
  expect(first).toContain('<script>top.postMessage(');
  expect(first).toContain('␇&amp;');
  expect(first).not.toMatch(/[\u0000-\u0008\u000b-\u001f\u007f‪-‮⁦-⁩]/);
  await expect(text.locator('img, script, iframe')).toHaveCount(0);
  await expect(page.locator('.xterm')).toHaveCount(0);
  // The line scrolls inside its own box; the page is no wider than the window.
  expect(await pre.evaluate((box) => box.scrollWidth)).toBeGreaterThan(100_000);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  // The title is text in the heading too, with its override shown as a mark.
  await expect(page.getByTestId('evidence-title')).toContainText('<img src="');
  await expect(page.getByTestId('evidence-title').locator('img')).toHaveCount(0);

  // ---- the same title on the timeline it is attached to, in the delete
  // confirmation and in the picker
  await goTo(page, incidentUrl(on.incidentId));
  const row = page.locator(`[data-row="evidence:${wide.id}"]`);
  await expect(row).toContainText('<img src="', { timeout: 30_000 });
  await expect(row.locator('img')).toHaveCount(0);
  await page.getByRole('button', { name: 'Attach evidence…' }).click();
  const picker = page.getByRole('dialog', { name: 'Attach evidence' });
  await expect(picker.getByRole('radio').first()).toBeVisible({ timeout: 30_000 });
  await expect(picker).toContainText('<img src="');
  await expect(picker.locator('img')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await nothingLived(page, life);

  // ---- the many lines. MEASURED: put on screen whole, this item took the
  // window seven seconds to show and five to answer each click after it
  // (393,000 elements). It is shown a page of lines at a time instead.
  await page.evaluate(() => {
    const frames = { worst: 0, last: performance.now() };
    (window as { __frames?: typeof frames }).__frames = frames;
    const tick = () => {
      const now = performance.now();
      frames.worst = Math.max(frames.worst, now - frames.last);
      frames.last = now;
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
  await goTo(page, evidenceUrl(tall.id));
  // (Counts are grouped for the reader's locale; the digits are what is held to.)
  const linesShown = page.getByTestId('evidence-lines-shown');
  await expect(linesShown).toContainText(/Lines 1 to 5.000 of 131.000 are on screen/, { timeout: 30_000 });
  // It answers: a dialog opens, a page turns, and the numbering goes on where it stood.
  await page.getByRole('button', { name: 'Rename…' }).click();
  await expect(page.getByRole('dialog', { name: 'Rename this evidence' })).toBeVisible();
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Later lines' }).click();
  await expect(linesShown).toContainText(/Lines 5.001 to 10.000 of 131.000/);
  await expect(text.locator('pre [aria-hidden="true"]').first()).toHaveText('5001');
  await page.getByRole('button', { name: 'The last lines' }).click();
  await expect(linesShown).toContainText(/Lines 130.001 to 131.000 of 131.000/);
  await expect(text.locator('pre [aria-hidden="true"]').last()).toHaveText('131000');
  const cost = await page.evaluate(() => ({
    elements: document.querySelectorAll('*').length,
    worstFrameMs: Math.round((window as { __frames?: { worst: number } }).__frames?.worst ?? 0),
  }));
  // The whole item as numbered lines is 393,000 elements; a page of it is 15,000.
  expect(cost.elements).toBeLessThan(40_000);
  // And no frame took anything like the seconds it used to. Generous, for a
  // machine that is doing other things: the freeze this guards against was 6 s.
  expect(cost.worstFrameMs).toBeLessThan(2_500);
  // What is downloaded is all of it, not the page on screen.
  const saving = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Download as stored' }).click();
  const file = await readFile(await (await saving).path());
  expect(file.length).toBe(262_000);
  expect(createHash('sha256').update(file).digest('hex')).toBe(tall.sha256);

  await nothingLived(page, life);
  expect(console.messages).toEqual([]);
});

test('a file the window cannot hand to the browser is reported as not saved, and nothing says it was', async ({
  page,
  request,
}) => {
  test.setTimeout(150_000);
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const item = await rpc<EvidenceWithBody>('evidence.add_text', {
    text: 'interface Fa0/1 is down',
    title: 'a line to save',
  });
  const drawing = await rpc<EvidenceWithBody>('evidence.capture_topology', { title: 'a drawing to save' });

  // THE FAULT INJECTION: a window that cannot make the address a download is
  // handed over on. Nothing else about the page is touched.
  await page.addInitScript(() => {
    URL.createObjectURL = () => {
      throw new Error('this window makes no addresses for files');
    };
  });
  let downloads = 0;
  page.on('download', () => {
    downloads += 1;
  });
  const notices = page.getByRole('region', { name: 'Notifications' });
  /** One failure: said as a failure, with its reason, and with nothing beside it that says otherwise. */
  const saidNotSaved = async (file: string | RegExp) => {
    await expect(notices.getByText(file)).toBeVisible({ timeout: 30_000 });
    await expect(notices.getByText('this window makes no addresses for files')).toBeVisible();
    await expect(notices.getByText(/Download started|Saved|Asked the system to save/)).toHaveCount(0);
    // An error stays until it is dismissed, as a person would before going on.
    await notices.getByRole('button', { name: 'Dismiss' }).click({ timeout: 10_000 });
    await expect(notices.getByRole('button', { name: 'Dismiss' })).toHaveCount(0);
  };

  // ---- text
  await openApp(page, evidenceUrl(item.id), { backend });
  await page.getByRole('button', { name: 'Download as stored' }).click({ timeout: 30_000 });
  await saidNotSaved(`a-line-to-save-${item.id}.txt was not saved`);

  // ---- a picture made here: the PNG exists, and still could not be handed over
  await goTo(page, evidenceUrl(drawing.id));
  await expect(page.getByTestId('evidence-drawing')).toBeVisible({ timeout: 30_000 });
  await page.getByRole('button', { name: 'Save as PNG' }).click({ timeout: 10_000 });
  await saidNotSaved(`a-drawing-to-save-${drawing.id}.png was not saved`);

  // ---- a document the engine wrote
  await goTo(page, '/topology');
  await page.getByRole('button', { name: 'Export this drawing' }).click({ timeout: 30_000 });
  const dialog = page.getByRole('dialog', { name: 'Export this drawing' });
  await expect(dialog.locator('pre')).toBeVisible({ timeout: 30_000 });
  await dialog.getByRole('button', { name: 'Download' }).click({ timeout: 10_000 });
  await saidNotSaved(/^topology-.*\.svg was not saved$/);

  // Three failures said, and not one file.
  expect(downloads).toBe(0);
  expect(console.messages).toEqual([]);
});

/**
 * What the engine answers when the feature a capture reads from is not in the
 * build: its own sentences, as its own test run records them
 * (`nocturne-ops`, `tests/evidence.rs`,
 * `without_the_feature_it_reads_from_a_capture_is_unavailable_and_says_why`).
 */
const UNAVAILABLE: Record<string, string> = {
  'evidence.capture_terminal':
    'evidence cannot be captured from a terminal session: `terminal.list` is not part of this build',
  'evidence.capture_topology':
    'evidence cannot be captured from the topology: `topology.export` is not part of this build',
};

test('a capture answered “unavailable” shows the engine’s reason where it was asked for, and keeps nothing', async ({
  page,
  request,
}) => {
  test.setTimeout(150_000);
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const on = await staged(rpc);
  const count = async () => (await rpc<EvidenceListResult>('evidence.list', { limit: 200 })).items.length;
  const before = await count();

  // THE FAULT INJECTION: a build with every feature never answers a capture
  // with `unavailable`, so these two calls are answered here, with the
  // engine's recorded sentences, the way it answers a refusal (200, ok: false).
  // Every other call reaches the engine.
  await page.route('**/api/rpc', async (route) => {
    const body = route.request().postDataJSON() as { method?: string };
    const message = UNAVAILABLE[body.method ?? ''];
    if (message === undefined) return route.continue();
    await route.fulfill({ json: { ok: false, error: { code: 'unavailable', message } } });
  });

  // ---- from the timeline
  await openApp(page, incidentUrl(on.incidentId), { backend });
  await page.getByRole('button', { name: 'Attach evidence…' }).click({ timeout: 30_000 });
  const attach = page.getByRole('dialog', { name: 'Attach evidence' });
  await attach.getByRole('tab', { name: 'From a terminal session' }).click();
  await attach.getByRole('combobox', { name: 'Session' }).selectOption(on.sessionId);
  await attach.getByRole('button', { name: 'Capture and attach' }).click();
  const refused = attach.getByTestId('attach-refused');
  await expect(refused).toContainText('The engine could not read it.');
  await expect(refused).toContainText(UNAVAILABLE['evidence.capture_terminal'] ?? '');
  await expect(refused).toContainText('Nothing was stored.');
  // The dialog is still there with what was chosen, and says nothing of success.
  await expect(attach.getByRole('combobox', { name: 'Session' })).toHaveValue(on.sessionId);
  await expect(page.getByText('Attached to the timeline')).toHaveCount(0);
  await page.keyboard.press('Escape');

  // ---- from the map
  await goTo(page, '/topology');
  await page.getByRole('button', { name: 'Export this drawing' }).click({ timeout: 30_000 });
  await page
    .getByRole('dialog', { name: 'Export this drawing' })
    .getByRole('button', { name: 'Keep as evidence…' })
    .click();
  const keep = page.getByRole('dialog', { name: 'Keep as evidence' });
  await keep.getByRole('button', { name: 'Keep', exact: true }).click();
  const said = keep.getByTestId('keep-refused');
  await expect(said).toContainText('The engine could not read it.');
  await expect(said).toContainText(UNAVAILABLE['evidence.capture_topology'] ?? '');
  await expect(page.getByText('Kept as evidence')).toHaveCount(0);
  await expect(keep).toBeVisible();

  expect(await count()).toBe(before);
  expect(console.messages).toEqual([]);
});

test('a build without the terminal feature offers no capture from a session, and the rest still works', async ({
  page,
  request,
}) => {
  test.setTimeout(150_000);
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const on = await staged(rpc);

  // The same kind of injection as the other "build without" cases: only the
  // method list is trimmed, everything that remains answers for real.
  await page.route('**/api/rpc', async (route) => {
    const body = route.request().postDataJSON() as { method?: string };
    if (body.method !== 'app.methods') return route.continue();
    const response = await route.fetch();
    const payload = (await response.json()) as { ok: boolean; result?: string[] };
    if (payload.ok && payload.result) {
      payload.result = payload.result.filter((method) => !method.startsWith('terminal.'));
    }
    await route.fulfill({ response, json: payload });
  });

  await openApp(page, incidentUrl(on.incidentId), { backend });
  await page.getByRole('button', { name: 'Attach evidence…' }).click({ timeout: 30_000 });
  const dialog = page.getByRole('dialog', { name: 'Attach evidence' });
  // Two ways in, not three: there is no session list to choose from.
  await expect(dialog.getByRole('tab')).toHaveText(['Already kept', 'Text I hold']);
  await expect(dialog.getByRole('tab', { name: 'From a terminal session' })).toHaveCount(0);

  // Text a person holds is still kept and attached, by the engine.
  await dialog.getByRole('tab', { name: 'Text I hold' }).click();
  await dialog.getByRole('textbox', { name: 'Title' }).fill('kept without a terminal');
  await dialog.getByRole('textbox', { name: 'Text' }).fill('Gi0/2 is down');
  await dialog.getByRole('button', { name: 'Keep and attach' }).click();
  await expect(dialog).toHaveCount(0, { timeout: 30_000 });
  await expect(timelineRows(page).filter({ hasText: 'kept without a terminal' })).toBeVisible({ timeout: 30_000 });

  expect(console.messages).toEqual([]);
});

test('the numbers behind a stored chart leave a missing sample empty and cannot be run by a spreadsheet', async ({
  page,
  request,
}) => {
  test.setTimeout(150_000);
  const console = watchConsole(page);
  const life = await watchForLife(page);
  const rpc = rpcFor(request, backend);
  const on = await staged(rpc);

  // Real samples of the stage's switch, kept by the engine.
  const since = Date.now() - 60_000;
  for (let index = 0; index < 3; index += 1) {
    const poll = await rpc<{ job_id: string }>('monitor.poll_now', { device_ids: [on.switchId] });
    await finished(rpc, poll.job_id);
    await page.waitForTimeout(2_100);
  }
  const kept = await rpc<EvidenceWithBody>('evidence.capture_chart', {
    series: [{ device_id: on.switchId, metric: 'up' }],
    since,
    until: Date.now() + 1_000,
    step_s: 1,
    title: 'SW3 reachability',
  });
  const body = kept.body.type === 'chart' ? kept.body : null;
  const points = (body?.series[0]?.points ?? []) as [number, number][];
  expect(points.length).toBeGreaterThanOrEqual(3);
  const covered = body?.coverage.series[0];
  expect(covered).toBeDefined();

  // THE FAULT INJECTION (see `storeBody`): what is stored gains a second
  // series the monitor could have answered with — one whose name a
  // spreadsheet would run as a formula, and which has no sample at one of the
  // times the first has.
  const formula = `=HYPERLINK("${BEACON}","up")`;
  const missing = points[1]?.[0] ?? 0;
  const fewer = points.filter(([at]) => at !== missing);
  storeBody(
    kept.id,
    JSON.stringify({
      ...body,
      series: [...(body?.series ?? []), { ...body?.series[0], metric: formula, points: fewer }],
      coverage: {
        ...body?.coverage,
        points: points.length + fewer.length,
        series: [...(body?.coverage.series ?? []), { ...covered, metric: formula, points: fewer.length }],
      },
    }),
  );

  await openApp(page, evidenceUrl(kept.id), { backend });
  const figure = page.locator('figure').filter({ hasText: 'SW3 reachability' });
  await expect(figure.locator('canvas')).toBeVisible({ timeout: 30_000 });
  // The name is on screen as the text it is.
  await expect(page.getByTestId('evidence-coverage')).toContainText(formula);

  await figure.getByRole('button', { name: 'Export this chart' }).click();
  const saving = page.waitForEvent('download');
  await page.getByRole('menuitem', { name: 'Save numbers as CSV' }).click();
  const csv = (await readFile(await (await saving).path(), 'utf8')).trimEnd().split('\r\n');
  // A cell that starts with `=` is quoted out with an apostrophe, and quoted
  // because it holds a comma and quotes of its own.
  expect(csv[0]).toBe(`time_utc,up,"'=HYPERLINK(""${BEACON}"",""up"")"`);
  // One row per stored time. Where the second series has no sample the cell
  // is empty: not a zero, and not the value before it.
  expect(csv.slice(1)).toEqual(
    points.map(([at, value]) =>
      [new Date(at).toISOString(), String(value), at === missing ? '' : String(value)].join(','),
    ),
  );
  expect(csv.find((row) => row.startsWith(new Date(missing).toISOString()))?.endsWith(',')).toBe(true);

  await nothingLived(page, life);
  expect(console.messages).toEqual([]);
});

// ------------------------------------------------------------ keyboard only

/**
 * Moves the focus to `target` with the Tab key alone (or Shift+Tab), and
 * requires what a sighted keyboard user needs once it is there: the element
 * is the one that has the focus, and a focus ring is drawn on it or on the
 * field it is the inside of.
 */
async function tabTo(page: Page, target: Locator, { back = false, limit = 60 } = {}): Promise<void> {
  const has = async () => target.evaluate((element) => element === document.activeElement).catch(() => false);
  for (let presses = 0; presses < limit && !(await has()); presses += 1) {
    await page.keyboard.press(back ? 'Shift+Tab' : 'Tab');
  }
  expect(await has(), `the Tab key reaches ${String(target)}`).toBe(true);
  const ring = await target.evaluate((element) => {
    const drawn = (on: Element | null) => {
      if (!on) return false;
      const style = getComputedStyle(on);
      return style.outlineStyle !== 'none' && Number.parseFloat(style.outlineWidth) > 0;
    };
    return drawn(element) || drawn(element.closest('[data-control]'));
  });
  expect(ring, `a focus ring is drawn on ${String(target)}`).toBe(true);
}

/** Chooses an option of a native select with the arrow keys. */
async function chooseWithArrows(page: Page, select: Locator, value: string): Promise<void> {
  await select.evaluate((element: HTMLSelectElement) => {
    if (document.activeElement !== element) throw new Error('the select does not have the focus');
  });
  const count = await select.locator('option').count();
  for (let presses = 0; presses < count && (await select.inputValue()) !== value; presses += 1) {
    await page.keyboard.press('ArrowDown');
  }
  await expect(select).toHaveValue(value);
}

test('every evidence control can be reached and worked with the keyboard alone', async ({ page, request }) => {
  test.setTimeout(240_000);
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const on = await staged(rpc);
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write'], { origin: backend.url });
  const main = page.getByRole('main');

  // Real items to work on: text long enough to page through, a drawing, a chart.
  const text = await rpc<EvidenceWithBody>('evidence.add_text', {
    text: Array.from({ length: 10_001 }, (_value, index) => `keyboard line ${index + 1}`).join('\n'),
    title: 'keyboard: a long capture',
  });
  const drawing = await rpc<EvidenceWithBody>('evidence.capture_topology', { title: 'keyboard: the map' });
  const poll = await rpc<{ job_id: string }>('monitor.poll_now', { device_ids: [on.switchId] });
  await finished(rpc, poll.job_id);
  const chart = await rpc<EvidenceWithBody>('evidence.capture_chart', {
    series: [{ device_id: on.switchId, metric: 'up' }],
    since: Date.now() - 600_000,
    until: Date.now() + 1_000,
    step_s: 1,
    title: 'keyboard: reachability',
  });

  // ---- the list: search, the three filters, a row, and Enter to open it
  await openApp(page, '/ops/evidence', { backend });
  await expect(page.getByTestId('evidence-row').first()).toBeVisible({ timeout: 30_000 });
  await tabTo(page, page.getByRole('searchbox', { name: 'Search evidence' }));
  await page.keyboard.type('keyboard: a long');
  await expect(page.getByTestId('evidence-row')).toHaveCount(1, { timeout: 30_000 });
  await tabTo(page, page.getByRole('combobox', { name: 'Kind' }));
  await chooseWithArrows(page, page.getByRole('combobox', { name: 'Kind' }), 'text');
  await tabTo(page, page.getByRole('combobox', { name: 'Device' }));
  await tabTo(page, page.getByRole('combobox', { name: 'Incident' }));
  const row = page.getByRole('row').filter({ has: page.locator(`[data-evidence="${text.id}"]`) });
  await tabTo(page, row);
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('evidence-title')).toHaveText('keyboard: a long capture', { timeout: 30_000 });

  // ---- the item: rename, and the focus comes back to where it was
  const rename = main.getByRole('button', { name: 'Rename…' });
  await tabTo(page, rename);
  await page.keyboard.press('Enter');
  const renaming = page.getByRole('dialog', { name: 'Rename this evidence' });
  await expect(renaming.getByRole('textbox', { name: 'Title' })).toBeFocused();
  await page.keyboard.press('ControlOrMeta+A');
  await page.keyboard.type('keyboard: renamed');
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('evidence-title')).toHaveText('keyboard: renamed', { timeout: 30_000 });
  await expect(renaming).toHaveCount(0);
  await expect(rename).toBeFocused();

  // ---- attach it to an incident, from its page
  await tabTo(page, main.getByRole('button', { name: 'Attach to an incident…' }));
  await page.keyboard.press('Enter');
  const attaching = page.getByRole('dialog', { name: 'Attach to an incident' });
  const incident = attaching.getByRole('combobox', { name: 'Incident' });
  await expect(incident).toBeFocused();
  await chooseWithArrows(page, incident, on.incidentId);
  await tabTo(page, attaching.getByRole('textbox', { name: 'Your name' }));
  await page.keyboard.press('ControlOrMeta+A');
  await page.keyboard.type('kai');
  await tabTo(page, attaching.getByRole('button', { name: 'Attach', exact: true }));
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('evidence-incident')).toContainText(on.incidentTitle, { timeout: 30_000 });

  // ---- the long text: a page on, then to the end, where the focus goes to the text
  await tabTo(page, main.getByRole('button', { name: 'Later lines' }));
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('evidence-lines-shown')).toContainText(/Lines 5.001 to 10.000 of 10.001/);
  await tabTo(page, main.getByRole('button', { name: 'The last lines' }));
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('evidence-lines-shown')).toContainText(/Lines 10.001 to 10.001 of 10.001/);
  await expect(page.getByTestId('evidence-text').locator('pre')).toBeFocused();
  await expect(page.getByTestId('evidence-text').locator('pre')).toContainText('keyboard line 10001');
  // … and back, the other way round.
  await tabTo(page, main.getByRole('button', { name: 'The first lines' }), { back: true });
  await page.keyboard.press('Space');
  await expect(page.getByTestId('evidence-lines-shown')).toContainText(/Lines 1 to 5.000 of 10.001/);

  // ---- the stored text as a file, and the hash on the clipboard
  await tabTo(page, main.getByRole('button', { name: 'Download as stored' }), { back: true });
  const savingText = page.waitForEvent('download');
  await page.keyboard.press('Enter');
  expect((await savingText).suggestedFilename()).toBe(`keyboard-renamed-${text.id}.txt`);
  await tabTo(page, main.getByRole('button', { name: 'Copy the SHA-256' }));
  await page.keyboard.press('Enter');
  await expect(page.getByText('Hash copied')).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(text.sha256);

  // ---- detach it again, from its page
  await tabTo(page, main.getByRole('button', { name: 'Detach' }));
  await page.keyboard.press('Enter');
  await expect(page.getByText('No incident. It is kept on its own')).toBeVisible({ timeout: 30_000 });

  // ---- a chart: its export menu, by arrow keys
  await goTo(page, evidenceUrl(chart.id));
  const figure = page.locator('figure').filter({ hasText: 'keyboard: reachability' });
  await expect(figure.locator('canvas')).toBeVisible({ timeout: 30_000 });
  await tabTo(page, figure.getByRole('button', { name: 'Export this chart' }));
  await page.keyboard.press('Enter');
  const menu = page.getByRole('menu');
  await expect(menu).toBeVisible();
  const numbers = menu.getByRole('menuitem', { name: 'Save numbers as CSV' });
  for (let presses = 0; presses < 5 && !(await numbers.evaluate((item) => item === document.activeElement)); presses += 1) {
    await page.keyboard.press('ArrowDown');
  }
  await expect(numbers).toBeFocused();
  const savingCsv = page.waitForEvent('download');
  await page.keyboard.press('Enter');
  expect((await savingCsv).suggestedFilename()).toMatch(/^keyboard-reachability-.*\.csv$/);
  await expect(menu).toHaveCount(0);
  // "What was asked for" opens with the keyboard too.
  await tabTo(page, main.locator('summary', { hasText: 'What was asked for' }));
  await page.keyboard.press('Enter');
  await expect(main.locator('details[open] pre')).toContainText('"metric": "up"');

  // ---- a drawing: a PNG of it, then delete it through the confirmation
  await goTo(page, evidenceUrl(drawing.id));
  await expect(page.getByTestId('evidence-drawing')).toBeVisible({ timeout: 30_000 });
  await tabTo(page, main.getByRole('button', { name: 'Save as PNG' }));
  const savingPng = page.waitForEvent('download');
  await page.keyboard.press('Enter');
  expect((await savingPng).suggestedFilename()).toBe(`keyboard-the-map-${drawing.id}.png`);
  await tabTo(page, main.getByRole('button', { name: 'Copy image' }), { back: true });
  await tabTo(page, main.getByRole('button', { name: 'Download SVG' }));
  await tabTo(page, main.getByRole('button', { name: 'Delete…' }), { back: true });
  await page.keyboard.press('Enter');
  const confirm = page.getByRole('dialog', { name: 'Delete this evidence?' });
  await tabTo(page, confirm.getByRole('button', { name: 'Delete it' }));
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('evidence-row').first()).toBeVisible({ timeout: 30_000 });
  await expect(rpc('evidence.get', { id: drawing.id })).rejects.toThrow(/not_found/);

  // ---- the incident's timeline: "Attach evidence…", its three tabs by arrow key
  await goTo(page, incidentUrl(on.incidentId));
  await expect(page.getByTestId('timeline-row').first()).toBeVisible({ timeout: 30_000 });
  await tabTo(page, main.getByRole('button', { name: 'Attach evidence…' }));
  await page.keyboard.press('Enter');
  const dialog = page.getByRole('dialog', { name: 'Attach evidence' });
  // Already kept: search, then the list is one radio group worked by arrows.
  await tabTo(page, dialog.getByRole('searchbox', { name: 'Search evidence' }));
  await page.keyboard.type('keyboard: renamed');
  const choice = dialog.getByRole('radio', { name: /keyboard: renamed/ });
  await expect(dialog.getByRole('radio')).toHaveCount(1, { timeout: 30_000 });
  await tabTo(page, choice);
  await page.keyboard.press('Space');
  await expect(choice).toBeChecked();
  await tabTo(page, dialog.getByRole('button', { name: 'Attach', exact: true }));
  await page.keyboard.press('Enter');
  await expect(dialog).toHaveCount(0, { timeout: 30_000 });
  const attached = page.locator(`[data-row="evidence:${text.id}"]`);
  await expect(attached).toBeVisible({ timeout: 30_000 });

  // From a terminal session: the session, the lines, a title.
  await tabTo(page, main.getByRole('button', { name: 'Attach evidence…' }), { back: true });
  await page.keyboard.press('Enter');
  await tabTo(page, dialog.getByRole('tab', { name: 'Already kept' }));
  await page.keyboard.press('ArrowRight');
  await expect(dialog.getByRole('tab', { name: 'From a terminal session' })).toBeFocused();
  const session = dialog.getByRole('combobox', { name: 'Session' });
  await tabTo(page, session);
  await chooseWithArrows(page, session, on.sessionId);
  await tabTo(page, dialog.getByRole('spinbutton', { name: 'Last lines' }));
  await page.keyboard.press('ControlOrMeta+A');
  await page.keyboard.type('40');
  await tabTo(page, dialog.getByRole('textbox', { name: 'Title' }));
  await page.keyboard.type('keyboard: what SW3 printed');
  await tabTo(page, dialog.getByRole('button', { name: 'Capture and attach' }));
  await page.keyboard.press('Enter');
  await expect(dialog).toHaveCount(0, { timeout: 30_000 });
  await expect(timelineRows(page).filter({ hasText: 'keyboard: what SW3 printed' })).toBeVisible({ timeout: 30_000 });

  // Text I hold: the last tab, reached with End.
  await tabTo(page, main.getByRole('button', { name: 'Attach evidence…' }), { back: true });
  await page.keyboard.press('Enter');
  await tabTo(page, dialog.getByRole('tab', { name: 'Already kept' }));
  await page.keyboard.press('End');
  await expect(dialog.getByRole('tab', { name: 'Text I hold' })).toBeFocused();
  await tabTo(page, dialog.getByRole('textbox', { name: 'Title' }));
  await page.keyboard.type('keyboard: read out');
  await tabTo(page, dialog.getByRole('textbox', { name: 'Text' }));
  await page.keyboard.type('Gi0/1 is err-disabled');
  await tabTo(page, dialog.getByRole('combobox', { name: 'About device' }));
  await tabTo(page, dialog.getByRole('textbox', { name: 'Where it is from' }));
  await tabTo(page, dialog.getByRole('button', { name: 'Keep and attach' }));
  await page.keyboard.press('Enter');
  await expect(dialog).toHaveCount(0, { timeout: 30_000 });
  await expect(timelineRows(page).filter({ hasText: 'keyboard: read out' })).toBeVisible({ timeout: 30_000 });

  // ---- an entry on the timeline: its link, its provenance, and Detach
  await tabTo(page, attached.getByRole('link', { name: 'keyboard: renamed' }), { limit: 120 });
  await tabTo(page, attached.locator('summary'));
  await page.keyboard.press('Enter');
  await expect(attached).toContainText(text.sha256);
  await tabTo(page, attached.getByRole('button', { name: 'Detach' }));
  await page.keyboard.press('Enter');
  await expect(attached).toHaveCount(0, { timeout: 30_000 });

  // ---- the map: PNG, and "Keep as evidence…" with its own dialog
  await goTo(page, '/topology');
  await tabTo(page, page.getByRole('button', { name: 'Export this drawing' }), { limit: 120 });
  await page.keyboard.press('Enter');
  const exporting = page.getByRole('dialog', { name: 'Export this drawing' });
  const format = exporting.getByRole('combobox', { name: 'Format' });
  await tabTo(page, format);
  await chooseWithArrows(page, format, 'png');
  await expect(exporting.getByRole('img', { name: 'The drawing that will be saved' })).toBeVisible({ timeout: 30_000 });
  await tabTo(page, exporting.getByRole('button', { name: 'Copy image' }));
  await tabTo(page, exporting.getByRole('button', { name: 'Download' }));
  const savingMap = page.waitForEvent('download');
  await page.keyboard.press('Enter');
  expect((await savingMap).suggestedFilename()).toMatch(/^topology-.*\.png$/);
  await tabTo(page, exporting.getByRole('button', { name: 'Keep as evidence…' }), { back: true });
  await page.keyboard.press('Enter');
  const keep = page.getByRole('dialog', { name: 'Keep as evidence' });
  await expect(keep.getByRole('textbox', { name: 'Title' })).toBeFocused();
  await page.keyboard.type('keyboard: the map, kept');
  const attachTo = keep.getByRole('combobox', { name: 'Attach to an incident' });
  await tabTo(page, attachTo);
  await chooseWithArrows(page, attachTo, on.incidentId);
  await tabTo(page, keep.getByRole('textbox', { name: 'Your name' }));
  await tabTo(page, keep.getByRole('button', { name: 'Keep and attach' }));
  await page.keyboard.press('Enter');
  await expect(page.getByText('Kept as evidence and attached')).toBeVisible({ timeout: 30_000 });
  const keptMap = (await rpc<EvidenceListResult>('evidence.list', { incident_id: on.incidentId, kind: 'topology' }))
    .items;
  expect(keptMap.map((item) => item.title)).toContain('keyboard: the map, kept');

  expect(console.messages).toEqual([]);
});
