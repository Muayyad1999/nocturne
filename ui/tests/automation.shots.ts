/**
 * `pnpm shots` — PNGs of the Automation screen, in both themes, at 1440×900
 * and 1280×720, with the assistant dock open and closed.
 *
 * Driven by a real engine: a lab of two switches and a router is started and
 * really run against, one device really cannot be reached, a playbook really
 * holds a device so another run really is skipped, and the comparison really
 * is of what those devices really printed. Nothing in these pictures is made
 * up. They are written to `tests/shots/<night|day>/` and compared against
 * nothing: they exist to be looked at before a change ships.
 */

import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { test, type Page } from '@playwright/test';
import { openApp, settle, type Theme } from './support/app';
import { startBackend, type Backend } from './support/backend';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));
const SHOTS = resolve(fileURLToPath(new URL('./shots', import.meta.url)));

const THEMES: readonly Theme[] = ['dark', 'light'];
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let uiDir: string;
/** The run with three answers, one unreachable device and a real difference. */
let comparison = '';

async function rpc<T = unknown>(method: string, params?: unknown): Promise<T> {
  const response = await fetch(`${backend.url}/api/rpc`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${backend.token}` },
    body: JSON.stringify({ method, params: params ?? null }),
  });
  const body = (await response.json()) as
    | { ok: true; result: T }
    | { ok: false; error: { code: string; message: string } };
  if (!body.ok) throw new Error(`${method} failed: ${body.error.code} — ${body.error.message}`);
  return body.result;
}

const wait = (ms: number) => new Promise<void>((done) => setTimeout(done, ms));

/** Starts a run and waits for the record to be finished. */
async function runCommands(params: Record<string, unknown>): Promise<string> {
  const started = await rpc<{ run_id: string }>('runner.run', params);
  for (let attempt = 0; attempt < 60; attempt += 1) {
    const detail = await rpc<{ run: { finished_at?: number | null } }>('runner.get', {
      run_id: started.run_id,
    });
    if (detail.run.finished_at != null) return started.run_id;
    await wait(500);
  }
  throw new Error('the run never finished');
}

test.beforeAll(async () => {
  test.setTimeout(600_000);
  uiDir = process.env.NOCTURNE_UI_DIST ?? (await mkdtemp(join(tmpdir(), 'nocturne-ui-')));
  if (process.env.NOCTURNE_UI_DIST === undefined) {
    await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
      cwd: UI_DIR,
      timeout: 240_000,
    });
  }
  backend = await startBackend({ uiDir });

  const lab = await rpc<{ id: string }>('labs.create', { name: 'Campus' });
  for (const [model, name] of [
    ['switch', 'AS1'],
    ['switch', 'AS2'],
    ['router', 'EDGE'],
  ] as const) {
    await rpc('labs.node.add', { lab_id: lab.id, model, name });
  }
  await rpc('labs.start', { id: lab.id });
  await rpc('devices.create', {
    name: 'dead-sw',
    address: '192.0.2.77',
    transport: 'ssh',
    profile: 'cisco_ios',
    site: 'HQ',
    role: 'access',
    tags: ['critical'],
  });

  // The picture the screen exists for: one command, four devices, two that
  // agree, one that differs and one that was never reached.
  comparison = await runCommands({
    filter: {},
    commands: ['show ip interface brief'],
    name: 'Interface state across the campus',
  });
  await runCommands({ filter: { lab: true }, commands: ['show version'], name: 'Software versions' });

  // A device held by a playbook, so a run really is skipped.
  const holder = await rpc<{ job_id: string }>('playbooks.run', {
    device_ids: ['Campus/AS1'],
    yaml: 'name: Holder\nsteps:\n  - name: Hold the device\n    pause: 20\n',
    name: 'Nightly evidence',
  });
  await runCommands({ device_ids: ['Campus/AS1'], commands: ['show clock'], name: 'Clock check' });
  await rpc('jobs.cancel', { id: holder.job_id });

  // Something saved in every tab that has a list.
  const task = await rpc<{ id: string }>('tasks.save', {
    name: 'Access-switch health',
    description: 'What to look at before a change: version, uptime, interfaces.',
    select: { filter: { lab: true, role: 'switch' } },
    action: { type: 'commands', commands: ['show version', 'show ip interface brief'], parse: true },
    tags: ['daily'],
  });
  await rpc('tasks.save', {
    name: 'Edge router evidence',
    select: { device_ids: ['Campus/EDGE'] },
    action: { type: 'commands', commands: ['show ip route'] },
  });
  await rpc('playbooks.save', {
    name: 'Pre-change health check',
    description: 'Collect, read, and refuse to go on if an uplink is down.',
    yaml: [
      'name: Pre-change health check',
      'vars: { min_uplinks: 1 }',
      'steps:',
      '  - name: Collect state',
      '    collect: [facts, interfaces]',
      '  - name: Save evidence',
      '    commands: [show version]',
      '    register: evidence',
      '  - name: Uplinks are up',
      '    assert:',
      "      that: \"interfaces | selectattr('oper_up') | list | length >= vars.min_uplinks\"",
      '      message: "{{ device.name }} has no uplink up"',
      '',
    ].join('\n'),
  });
  await rpc('schedules.save', {
    name: 'Nightly health check',
    when: { type: 'daily', at: '02:30' },
    action: { method: 'tasks.run', params: { id: task.id } },
  });
  await rpc('schedules.save', {
    name: 'Hourly interface sweep',
    when: { type: 'interval', every_s: 3_600 },
    action: { method: 'runner.run', params: { filter: { lab: true }, commands: ['show ip interface brief'] } },
  });
  await rpc('triggers.save', {
    name: 'Evidence when a device goes down',
    topic: 'devices.state',
    match_payload: { '/reachable': false },
    debounce_s: 30,
    pass_device: true,
    action: { method: 'tasks.run', params: { id: task.id } },
  });
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir && process.env.NOCTURNE_UI_DIST === undefined) {
    await rm(uiDir, { recursive: true, force: true });
  }
});

async function shoot(page: Page, theme: Theme, name: string): Promise<void> {
  const directory = join(SHOTS, THEME_FOLDER[theme]);
  await mkdir(directory, { recursive: true });
  await settle(page);
  await page.screenshot({ path: join(directory, `${name}.png`), animations: 'disabled' });
}

for (const theme of THEMES) {
  test.describe(`${THEME_FOLDER[theme]} theme`, () => {
    test('the composer and the runs behind it', async ({ page }) => {
      await openApp(page, '/automation', { backend, theme });
      await page.getByRole('table', { name: 'Runs' }).locator('tbody tr').first().waitFor();
      await shoot(page, theme, 'automation-run');

      await page.setViewportSize({ width: 1280, height: 720 });
      await shoot(page, theme, 'automation-run-1280');
      await page.setViewportSize({ width: 1440, height: 900 });
    });

    test('the composer with the assistant dock open', async ({ page }) => {
      await openApp(page, '/automation', { backend, theme, dock: true });
      await page.getByRole('table', { name: 'Runs' }).locator('tbody tr').first().waitFor();
      await shoot(page, theme, 'automation-run-dock');
    });

    test('the comparison', async ({ page }) => {
      await openApp(page, `/automation/run?run=${comparison}`, { backend, theme });
      await page.getByText('2 say one thing, 1 differs').waitFor();
      await shoot(page, theme, 'automation-compare');

      await page.setViewportSize({ width: 1280, height: 720 });
      await shoot(page, theme, 'automation-compare-1280');
      await page.setViewportSize({ width: 1440, height: 900 });

      // The lines that were ignored, and the devices with nothing to compare.
      await page.getByText(/Lines matching \d+ patterns/).click();
      await shoot(page, theme, 'automation-compare-ignored');
    });

    test('the comparison with the assistant dock open', async ({ page }) => {
      await openApp(page, `/automation/run?run=${comparison}`, { backend, theme, dock: true });
      await page.getByText('2 say one thing, 1 differs').waitFor();
      await shoot(page, theme, 'automation-compare-dock');
    });

    test('what each device did', async ({ page }) => {
      await openApp(page, `/automation/run?run=${comparison}`, { backend, theme });
      await page.getByRole('tab', { name: 'Devices' }).click();
      await page.getByRole('region', { name: /^Not reached/ }).waitFor();
      await shoot(page, theme, 'automation-devices');

      // One device's answer, as it printed it.
      await page.getByRole('button', { name: 'Campus/EDGE' }).click();
      await shoot(page, theme, 'automation-device-output');

      await page.setViewportSize({ width: 1280, height: 720 });
      await shoot(page, theme, 'automation-devices-1280');
      await page.setViewportSize({ width: 1440, height: 900 });
    });

    test('tasks and the task editor', async ({ page }) => {
      await openApp(page, '/automation/tasks', { backend, theme });
      await page.getByRole('table', { name: 'Saved tasks' }).locator('tbody tr').first().waitFor();
      await shoot(page, theme, 'automation-tasks');

      await page.getByRole('cell', { name: 'Access-switch health' }).first().dblclick();
      await page.getByRole('dialog').waitFor();
      await page.getByText('2 devices right now').waitFor();
      await shoot(page, theme, 'automation-task');
      await page.keyboard.press('Escape');
    });

    test('templates and one template open', async ({ page }) => {
      await openApp(page, '/automation/templates', { backend, theme });
      await page.getByRole('table', { name: 'Configuration templates' }).locator('tbody tr').first().waitFor();
      await shoot(page, theme, 'automation-templates');

      await page.getByRole('cell', { name: 'NTP servers (IOS)' }).first().dblclick();
      await page.getByRole('textbox', { name: 'NTP servers' }).fill('192.0.2.1');
      await page.getByRole('textbox', { name: 'NTP servers' }).press('Enter');
      await page.getByRole('combobox', { name: 'Preview against' }).click();
      await page.getByRole('option', { name: 'Campus/AS1' }).click();
      await page.getByText('ntp server 192.0.2.1').waitFor();
      // Typing into the variables scrolls the editor; the picture is of the
      // whole screen, so put it back at the top first.
      await page.getByRole('button', { name: 'All templates' }).scrollIntoViewIfNeeded();
      await shoot(page, theme, 'automation-template');

      await page.setViewportSize({ width: 1280, height: 720 });
      await shoot(page, theme, 'automation-template-1280');
      await page.setViewportSize({ width: 1440, height: 900 });

      // The one value that may span lines: the banner's text, in a text area,
      // rendered for a real switch.
      await openApp(page, '/automation/templates', { backend, theme });
      await page.getByRole('cell', { name: 'Login banner (IOS)' }).first().dblclick();
      await page
        .getByRole('textbox', { name: 'Banner text', exact: true })
        .fill('Authorised use only.\nDisconnect now if you are not.');
      await page.getByRole('combobox', { name: 'Preview against' }).click();
      await page.getByRole('option', { name: 'Campus/AS1' }).click();
      // Below the body: the text area, and the four lines it rendered.
      await page.getByText('Campus/AS1: 4 lines').scrollIntoViewIfNeeded();
      await shoot(page, theme, 'automation-template-banner');
    });

    test('playbooks', async ({ page }) => {
      await openApp(page, '/automation/playbooks', { backend, theme });
      await page.getByRole('cell', { name: 'Pre-change health check' }).first().dblclick();
      await page.getByText('3 steps, all valid').waitFor();
      await shoot(page, theme, 'automation-playbook');
    });

    test('schedules, triggers and the schedule editor', async ({ page }) => {
      await openApp(page, '/automation/schedules', { backend, theme });
      await page.getByRole('table', { name: 'Schedules' }).locator('tbody tr').first().waitFor();
      await shoot(page, theme, 'automation-schedules');

      await page.setViewportSize({ width: 1280, height: 720 });
      await shoot(page, theme, 'automation-schedules-1280');
      await page.setViewportSize({ width: 1440, height: 900 });

      // `exact`: a row's "On" switch is labelled "<name> is on", so a
      // substring match finds that cell first — and double-clicking it
      // toggles the schedule instead of opening it.
      await page.getByRole('cell', { name: 'Nightly health check', exact: true }).dblclick();
      await page.getByRole('dialog').waitFor();
      await shoot(page, theme, 'automation-schedule');
      await page.keyboard.press('Escape');
    });

    test('the Ansible tab, which leads with what it cannot promise', async ({ page }) => {
      await openApp(page, '/automation/ansible', { backend, theme });
      await page.getByRole('note', { name: 'Outside the change pipeline' }).waitFor();
      await shoot(page, theme, 'automation-ansible');

      await page.setViewportSize({ width: 1280, height: 720 });
      await shoot(page, theme, 'automation-ansible-1280');
      await page.setViewportSize({ width: 1440, height: 900 });
    });
  });
}
