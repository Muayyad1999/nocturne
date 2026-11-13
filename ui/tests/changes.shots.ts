/**
 * `pnpm shots` — the Changes screen, photographed in both themes at the two
 * sizes that matter, with and without the assistant dock.
 *
 * Taken against the real engine with a real (simulated) switch behind it, so
 * every command, diff, risk and protected resource in these pictures is one
 * the engine actually produced. A screenshot of an invented plan would be
 * worth nothing: the whole point of this screen is that it shows the truth.
 */

import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import { openApp, settle, type Theme } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor, type Rpc } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));
const SHOTS = resolve(fileURLToPath(new URL('./shots/changes', import.meta.url)));
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };
const THEMES: readonly Theme[] = ['dark', 'light'];

test.describe.configure({ mode: 'serial', timeout: 600_000 });

let backend: Backend;
let uiDir: string;
/** Change ids, filled in by the seed below. */
const ids: Record<string, string> = {};
let device = '';

async function rpcOf(request: Parameters<typeof rpcFor>[0]): Promise<Rpc> {
  return rpcFor(request, backend);
}

async function plan(
  rpc: Rpc,
  title: string,
  lines: string[],
  extra: Record<string, unknown> = {},
): Promise<string> {
  const created = await rpc<{ change: { id: string } }>('changes.create', {
    title,
    targets: [{ device_id: device, intent: { type: 'lines', lines } }],
    plan: true,
    ...extra,
  });
  const id = created.change.id;
  await expect
    .poll(async () => (await rpc<{ state: string }>('changes.get', { id })).state, {
      timeout: 60_000,
    })
    .toBe('planned');
  return id;
}

test.beforeAll(async ({ request }) => {
  test.setTimeout(600_000);
  uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
  await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
    cwd: UI_DIR,
    timeout: 240_000,
  });
  backend = await startBackend({ uiDir });
  const rpc = await rpcOf(request);

  const lab = await rpc<{ id: string }>('labs.create', { name: 'Campus' });
  await rpc('labs.node.add', { lab_id: lab.id, model: 'switch', name: 'access-sw-3' });
  await rpc('labs.start', { id: lab.id });
  device = 'Campus/access-sw-3';

  // One waiting for approval, low risk.
  ids.waiting = await plan(
    rpc,
    'Guest VLAN on the east access ports',
    [
      'interface FastEthernet0/5',
      'description guest — reception',
      'switchport mode access',
      'exit',
      'interface FastEthernet0/6',
      'description guest — meeting room',
      'switchport mode access',
      'exit',
    ],
    { ticket: 'CHG-1042', description: 'Two reception ports move to the guest VLAN.' },
  );

  // One that touches the way Nocturne gets in: critical, protected, and the
  // approve bar has to ask for the device's name.
  ids.protectedChange = await plan(
    rpc,
    'Require SSH on the login lines',
    ['line vty 0 4', 'transport input ssh', 'login local', 'exit'],
    { ticket: 'CHG-1043' },
  );

  // One that has been through the pipeline, so the stepper and the
  // verification have something real in them.
  ids.applied = await plan(rpc, 'Describe the uplink', [
    'interface GigabitEthernet0/1',
    'description uplink to dist-1',
    'exit',
  ]);
  const change = await rpc<{ targets: { plan: { plan_hash: string } }[] }>('changes.get', {
    id: ids.applied,
  });
  await rpc('changes.approve', {
    id: ids.applied,
    plan_hashes: [change.targets[0]?.plan.plan_hash],
    note: 'Reviewed with the evening shift.',
  });
  await rpc('changes.apply', { id: ids.applied });
  await expect
    .poll(async () => (await rpc<{ state: string }>('changes.get', { id: ids.applied })).state, {
      timeout: 180_000,
    })
    .toBe('applied');

  // And one the assistant drafted, so the list shows the AI tint.
  ids.assistant = await plan(
    rpc,
    'Name the unused access ports',
    ['interface FastEthernet0/20', 'description spare', 'exit'],
    { origin: 'assistant' },
  );
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

async function shoot(page: Page, theme: Theme, name: string): Promise<void> {
  const directory = join(SHOTS, THEME_FOLDER[theme]);
  await mkdir(directory, { recursive: true });
  await settle(page);
  await page.screenshot({ path: join(directory, `${name}.png`), animations: 'disabled' });
}

for (const theme of THEMES) {
  test(`changes — ${theme}`, async ({ page }) => {
    test.setTimeout(300_000);

    await openApp(page, '/changes', { backend, theme });
    await expect(page.getByRole('table', { name: 'Changes' })).toBeVisible();
    await shoot(page, theme, 'list');

    // The change everybody has to read before it goes anywhere.
    await openApp(page, `/changes/${ids.waiting}`, { backend, theme });
    await expect(page.getByTestId('command-line').first()).toBeVisible();
    await shoot(page, theme, 'plan');

    // The one that cannot be approved without typing the device's name.
    await openApp(page, `/changes/${ids.protectedChange}`, { backend, theme });
    await expect(page.getByTestId('protected-hit').first()).toBeVisible();
    await page.getByRole('textbox', { name: `Type ${device} to confirm` }).scrollIntoViewIfNeeded();
    await shoot(page, theme, 'protected');

    // What it looks like afterwards.
    await openApp(page, `/changes/${ids.applied}`, { backend, theme });
    await expect(page.getByTestId('apply-step').first()).toBeVisible();
    await shoot(page, theme, 'applied');

    // Drafting one.
    await openApp(page, '/changes', { backend, theme });
    await page.getByRole('button', { name: 'New change' }).first().click();
    const dialog = page.getByRole('dialog');
    await dialog.getByText(device, { exact: true }).click();
    await dialog.getByRole('button', { name: 'Continue' }).click();
    await dialog
      .getByRole('textbox', { name: 'Configuration lines' })
      .fill('interface FastEthernet0/9\ndescription printer — second floor\nswitchport mode access\nexit');
    await dialog.getByRole('button', { name: `Preview on ${device}` }).click();
    await expect(dialog.getByTestId('command-line').first()).toBeVisible();
    await shoot(page, theme, 'new-change');
    await page.keyboard.press('Escape');

    // The narrow end of the range, and the same page with the dock open.
    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page, `/changes/${ids.waiting}`, { backend, theme });
    await expect(page.getByTestId('command-line').first()).toBeVisible();
    await shoot(page, theme, 'plan-1280');

    await openApp(page, `/changes/${ids.protectedChange}`, { backend, theme, dock: true });
    await expect(page.getByTestId('protected-hit').first()).toBeVisible();
    await shoot(page, theme, 'protected-1280-dock');

    await page.setViewportSize({ width: 1440, height: 900 });
    await openApp(page, '/changes', { backend, theme, dock: true });
    await expect(page.getByRole('table', { name: 'Changes' })).toBeVisible();
    await shoot(page, theme, 'list-dock');
  });
}
