/**
 * `pnpm shots` — the Lab screen, photographed in both themes, at both sizes,
 * with the assistant dock open and closed.
 *
 * Taken against a real engine with a real lab in it: a screenshot of an empty
 * canvas says nothing about how the screen reads. The campus example is
 * started, a console is opened and typed into, and the reachability drawer is
 * made to show a packet that really died, so what is in the picture is what a
 * user would see.
 */

import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import { openApp, settle, type Theme } from './support/app';
import { startBackend, type Backend } from './support/backend';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));
const SHOTS = resolve(fileURLToPath(new URL('./shots/lab', import.meta.url)));
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };
const THEMES: readonly Theme[] = ['dark', 'light'];

test.describe.configure({ mode: 'serial', timeout: 420_000 });

let backend: Backend;
let uiDir: string;
let campus = '';

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

test.beforeAll(async () => {
  test.setTimeout(420_000);
  uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
  await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
    cwd: UI_DIR,
    timeout: 300_000,
  });
  backend = await startBackend({ uiDir });

  // A lab worth photographing, and a second one so the list is a list.
  const lab = await rpc<{ id: string }>('labs.create', { name: 'Campus', example: 'demo' });
  campus = lab.id;
  await rpc('labs.start', { id: campus });
  await rpc('labs.create', { name: 'OSPF triangle', example: 'ospf_triangle' });
  // A real device, so the twin drawer has something to offer.
  await rpc('devices.create', {
    name: 'core-sw1',
    address: '10.0.0.11',
    profile: 'cisco_ios',
    site: 'HQ',
  });
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

function node(page: Page, name: string) {
  return page.getByRole('button', { name: new RegExp(`^${name},`) });
}

/** Leaves the engine with no console sessions, so each shot starts the same. */
async function clearSessions(): Promise<void> {
  const sessions = await rpc<{ id: string }[]>('terminal.list');
  for (const session of sessions) await rpc('terminal.close', { session_id: session.id });
}

for (const theme of THEMES) {
  test(`lab screens — ${theme}`, async ({ page }) => {
    test.setTimeout(240_000);
    await clearSessions();

    // ------------------------------------------------------------ the list
    await openApp(page, '/lab', { theme, backend });
    await expect(page.getByRole('heading', { level: 1, name: 'Lab' })).toBeVisible();
    await expect(page.getByRole('listitem').filter({ hasText: 'Campus' }).first()).toBeVisible();
    await shoot(page, theme, 'list');

    await page.getByRole('button', { name: 'New lab' }).first().click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await shoot(page, theme, 'new-lab');
    await page.keyboard.press('Escape');

    await page.getByRole('button', { name: 'Twin the real network…' }).click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await expect(page.getByText('core-sw1').first()).toBeVisible();
    await shoot(page, theme, 'twin');
    await page.keyboard.press('Escape');

    // ---------------------------------------------------------- the canvas
    await openApp(page, `/lab/${campus}`, { theme, backend });
    await expect(node(page, 'EDGE')).toBeVisible({ timeout: 60_000 });
    await node(page, 'DS1').click();
    await expect(page.getByText('Layer-3 switch').first()).toBeVisible();
    await shoot(page, theme, 'canvas');

    // 1280×720: the size the screen has to work at.
    await page.setViewportSize({ width: 1280, height: 720 });
    await page.getByRole('button', { name: 'Fit to view' }).click();
    await shoot(page, theme, 'canvas-1280');
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.getByRole('button', { name: 'Fit to view' }).click();

    // ------------------------------------------------------ reachability
    await page.getByRole('button', { name: 'Reachability' }).click();
    const drawer = page.getByRole('dialog', { name: 'Reachability' });
    await drawer.getByRole('combobox', { name: 'From' }).selectOption('Guest-Laptop1');
    await drawer.getByRole('combobox', { name: 'To' }).selectOption('SRV1');
    await drawer.getByRole('button', { name: 'Check this pair' }).click();
    // The guest VLAN is fenced off by an access list: a packet that really dies.
    await expect(drawer.getByText(/cannot reach/)).toBeVisible({ timeout: 30_000 });
    await shoot(page, theme, 'reachability');

    await drawer.getByRole('button', { name: 'Check every pair' }).click();
    await expect(drawer.getByText(/ordered pairs get through/)).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'matrix');
    await page.keyboard.press('Escape');

    // --------------------------------------------------------- the console
    await node(page, 'AS1').dblclick();
    await expect(page.getByRole('tab', { name: /AS1/ })).toBeVisible({ timeout: 30_000 });
    await page.locator('.xterm-screen:visible').first().click();
    for (const line of ['enable', 'terminal length 0', 'show vlan brief']) {
      await page.keyboard.type(`${line}\r`);
      await page.waitForTimeout(250);
    }
    await shoot(page, theme, 'console');

    // ------------------------------------------- with the assistant dock
    await openApp(page, `/lab/${campus}`, { theme, backend, dock: true });
    await expect(node(page, 'EDGE')).toBeVisible({ timeout: 60_000 });
    await node(page, 'AS2').click();
    await shoot(page, theme, 'canvas-dock');

    await page.setViewportSize({ width: 1280, height: 720 });
    await page.getByRole('button', { name: 'Fit to view' }).click();
    await shoot(page, theme, 'canvas-dock-1280');
    await page.setViewportSize({ width: 1440, height: 900 });

    await clearSessions();
  });
}
