/**
 * `pnpm shots` — the Configuration screen, photographed in both themes at
 * the two sizes that matter, with and without the assistant dock.
 *
 * Taken against the real engine with real simulated switches behind it, so
 * every configuration, diff, search result and compliance finding in these
 * pictures is one the engine produced from a device. A screenshot of invented
 * data would prove nothing about a screen whose whole job is to show what a
 * device actually says.
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
const SHOTS = resolve(fileURLToPath(new URL('./shots/config', import.meta.url)));
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };
const THEMES: readonly Theme[] = ['dark', 'light'];

test.describe.configure({ mode: 'serial', timeout: 900_000 });

let backend: Backend;
let uiDir: string;
/** Inventory ids of the lab's switches, and the two versions of SW1. */
const ids: Record<string, string> = {};
const versions: number[] = [];

/** A configured access switch: descriptions, a user password, an SNMP community. */
const CONFIGURED = `!
version 15.2
no service password-encryption
!
hostname SW1
!
username netops privilege 15 password 0 sup3rsecret
no aaa new-model
!
snmp-server community public RO
!
spanning-tree mode pvst
!
interface FastEthernet0/1
 description uplink to dist-1
!
interface FastEthernet0/2
 description reception desk — guest VLAN
!
interface FastEthernet0/3
 description printer, second floor
!
interface Vlan1
 no ip address
 shutdown
!
line con 0
line vty 0 4
 login
!
end
`;

async function backUp(rpc: Rpc, deviceIds: string[]): Promise<void> {
  const started = await rpc<{ job_id: string }>('config.backup', { device_ids: deviceIds });
  await expect
    .poll(async () => (await rpc<{ state: string }>('jobs.get', { id: started.job_id })).state, {
      timeout: 120_000,
    })
    .not.toBe('running');
}

test.beforeAll(async ({ request }) => {
  test.setTimeout(900_000);
  uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
  await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
    cwd: UI_DIR,
    timeout: 240_000,
  });
  backend = await startBackend({ uiDir });
  const rpc = rpcFor(request, backend);

  const lab = await rpc<{ id: string }>('labs.create', { name: 'Campus' });
  for (const node of ['access-sw-1', 'access-sw-2', 'dist-sw-1']) {
    await rpc('labs.node.add', { lab_id: lab.id, model: 'switch', name: node });
  }
  await rpc('labs.start', { id: lab.id });

  const devices = await rpc<{ id: string; name: string }[]>('devices.list', {});
  for (const device of devices) ids[device.name] = device.id;

  // Two switches captured, then one of them changed at its console and
  // captured again: a real pair of versions to diff.
  const captured = [ids['Campus/access-sw-1']!, ids['Campus/access-sw-2']!];
  await backUp(rpc, captured);
  await rpc('labs.node.config_set', { lab_id: lab.id, node: 'access-sw-1', startup: CONFIGURED });
  await backUp(rpc, [ids['Campus/access-sw-1']!]);
  const stored = await rpc<{ id: number }[]>('config.versions', {
    device_id: ids['Campus/access-sw-1']!,
  });
  versions.push(...stored.map((version) => version.id));

  // A third switch with nothing captured, so the coverage table shows what
  // "never captured" looks like next to devices that have been read.
  // (`dist-sw-1` is deliberately left alone.)

  // An intended configuration, so the drift view has something in it.
  await rpc('config.intended.set', {
    device_id: ids['Campus/access-sw-1']!,
    text: 'service password-encryption\nip ssh version 2\nntp server 10.0.0.1\nlogging host 10.0.0.5\n',
    exact: false,
  });

  await rpc('compliance.run', {});
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
  test(`config — ${theme}`, async ({ page }) => {
    test.setTimeout(600_000);
    const device = ids['Campus/access-sw-1']!;
    const never = ids['Campus/dist-sw-1']!;

    // Who is backed up and who is not.
    await openApp(page, '/config/backups', { backend, theme });
    await expect(page.getByRole('table', { name: 'Backup coverage' })).toBeVisible();
    await shoot(page, theme, 'backups');

    // One device's configuration, with its versions and the redaction note.
    await openApp(page, `/config/history?device=${device}`, { backend, theme });
    await expect(page.getByTestId('redaction-note')).toBeVisible();
    await expect(page.getByRole('textbox', { name: /Running configuration, version/ })).toContainText(
      'hostname SW1',
    );
    await shoot(page, theme, 'history');

    // The engine's diff between the two versions.
    for (const version of versions) {
      await page.getByRole('checkbox', { name: `Compare version ${version}` }).check();
    }
    await expect(page.getByTestId('diff-line').first()).toBeVisible();
    await shoot(page, theme, 'diff');
    await page.getByRole('button', { name: 'Side by side' }).click();
    await expect(page.getByTestId('diff-row').first()).toBeVisible();
    await shoot(page, theme, 'diff-split');

    // A device nobody has captured: not an empty configuration.
    await openApp(page, `/config/history?device=${never}`, { backend, theme });
    await expect(page.getByRole('button', { name: 'Back up this device now' })).toBeVisible();
    await shoot(page, theme, 'never-captured');

    // The search everybody uses.
    await openApp(page, '/config/search?q=description', { backend, theme });
    await expect(page.getByTestId('search-hit').first()).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'search');

    // Compliance: the policies, the score and the findings.
    await openApp(page, '/config/compliance', { backend, theme });
    await expect(page.getByRole('table', { name: 'Compliance findings' })).toBeVisible();
    await shoot(page, theme, 'compliance');

    // The policy list, which opens collapsed so the findings come first.
    await page.getByRole('button', { name: 'Policies', exact: true }).click();
    await expect(page.getByTestId('policy').first()).toBeVisible();
    await shoot(page, theme, 'policies');

    // One rule in full.
    await page.getByTestId('rule-chip').nth(3).click();
    await expect(page.getByRole('dialog').filter({ hasText: 'Why it matters' })).toBeVisible();
    await shoot(page, theme, 'rule');
    await page.keyboard.press('Escape');

    // Drift, and the hand-over to the change pipeline.
    await openApp(page, `/config/intended?device=${device}`, { backend, theme });
    await expect(page.getByTestId('drift-command').first()).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'drift');

    // The narrow end of the range, and the same screens with the dock open.
    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page, `/config/history?device=${device}`, { backend, theme });
    await expect(page.getByTestId('redaction-note')).toBeVisible();
    // The editor arrives in its own chunk; a screenshot of the skeleton
    // would say nothing about the screen.
    await expect(page.getByRole('textbox', { name: /Running configuration, version/ })).toBeVisible();
    await shoot(page, theme, 'history-1280');

    await openApp(page, '/config/search?q=snmp-server+community', { backend, theme, dock: true });
    await expect(page.getByTestId('search-hit').first()).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'search-1280-dock');

    await openApp(page, '/config/compliance', { backend, theme, dock: true });
    await expect(page.getByRole('table', { name: 'Compliance findings' })).toBeVisible();
    await shoot(page, theme, 'compliance-1280-dock');

    await page.setViewportSize({ width: 1440, height: 900 });
    await openApp(page, '/config/backups', { backend, theme, dock: true });
    await expect(page.getByRole('table', { name: 'Backup coverage' })).toBeVisible();
    await shoot(page, theme, 'backups-dock');
  });
}
